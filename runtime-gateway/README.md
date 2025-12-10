# runtime-gateway

Distributes HTTP Requests to Wasm Hosts.

## How it works?

It watches both `Host` and `Workload` CRDs, creating an internal mapping of which workloads are associated with which hosts.

When an HTTP request is received, the gateway determines which host to forward the request to based on the request's hostname and the registered workloads. It then forwards the request to the appropriate host, which processes the request and returns a response.

It also enriches the request with additional headers with:

- `X-Real-Ip`: The canonical IP address of the client.
- `X-Workload-Id`: The unique identifier of the workload handling the request.
