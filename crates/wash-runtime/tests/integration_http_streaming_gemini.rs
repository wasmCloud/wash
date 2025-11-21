//! Integration test for streaming Gemini proxy component

use anyhow::Result;
use futures::stream;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, StreamBody};
use hyper::{
    StatusCode,
    body::{Bytes, Frame},
    client::conn::http1,
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

mod common;
use common::find_available_port;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_logging::WasiLogging,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_STREAMING_GEMINI_WASM: &[u8] = include_bytes!("./fixtures/http_ai_proxy.wasm");

#[test_log::test(tokio::test)]
async fn wasi_http_gemini_proxy() -> Result<()> {
    println!("\n🚀 STREAMING GEMINI PROXY TEST\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_handler = DevRouter::default();
    let http_plugin = HttpServer::new(http_handler, addr);

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(WasiLogging {}))?
        .build()?
        .start()
        .await?;

    println!("✓ Host started on {addr}");

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "gemini-proxy".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: bytes::Bytes::from_static(HTTP_STREAMING_GEMINI_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 512,
                    cpu_limit: 2,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: vec!["generativelanguage.googleapis.com".to_string()],
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "gemini-proxy".to_string());
                        config
                    },
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "logging".to_string(),
                    interfaces: ["logging".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
                    config: HashMap::new(),
                },
            ],
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;
    println!("✓ Workload deployed\n");

    let prompt = "Explain how AI works";
    let body_stream = stream::iter([Ok::<_, hyper::Error>(Frame::data(Bytes::from(prompt)))]);

    // Create HTTP client first
    let stream = tokio::net::TcpStream::connect(addr).await?;
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(false)
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(err) = conn.await {
            eprintln!("Connection error: {err:?}");
        }
    });

    // Build request with relative URI but explicit Host header
    let mut request = hyper::Request::builder()
        .method(hyper::Method::POST)
        .uri("/gemini-proxy")
        .header("content-type", "text/plain")
        .body(BoxBody::new(StreamBody::new(body_stream)))?;

    request.headers_mut().insert(
        hyper::header::HOST,
        hyper::header::HeaderValue::from_static("gemini-proxy"),
    );

    let response = sender.send_request(request).await?;

    assert_eq!(StatusCode::OK, response.status());

    println!("\nResponse status: {:?}", response.status());
    println!("Response headers: {:?}", response.headers());
    println!("\n=== Streaming Response ===");

    // Track streaming metrics
    let (_parts, body) = response.into_parts();
    let mut body_stream = body;
    let start_time = std::time::Instant::now();
    let mut chunk_count = 0;
    let mut total_bytes = 0;
    let mut response_text = String::new();

    while let Some(frame) = body_stream.frame().await {
        match frame {
            Ok(frame) => {
                if let Some(chunk) = frame.data_ref() {
                    chunk_count += 1;
                    total_bytes += chunk.len();

                    let elapsed = start_time.elapsed();
                    println!(
                        "[{:.7}s] Chunk #{} received ({} bytes)",
                        elapsed.as_secs_f64(),
                        chunk_count,
                        chunk.len()
                    );

                    if let Ok(text) = std::str::from_utf8(chunk) {
                        response_text.push_str(text);
                        print!("{}", text);
                        use std::io::Write;
                        std::io::stdout().flush().unwrap();
                    }
                }
            }
            Err(e) => {
                eprintln!("\nError reading frame: {e}");
                break;
            }
        }
    }

    let total_time = start_time.elapsed();
    println!(
        "\n=== End (Total: {:.7}s, {} chunks, {} bytes) ===\n",
        total_time.as_secs_f64(),
        chunk_count,
        total_bytes
    );

    // Assertions to verify streaming behavior
    assert!(
        chunk_count >= 10,
        "Expected at least 10 chunks for streaming response, got {}",
        chunk_count
    );

    assert!(
        total_bytes > 100,
        "Expected substantial response content, got {} bytes",
        total_bytes
    );

    assert!(
        !response_text.is_empty(),
        "Expected non-empty response text"
    );

    // Verify that streaming actually took time (not instant)
    assert!(
        total_time.as_millis() > 100,
        "Response came too fast ({:?}ms), may not be streaming",
        total_time.as_millis()
    );

    println!("✓ All streaming assertions passed!");

    Ok(())
}
