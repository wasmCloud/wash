//! A test component that intentionally delays its response.
//! Used for testing graceful drain during component updates.

use wstd::http::{Body, Request, Response};
use wstd::time::Duration;

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path_and_query().unwrap().as_str() {
        "/slow" | "/" => slow_handler(req).await,
        "/fast" => fast_handler(req).await,
        _ => slow_handler(req).await,
    }
}

/// Handler that sleeps for SLEEP_DURATION before responding.
/// Used to simulate a long-running request for drain testing.
async fn slow_handler(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    // Sleep for 2 seconds to simulate a long-running operation
    wstd::task::sleep(Duration::from_secs(2)).await;

    Ok(Response::new("SLOW RESPONSE COMPLETED\n".into()))
}

/// Fast handler that responds immediately (for comparison/baseline)
async fn fast_handler(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Ok(Response::new("FAST RESPONSE\n".into()))
}
