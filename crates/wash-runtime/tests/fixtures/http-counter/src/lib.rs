use anyhow::{Context, Result};

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::{
        blobstore::{
            blobstore::{create_container, get_container},
            types::OutgoingValue,
        },
        config::store::get_all,
        http::{
            outgoing_handler::{OutgoingRequest, RequestOptions, handle},
            types::{
                Fields, IncomingRequest, Method, OutgoingBody, OutgoingResponse, ResponseOutparam,
                Scheme,
            },
        },
        keyvalue::{atomics::increment, store::open},
        logging::logging::{Level, log},
    },
};

struct Component;

const CONTAINER_NAME: &str = "http-responses";
const OBJECT_KEY: &str = "example-com-response";
const COUNTER_KEY: &str = "request-count";

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        match handle_request() {
            Ok(count) => {
                log(
                    Level::Info,
                    "",
                    &format!("Successfully processed request, count: {count}"),
                );
                let response = OutgoingResponse::new(Fields::new());
                response.set_status_code(200).unwrap();
                let body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));

                let stream = body.write().unwrap();
                stream.blocking_write_and_flush(count.as_bytes()).unwrap();
                drop(stream);
                OutgoingBody::finish(body, None).unwrap();
            }
            Err(e) => {
                log(Level::Error, "", &format!("Error processing request: {e}"));
                let response = OutgoingResponse::new(Fields::new());
                response.set_status_code(500).unwrap();
                let body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));

                let error_msg = format!("Internal server error: {e}");
                let stream = body.write().unwrap();
                stream
                    .blocking_write_and_flush(error_msg.as_bytes())
                    .unwrap();
                drop(stream);
                OutgoingBody::finish(body, None).unwrap();
            }
        }
    }
}

fn handle_request() -> Result<String> {
    log(Level::Info, "", "Starting HTTP counter request processing");

    log_runtime_config()?;

    let response_body = make_outgoing_request()?;

    store_response_in_blobstore(&response_body)?;

    let count = increment_counter()?;

    Ok(count.to_string())
}

fn log_runtime_config() -> Result<()> {
    log(Level::Info, "", "Retrieving runtime configuration");
    match get_all() {
        Ok(config) => {
            log(Level::Info, "", "Runtime configuration keys available:");
            for (key, value) in config.iter() {
                log(Level::Info, "", &format!("Config key: {key} = {value}"));
            }
            if config.is_empty() {
                log(Level::Info, "", "No runtime configuration keys found");
            }
        }
        Err(e) => {
            log(
                Level::Warn,
                "",
                &format!("Failed to retrieve runtime configuration: {e:?}"),
            );
        }
    }
    Ok(())
}

fn make_outgoing_request() -> Result<String> {
    log(
        Level::Info,
        "",
        "Making outgoing HTTP request to http://example.com",
    );

    let request = OutgoingRequest::new(Fields::new());
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|_| anyhow::anyhow!("Failed to set HTTP scheme"))?;
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
    log(
        Level::Info,
        "",
        &format!("Received response with status: {status}"),
    );

    if status < 200 || status >= 300 {
        log(
            Level::Warn,
            "",
            &format!("Non-success status code received: {status}"),
        );
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
    log(
        Level::Info,
        "",
        &format!("Retrieved {} bytes from example.com", body_data.len()),
    );

    Ok(body_string)
}

fn store_response_in_blobstore(response_body: &str) -> Result<()> {
    log(
        Level::Info,
        "",
        &format!("Storing response in blobstore container: {CONTAINER_NAME}"),
    );

    let container = match get_container(CONTAINER_NAME) {
        Ok(container) => {
            log(
                Level::Info,
                "",
                &format!("Using existing container: {CONTAINER_NAME}"),
            );
            container
        }
        Err(_) => {
            log(
                Level::Info,
                "",
                &format!("Creating new container: {CONTAINER_NAME}"),
            );
            create_container(CONTAINER_NAME)
                .map_err(|e| anyhow::anyhow!("Failed to create blobstore container: {e}"))?
        }
    };

    let response_bytes = response_body.as_bytes();

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

    OutgoingValue::finish(outgoing_value).ok();

    log(
        Level::Info,
        "",
        &format!(
            "Successfully stored {} bytes in object: {OBJECT_KEY}",
            response_bytes.len()
        ),
    );

    Ok(())
}

fn increment_counter() -> Result<u64> {
    log(Level::Info, "", "Incrementing request counter");

    let bucket = open("")?;

    let new_count = increment(&bucket, COUNTER_KEY, 1).context("Failed to increment counter")?;

    log(
        Level::Info,
        "",
        &format!("Request count incremented to: {new_count}"),
    );

    Ok(new_count)
}

bindings::export!(Component with_types_in bindings);
