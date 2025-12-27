# Gemini API Proxy

This is a streaming HTTP proxy component that forwards requests to Google's Gemini API and streams the response back to clients in real-time using Server-Sent Events (SSE).

## Features

- **Real-time streaming**: Streams Gemini API responses chunk-by-chunk as they arrive
- **SSE support**: Returns responses as `text/event-stream` for easy client consumption
- **WASI HTTP**: Built using the `wasi:http` interface for portability across WebAssembly runtimes

## Prerequisites

- `cargo` 1.82+
- [`wash`](https://wasmcloud.com/docs/installation) 0.36.1+
- `wasmtime` >=25.0.0 (if running with wasmtime standalone)

## Configuration

Before building, update the Gemini API key in `src/lib.rs`:

```rust
const API_KEY = "YOUR_API_KEY_HERE";  // Replace with your actual Gemini API key
```

Get your API key from [Google AI Studio](https://aistudio.google.com/app/apikey).

## Building

```bash
wash build
```

## Running with wasmtime

You must have wasmtime >=25.0.0 for this to work. Make sure to follow the build step above first.

```bash
wasmtime serve -Scommon ./build/http_ai_proxy_s.wasm
```

Then send a POST request with your prompt:

```bash
curl -X POST http://127.0.0.1:8080/gemini-proxy \
  -H "Content-Type: text/plain" \
  -d "Explain how AI works"
```

## Running with wasmCloud

please refer to `crates/wash-runtime/tests/integration_http_streaming_gemini.rs` for reference.

## API Endpoints

### `POST /gemini-proxy`

Proxies your prompt to the Gemini API and streams the response back.

**Request:**
- Method: `POST`
- Content-Type: `text/plain`
- Body: Your prompt text

**Response:**
- Content-Type: `text/event-stream`
- Body: Streamed SSE chunks from Gemini API

**Example:**

```bash
# Stream a response in real-time
curl -X POST http://127.0.0.1:8080/gemini-proxy \
  -H "Content-Type: text/plain" \
  -d "Write a haiku about WebAssembly" \
```

## How It Works

1. Client sends a POST request with a text prompt
2. Component reads the full prompt from the request body
3. Component forwards the request to Gemini's streaming API endpoint
4. As Gemini generates the response, chunks arrive in real-time
5. Component streams each chunk back to the client immediately
6. Client sees the response appear progressively (like ChatGPT)

## Technical Details

This component demonstrates:
- **Streaming HTTP responses** using WASI HTTP outgoing-handler
- **Proper resource management** - the Wasmtime Store stays alive during streaming
- **Async I/O** with a custom executor for WebAssembly
- **SSE formatting** for browser-compatible streaming

The key to making streaming work is ensuring the component's execution context (Store) remains alive while chunks are being written to the response body, rather than being dropped after the headers are sent.

## Troubleshooting

**Only receiving one chunk?**
- Ensure your HTTP server plugin keeps the Wasmtime Store alive for the duration of streaming
- The Store must not be dropped until the component finishes writing all chunks

**API errors?**
- Verify your API key is valid and has quota remaining
- Check that `generativelanguage.googleapis.com` is in your allowed hosts list

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.