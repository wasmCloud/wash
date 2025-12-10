//! Integration test for HTTP streaming component
//!
//! This test demonstrates:
//! 1. Starting a host with HTTP and logging plugins
//! 2. Creating and starting a workload that streams data at fixed intervals
//! 3. Verifying that exactly 20 chunks are received
//! 4. Validating total streaming duration is approximately 10 seconds
//! 5. Asserting each chunk arrives at ~0.5 second intervals

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
// The component we built just now
const HTTP_STREAMING_WASM: &[u8] = include_bytes!("./fixtures/http_streaming.wasm");

#[test_log::test(tokio::test)]
async fn wasi_http_stream_random_chunks() -> Result<()> {
    println!("\n🚀 STREAMING RANDOM CHUNKS TEST\n");

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
            name: "random-streamer".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: bytes::Bytes::from_static(HTTP_STREAMING_WASM),
                local_resources: LocalResources::default(),
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
                        config.insert("host".to_string(), "random-streamer".to_string());
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

    let body_stream = stream::iter([Ok::<_, hyper::Error>(Frame::data(Bytes::from("")))]);

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
        .uri("/stream-random")
        .header("content-type", "text/plain")
        .body(BoxBody::new(StreamBody::new(body_stream)))?;

    request.headers_mut().insert(
        hyper::header::HOST,
        hyper::header::HeaderValue::from_static("random-streamer"),
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
    let mut chunk_timestamps = Vec::new();

    while let Some(frame) = body_stream.frame().await {
        match frame {
            Ok(frame) => {
                if let Some(chunk) = frame.data_ref() {
                    let elapsed = start_time.elapsed();
                    chunk_timestamps.push(elapsed);
                    chunk_count += 1;
                    total_bytes += chunk.len();

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

    // Assert exactly 20 chunks
    assert_eq!(
        chunk_count, 20,
        "Expected exactly 20 chunks, got {}",
        chunk_count
    );

    // Assert total time is approximately 10 seconds (20 chunks * 0.5s = 10s)
    // Allow some variance for timing precision (9-11 seconds)
    assert!(
        total_time.as_secs_f64() >= 9.0 && total_time.as_secs_f64() <= 11.0,
        "Expected total time ~10 seconds (9-11s range), got {:.2}s",
        total_time.as_secs_f64()
    );

    // Assert chunks arrived at approximately 0.5 second intervals
    for i in 1..chunk_timestamps.len() {
        let interval = (chunk_timestamps[i] - chunk_timestamps[i - 1]).as_secs_f64();
        assert!(
            interval >= 0.4 && interval <= 0.6,
            "Chunk {} interval was {:.3}s, expected ~0.5s (0.4-0.6s range)",
            i + 1,
            interval
        );
    }

    // Verify response is not empty
    assert!(
        !response_text.is_empty(),
        "Expected non-empty response text"
    );

    println!("✓ All streaming assertions passed!");
    println!("  - Received exactly 20 chunks");
    println!(
        "  - Total time: {:.2}s (~10s expected)",
        total_time.as_secs_f64()
    );
    println!(
        "  - Average interval: {:.3}s (~0.5s expected)",
        total_time.as_secs_f64() / 20.0
    );

    Ok(())
}
