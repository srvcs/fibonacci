use axum::body::Body;
use axum::extract::Json as AxumJson;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router as AxumRouter};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use srvcs_fibonacci::{api::Deps, health, router, telemetry};
use tower::ServiceExt;

const DEAD_URL: &str = "http://127.0.0.1:1";

/// Spawn a *computing* mock `srvcs-add`: it reads `{"a": a, "b": b}` and returns
/// `{"result": a + b}` — the real sum, so the Fibonacci fold is genuinely driven
/// by the dependency rather than a canned response.
async fn spawn_add() -> String {
    let app = AxumRouter::new().route(
        "/",
        post(|AxumJson(body): AxumJson<Value>| async move {
            let a = body.get("a").and_then(Value::as_i64).unwrap_or(0);
            let b = body.get("b").and_then(Value::as_i64).unwrap_or(0);
            (StatusCode::OK, Json(json!({ "result": a + b })))
        }),
    );
    serve(app).await
}

/// Spawn a mock returning a fixed status + body (used for the 422-forward case).
async fn spawn_fixed(status: StatusCode, body: Value) -> String {
    let app = AxumRouter::new().route(
        "/",
        post(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        }),
    );
    serve(app).await
}

async fn serve(app: AxumRouter) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn app(add_url: &str) -> axum::Router {
    router(
        telemetry::metrics_handle_for_tests(),
        Deps {
            add_url: add_url.to_string(),
        },
    )
}

async fn fib(add_url: &str, value: i64) -> (StatusCode, Value) {
    let res = app(add_url)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "value": value }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn status_of(uri: &str) -> StatusCode {
    app(DEAD_URL)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn index_ok() {
    assert_eq!(status_of("/").await, StatusCode::OK);
}

#[tokio::test]
async fn healthz_ok() {
    assert_eq!(status_of("/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn readyz_reflects_state() {
    health::set_ready(true);
    assert_eq!(status_of("/readyz").await, StatusCode::OK);
}

#[tokio::test]
async fn metrics_ok() {
    assert_eq!(status_of("/metrics").await, StatusCode::OK);
}

#[tokio::test]
async fn openapi_ok() {
    assert_eq!(status_of("/openapi.json").await, StatusCode::OK);
}

#[tokio::test]
async fn fibonacci_0_is_0() {
    // n == 0: the loop never runs and add is never called.
    let add = spawn_add().await;
    let (status, body) = fib(&add, 0).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], 0);
    assert_eq!(body["result"], 0);
}

#[tokio::test]
async fn fibonacci_1_is_1() {
    let add = spawn_add().await;
    let (status, body) = fib(&add, 1).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], 1);
}

#[tokio::test]
async fn fibonacci_10_is_55() {
    let add = spawn_add().await;
    let (status, body) = fib(&add, 10).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["value"], 10);
    assert_eq!(body["result"], 55);
}

#[tokio::test]
async fn fibonacci_20_is_6765() {
    let add = spawn_add().await;
    let (status, body) = fib(&add, 20).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], 6765);
}

#[tokio::test]
async fn negative_index_is_422() {
    // Rejected locally before any dependency call.
    let add = spawn_add().await;
    let (status, body) = fib(&add, -1).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "n must be >= 0");
}

#[tokio::test]
async fn forwards_422_from_add() {
    // add rejects the operand -> forward 422.
    let add = spawn_fixed(
        StatusCode::UNPROCESSABLE_ENTITY,
        json!({ "error": "bad operand" }),
    )
    .await;
    let (status, _) = fib(&add, 5).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn degrades_when_add_unreachable() {
    // n >= 1 so the loop reaches the add call against a dead url.
    let (status, body) = fib(DEAD_URL, 5).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"], "srvcs-add");
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let res = app(DEAD_URL)
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.headers().contains_key("x-request-id"),
        "response must carry a generated x-request-id"
    );
}
