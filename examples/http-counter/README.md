# HTTP Counter Component

An HTTP service component that demonstrates WebAssembly Component Model integration with multiple WASI interfaces.

## Prerequisites

- `wash` CLI tool must be installed

## Features

This component implements an HTTP counter service that:

1. Uses `wasmcloud_component` logging macros for structured logging at appropriate levels
2. Logs all available runtime configuration keys at info level during initialization
3. Makes HTTP requests to `https://example.com` using `wasi:http/outgoing-handler`
4. Stores the HTTP response body in a blob container using `wasi:blobstore`
5. Maintains a request counter using `wasi:keyvalue`
6. Returns the current count as an HTTP response
7. Returns HTTP 500 status with error payload if any operation fails

## WASI Interfaces

- `wasi:http/incoming-handler` - Handles incoming HTTP requests
- `wasi:http/outgoing-handler` - Makes outbound HTTP requests
- `wasi:blobstore` - Persistent blob storage for response data
- `wasi:keyvalue` - Key-value store for request counting
- `wasi:config` - Runtime configuration access

## Building

```bash
wash build
```

## Running

```bash
wash dev
```

## Response Format

The component returns the request count as a plain text HTTP response. If any operation fails during request processing, it returns HTTP 500 with an error message.
