use anyhow::{Context, Result};
use wasmcloud_component::http::{self, StatusCode};
use wasmcloud_component::{error, info, warn};

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::wasi;

use wasi::blobstore::blobstore::{create_container, get_container};
use wasi::config::runtime::get_all;
use wasi::http::outgoing_handler::{OutgoingRequest, RequestOptions, handle};
use wasi::http::types::{Fields, Method, Scheme};
use wasi::keyvalue::atomics::increment;
use wasi::keyvalue::store::open;

struct Component;

http::export!(Component);

const CONTAINER_NAME: &str = "http-responses";
const OBJECT_KEY: &str = "example-com-response";
const COUNTER_KEY: &str = "request-count";

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        match handle_request() {
            Ok(count) => {
                info!("Successfully processed request, count: {count}");
                Ok(http::Response::new(count))
            }
            Err(e) => {
                error!("Error processing request: {e}");
                let error_msg = format!("Internal server error: {e}");
                let mut response = http::Response::new(error_msg);
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                Ok(response)
            }
        }
    }
}

fn handle_request() -> Result<String> {
    info!("Starting HTTP counter request processing");

    log_runtime_config()?;

    let response_body = make_outgoing_request()?;

    store_response_in_blobstore(&response_body)?;

    let count = increment_counter()?;

    Ok(count.to_string())
}

fn log_runtime_config() -> Result<()> {
    info!("Retrieving runtime configuration");
    match get_all() {
        Ok(config) => {
            info!("Runtime configuration keys available:");
            for (key, value) in config.iter() {
                info!("Config key: {key} = {value}");
            }
            if config.is_empty() {
                info!("No runtime configuration keys found");
            }
        }
        Err(e) => {
            warn!("Failed to retrieve runtime configuration: {e:?}");
        }
    }
    Ok(())
}

fn make_outgoing_request() -> Result<String> {
    info!("Making outgoing HTTP request to https://example.com");

    let request = OutgoingRequest::new(Fields::new());
    request
        .set_scheme(Some(&Scheme::Https))
        .map_err(|_| anyhow::anyhow!("Failed to set HTTPS scheme"))?;
    request
        .set_authority(Some("example.com"))
        .map_err(|_| anyhow::anyhow!("Failed to set authority"))?;
    request
        .set_path_with_query(Some("/"))
        .map_err(|_| anyhow::anyhow!("Failed to set path"))?;
    request
        .set_method(&Method::Get)
        .map_err(|_| anyhow::anyhow!("Failed to set GET method"))?;

    let options = RequestOptions::new();

    let future_response =
        handle(request, Some(options)).context("Failed to initiate outgoing request")?;

    future_response.subscribe().block();

    let incoming_response = future_response
        .get()
        .context("Failed to get response from future")?
        .map_err(|e| anyhow::anyhow!("Request failed: {e:?}"))??;

    let status = incoming_response.status();
    info!("Received response with status: {status}");

    if status < 200 || status >= 300 {
        warn!("Non-success status code received: {status}");
    }

    let body_stream = incoming_response
        .consume()
        .map_err(|_| anyhow::anyhow!("Failed to consume response body"))?;

    let input_stream = body_stream
        .stream()
        .map_err(|_| anyhow::anyhow!("Failed to get input stream"))?;

    let mut body_data = Vec::new();
    loop {
        match input_stream.read(8192) {
            Ok(chunk) => {
                if chunk.is_empty() {
                    break;
                }
                body_data.extend_from_slice(&chunk);
            }
            Err(_) => break,
        }
    }

    let body_string = String::from_utf8_lossy(&body_data).to_string();
    info!("Retrieved {} bytes from example.com", body_data.len());

    Ok(body_string)
}

fn store_response_in_blobstore(response_body: &str) -> Result<()> {
    info!("Storing response in blobstore container: {CONTAINER_NAME}");

    let container = match get_container(CONTAINER_NAME) {
        Ok(container) => {
            info!("Using existing container: {CONTAINER_NAME}");
            container
        }
        Err(_) => {
            info!("Creating new container: {CONTAINER_NAME}");
            create_container(CONTAINER_NAME)
                .map_err(|e| anyhow::anyhow!("Failed to create blobstore container: {e}"))?
        }
    };

    let response_bytes = response_body.as_bytes();

    // Use write_data method from container resource
    use wasi::blobstore::types::OutgoingValue;
    let outgoing_value = OutgoingValue::new_outgoing_value();
    let output_stream = outgoing_value
        .outgoing_value_write_body()
        .map_err(|_| anyhow::anyhow!("Failed to get output stream"))?;

    output_stream
        .blocking_write_and_flush(response_bytes)
        .context("Failed to write data")?;
    drop(output_stream);

    container
        .write_data(OBJECT_KEY, &outgoing_value)
        .map_err(|e| anyhow::anyhow!("Failed to store response in blobstore: {e}"))?;

    OutgoingValue::finish(outgoing_value);

    info!(
        "Successfully stored {} bytes in object: {OBJECT_KEY}",
        response_bytes.len()
    );

    Ok(())
}

fn increment_counter() -> Result<u64> {
    info!("Incrementing request counter");

    let bucket = open("")?;

    let new_count = increment(&bucket, COUNTER_KEY, 1).context("Failed to increment counter")?;

    info!("Request count incremented to: {new_count}");

    Ok(new_count)
}
