# redis-keyvalue

A WebAssembly HTTP server component that stores and retrieves key-value pairs using a Redis backend via the `wasi:keyvalue` interface.

## What it does

- `POST /` — accepts a JSON body `{"key":"...","value":"..."}` and stores the pair in Redis
- `GET /?key=<key>` — returns the stored value, or `404` if the key does not exist

The component itself has no knowledge of Redis. It speaks only the `wasi:keyvalue/store` interface; the host runtime wires that interface to Redis using the `wasi_keyvalue_redis_url` setting in `.wash/config.yaml`.

## Prerequisites

- Rust with the `wasm32-wasip2` target: `rustup target add wasm32-wasip2`
- A running Redis server (default: `127.0.0.1:6379`)
- The `wash` CLI

## Starting Redis locally

If you don't have a Redis server running, you can start one with Docker:

```bash
docker run --name redis -p 6379:6379 redis:latest
```

## Configuration

The Redis connection URL is set in [.wash/config.yaml](.wash/config.yaml):

```yaml
dev:
  wasi_keyvalue_redis_url: redis://127.0.0.1:6379
```

Change the URL to point at a different Redis instance if needed.

## Running

```bash
wash dev
```

This builds the component and starts an HTTP server on `http://localhost:8000`.

## Usage

Store a value:

```bash
curl -X POST http://localhost:8000 \
  -H "Content-Type: application/json" \
  -d '{"key":"mykey","value":"myvalue"}'
```

Retrieve a value:

```bash
curl "http://localhost:8000?key=mykey"
```

## Building manually

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled component is written to `target/wasm32-wasip2/release/redis_keyvalue.wasm`.
