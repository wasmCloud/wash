# Slow HTTP Component

A test component that intentionally delays its HTTP response by 2 seconds.

Used for testing:
- Graceful drain during component updates (in-flight requests complete before restart)
- Request queueing during component reconciliation

## Building

```bash
cargo component build --release
cp target/wasm32-wasip2/release/slow_http_component.wasm ../crates/wash-runtime/tests/fixtures/
```

## Endpoints

- `/slow` or `/` - Sleeps for 2 seconds, then returns "SLOW RESPONSE COMPLETED"
- `/fast` - Returns immediately with "FAST RESPONSE"
