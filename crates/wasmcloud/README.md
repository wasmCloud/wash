# wasmcloud

[![Stars](https://img.shields.io/github/stars/wasmcloud?color=gold&label=wasmCloud%20Org%20Stars)](https://github.com/wasmcloud/)
[![Homepage and Documentation](https://img.shields.io/website?label=Documentation&url=https%3A%2F%2Fwasmcloud.com)](https://wasmcloud.com)
[![CNCF Incubating project](https://img.shields.io/website?label=CNCF%20Incubating%20Project&url=https://landscape.cncf.io/?selected=wasm-cloud&item=orchestration-management--scheduling-orchestration--wasmcloud)](https://landscape.cncf.io/?selected=wasm-cloud&item=orchestration-management--scheduling-orchestration--wasmcloud)
![Powered by WebAssembly](https://img.shields.io/badge/powered%20by-WebAssembly-orange.svg)
[![OpenSSF Best Practices](https://www.bestpractices.dev/projects/6363/badge)](https://www.bestpractices.dev/projects/6363)
[![OpenSSF Scorecard](https://api.securityscorecards.dev/projects/github.com/wasmCloud/wasmCloud/badge)](https://securityscorecards.dev/viewer/?uri=github.com/wasmCloud/wasmCloud)
[![CLOMonitor](https://img.shields.io/endpoint?url=https://clomonitor.io/api/projects/cncf/wasm-cloud/badge)](https://clomonitor.io/projects/cncf/wasm-cloud)
[![FOSSA Status](https://app.fossa.com/api/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git.svg?type=small)](https://app.fossa.com/projects/custom%2B40030%2Fgit%40github.com%3AwasmCloud%2FwasmCloud.git?ref=badge_small)
[![twitter](https://img.shields.io/twitter/follow/wasmcloud?style=social)](https://twitter.com/wasmcloud)
[![youtube subscribers](https://img.shields.io/youtube/channel/subscribers/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)
[![youtube views](https://img.shields.io/youtube/channel/views/UCmZVIWGxkudizD1Z1and5JA?style=social)](https://youtube.com/wasmcloud)
![wasmCloud logo](https://raw.githubusercontent.com/wasmCloud/branding/main/02.Horizontal%20Version/Pixel/PNG/Wasmcloud.Logo-Hrztl_Color.png)
![wasmCloud contributors](https://markupgo.com/github/wasmCloud/wasmCloud/contributors?count=0&circleSize=40&circleRadius=40&center=true)

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

wasmCloud offers faster development cycles with reusable, polyglot components and centrally maintainable apps, allowing platform teams to manage thousands of diverse applications. It integrates seamlessly with existing stacks like Kubernetes and cloud providers, while providing portability across different operating systems and architectures without new builds. With custom capabilities, scale-to-zero, fault-tolerant features, and deployment across clouds, wasmCloud enables secure, reliable, and scalable applications without vendor lock-in.

## Getting Started

### Running wasmCloud

The primary target for consuming wasmCloud is as a binary that you run on your machine, in the cloud, in a container, on the edge, etc.

### Embedding wasmCloud

This crate is embeddable in Rust projects and can be extended with your own host plugins or functionality.

### Usage

```rust
use std::sync::Arc;
use std::collections::HashMap;

use wasmcloud::{
    engine::Engine,
    host::{HostBuilder, HostApi},
    plugin::{
        wasi_config::RuntimeConfig,
        wasi_http::HttpServer,
    },
    types::{WorkloadStartRequest, Workload},
};

#[tokio::main]
async fn main()  -> anyhow::Result<()> {
    let engine = Engine::builder().build()?;
    let http_plugin = HttpServer::new("127.0.0.1:8080".parse()?);
    let runtime_config_plugin = RuntimeConfig::default();

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(http_plugin))?
        .with_plugin(Arc::new(runtime_config_plugin))?
        .build()?;

    let host = host.start().await?;

    let req = WorkloadStartRequest {
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };
    let res = host.workload_start(req).await;

    assert!(res.is_ok());

    Ok(())
}
```

_We are a Cloud Native Computing Foundation [Incubating project](https://www.cncf.io/projects/)._
