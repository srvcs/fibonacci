use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::{OpenApi, ToSchema};

use crate::client::{self, DepError};

pub const SERVICE: &str = "srvcs-fibonacci";
pub const CONCERN: &str = "sequences: nth Fibonacci number (0-indexed)";
pub const DEPENDS_ON: &[&str] = &["srvcs-add"];

/// Upper bound on loop iterations. `fibonacci(n)` overflows `i64` well before
/// `n == 92`, so the sequence index is naturally tiny; this cap defends against
/// a misbehaving caller (or arithmetic that never advances), surfacing a `500`
/// instead of looping unboundedly.
const MAX_ITERATIONS: i64 = 100_000;

/// Dependency endpoints, injected as router state so tests can point them at
/// mock services.
#[derive(Clone)]
pub struct Deps {
    pub add_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct Info {
    pub service: &'static str,
    pub concern: &'static str,
    pub depends_on: Vec<&'static str>,
}

/// `GET /` — service identity (srvcs service standard).
#[utoipa::path(get, path = "/", responses((status = 200, body = Info)))]
pub async fn index() -> Json<Info> {
    Json(Info {
        service: SERVICE,
        concern: CONCERN,
        depends_on: DEPENDS_ON.to_vec(),
    })
}

#[derive(Deserialize, ToSchema)]
pub struct EvalRequest {
    pub value: i64,
}

#[derive(Serialize, ToSchema)]
pub struct FibonacciResponse {
    pub value: i64,
    pub result: i64,
}

fn ok(value: i64, result: i64) -> Response {
    (
        StatusCode::OK,
        Json(json!({ "value": value, "result": result })),
    )
        .into_response()
}

fn invalid(reason: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": reason })),
    )
        .into_response()
}

fn degraded(dependency: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "dependency unavailable", "dependency": dependency })),
    )
        .into_response()
}

fn forward(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    (code, Json(body)).into_response()
}

fn loop_exhausted() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "fibonacci loop exceeded iteration cap" })),
    )
        .into_response()
}

/// Call one dependency at `url` with `body`, mapping its outcome to either the
/// parsed response body (on `200`) or an early-return `Response` the caller
/// should surface verbatim:
///
/// - unreachable / non-`200`/`422` -> `503` degraded
/// - `422` -> forwarded `422` (the dependency rejected the input)
async fn ask(url: &str, body: &Value, dependency: &str) -> Result<Value, Response> {
    match client::call(url, body).await {
        Err(DepError::Unreachable) => Err(degraded(dependency)),
        Ok((200, body)) => Ok(body),
        Ok((422, body)) => Err(forward(422, body)),
        Ok(_) => Err(degraded(dependency)),
    }
}

/// `POST /` — compute the `n`th Fibonacci number (0-indexed).
///
/// This service owns the *control flow* — an iterative fold over the sequence —
/// but does no arithmetic of its own: each successive term is produced by
/// asking [`srvcs-add`] for `a + b`. `fibonacci(0) == 0`, `fibonacci(1) == 1`,
/// `fibonacci(10) == 55`.
///
/// A negative index is rejected with `422` locally. If `srvcs-add` is
/// unreachable it reports itself degraded (`503`); if `srvcs-add` rejects an
/// operand it forwards the `422`; and if the loop somehow exceeds its iteration
/// cap it returns `500` rather than spinning.
#[utoipa::path(
    post,
    path = "/",
    request_body = EvalRequest,
    responses(
        (status = 200, body = FibonacciResponse),
        (status = 422, description = "n is negative, or a dependency rejected the input (forwarded)"),
        (status = 500, description = "the fibonacci loop exceeded its iteration cap"),
        (status = 503, description = "a dependency is unavailable")
    )
)]
pub async fn evaluate(State(deps): State<Deps>, Json(req): Json<EvalRequest>) -> Response {
    let n = req.value;
    if n < 0 {
        return invalid("n must be >= 0");
    }
    if n > MAX_ITERATIONS {
        return loop_exhausted();
    }

    let mut a: i64 = 0;
    let mut b: i64 = 1;
    for _ in 0..n {
        // The defining operation — the sum producing the next term — goes
        // through srvcs-add.
        let body = match ask(&deps.add_url, &json!({ "a": a, "b": b }), "srvcs-add").await {
            Ok(body) => body,
            Err(resp) => return resp,
        };
        let next = match body.get("result").and_then(Value::as_i64) {
            Some(next) => next,
            None => return degraded("srvcs-add"),
        };
        a = b;
        b = next;
    }

    ok(n, a)
}

#[derive(OpenApi)]
#[openapi(
    paths(index, evaluate),
    components(schemas(Info, EvalRequest, FibonacciResponse))
)]
pub struct ApiDoc;

/// Serve OpenAPI document
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_documents_routes() {
        let doc = ApiDoc::openapi();
        let root = doc.paths.paths.get("/").expect("path / present");
        assert!(root.get.is_some());
        assert!(root.post.is_some());
    }

    #[tokio::test]
    async fn index_reports_dependency() {
        let Json(info) = index().await;
        assert_eq!(info.service, "srvcs-fibonacci");
        assert_eq!(info.concern, "sequences: nth Fibonacci number (0-indexed)");
        assert_eq!(info.depends_on, vec!["srvcs-add"]);
    }
}
