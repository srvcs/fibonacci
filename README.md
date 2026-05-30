# srvcs-fibonacci

The Fibonacci-sequence orchestrator of the srvcs.cloud distributed standard
library.

Its single concern: **sequences: nth Fibonacci number (0-indexed).** It owns the
*control flow* — an iterative fold over the sequence — but does no arithmetic of
its own. Each successive term is produced by asking
[`srvcs-add`](https://github.com/srvcs/add) for `a + b`.

```
fibonacci(n):
    a, b = 0, 1
    repeat n times:
        next = add(a, b)
        a, b = b, next
    return a
```

`fibonacci(0) == 0`, `fibonacci(1) == 1`, `fibonacci(10) == 55`. A negative index
is rejected with `422` before any dependency is called.

## API

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/` | Service identity, concern, and dependency list |
| `POST` | `/` | Compute the `n`th Fibonacci number |
| `GET` | `/healthz` `/readyz` `/metrics` `/openapi.json` | srvcs service standard surface |

```sh
curl -s -X POST localhost:8080/ -H 'content-type: application/json' -d '{"value": 10}'
# {"value":10,"result":55}
```

Responses:

- `200 {"value": n, "result": fib}` — evaluated.
- `422 {"error": "n must be >= 0"}` — `n` is negative; also forwarded verbatim if
  a dependency rejects an operand.
- `500` — the loop exceeded its iteration cap (a defensive guard).
- `503` — a dependency is unavailable.

## Dependencies

- [`srvcs-add`](https://github.com/srvcs/add)

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `SRVCS_BIND_ADDR` | `0.0.0.0:8080` | Bind address |
| `SRVCS_ADD_URL` | `http://127.0.0.1:8081` | Base URL of `srvcs-add` |
| `SRVCS_ENV` | `development` | Environment label for logs |
| `RUST_LOG` | `info,tower_http=info` | Tracing filter |

## Local checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Orchestration tests stand up a *computing* mock `srvcs-add` in-process — it reads
the request body and returns the real `a + b`, so the fold is genuinely exercised
against the asserted Fibonacci values. See
[`srvcs/platform`](https://github.com/srvcs/platform) for the shared standard.

> Note: the `cargoHash` in `flake.nix` is inherited from the template and must be
> refreshed with a `nix build` before the Nix gates pass.
