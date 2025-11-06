use wasmcloud_component::http;
use wasmcloud_component::wasi::http::outgoing_handler;

struct Component;

http::export!(Component);

impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let path = request.uri().path();
        
        // If path is /outbound, make an outbound request
        if path == "/outbound" {
            match make_outbound_request() {
                Ok(body) => Ok(http::Response::new(body)),
                Err(e) => {
                    let mut resp = http::Response::new(format!("Error: {e}"));
                    *resp.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
                    Ok(resp)
                }
            }
        } else {
            Ok(http::Response::new("Hello from Rust!\n".to_string()))
        }
    }
}

fn make_outbound_request() -> Result<String, String> {
    use outgoing_handler::{OutgoingRequest, RequestOptions, handle};
    use wasmcloud_component::wasi::http::types::{Fields, Method, Scheme};
    
    // Create request to example.com
    let req = OutgoingRequest::new(Fields::new());
    req.set_scheme(Some(&Scheme::Https))
        .map_err(|_| "Failed to set scheme".to_string())?;
    req.set_authority(Some("example.com"))
        .map_err(|_| "Failed to set authority".to_string())?;
    req.set_path_with_query(Some("/"))
        .map_err(|_| "Failed to set path".to_string())?;
    req.set_method(&Method::Get)
        .map_err(|_| "Failed to set method".to_string())?;
    
    // Make the request
    let future = handle(req, Some(RequestOptions::new()))
        .map_err(|e| format!("Request failed: {e:?}"))?;
    
    // Wait for response
    future.subscribe().block();
    
    let response = future
        .get()
        .ok_or("No response".to_string())?
        .map_err(|e| format!("Response error: {e:?}"))?
        .map_err(|e| format!("HTTP error: {e:?}"))?;
    
    let status = response.status();
    
    // Read body
    let body_stream = response
        .consume()
        .map_err(|_| "Failed to consume body".to_string())?;
    
    let stream = body_stream
        .stream()
        .map_err(|_| "Failed to get stream".to_string())?;
    
    let mut data = Vec::new();
    loop {
        match stream.read(8192) {
            Ok(chunk) if !chunk.is_empty() => data.extend_from_slice(&chunk),
            _ => break,
        }
    }
    
    let body_len = data.len();
    let preview = if body_len > 100 {
        format!("{}... ({} bytes)", String::from_utf8_lossy(&data[..100]), body_len)
    } else {
        String::from_utf8_lossy(&data).to_string()
    };
    
    Ok(format!("✓ Outbound HTTPS request successful!\nStatus: {}\nBody: {}", status, preview))
}
