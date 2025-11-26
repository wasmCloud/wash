# Runtime Operator

## Development

You will need a local Kind cluster, NATS, and a Wash host.

For NATS, use:

```sh
docker run --rm --name wasmcloud-nats -p 4222:4222 nats -js
```

For Wash Host, use:

```sh
wash host --http-addr 127.0.0.1:8000
```

or from the top git directory:

```sh
cargo run -- host --http-addr 127.0.0.1:8000
```

For local kind cluster, refer to [wash/README.md](../README.md).

Install the CRDs into the cluster:

```sh
make install
```

Run the Builder / Watcher:

```sh
make devlog
```

You can apply development samples from the config/sample:

```sh
kubectl apply -f config/samples
```

Verify the Wash Host registered correctly with:

```sh
kubectl get host
```

Example:

```
❯ kubectl get host
NAME               HOSTID                                 HOSTGROUP   READY   AGE
exotic-toes-5866   14b0a1df-3f91-441c-b304-2aa56e9bef6e   default     True    52s
```

If you applied the samples, check if they deployed correctly:

```
❯ kubectl get workloaddeployment
NAME     REPLICAS   READY
blobby   1          True
hello    1          True

❯ curl -i hello.localhost.direct:8000
HTTP/1.1 200 OK
transfer-encoding: chunked
date: Tue, 25 Nov 2025 15:30:32 GMT

Hello from Rust!
```
