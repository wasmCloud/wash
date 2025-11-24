use std::time::{Duration, Instant};

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::http::types::{
        Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam, Method,
    },
};

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        handle_request(request, response_out);
    }
}

fn handle_request(request: IncomingRequest, response_out: ResponseOutparam) {
    match (request.method(), request.path_with_query().as_deref()) {
        (Method::Post, Some("/stream-random")) => {
            stream_random_chunks(response_out);
        }
        _ => {
            eprintln!("[COMPONENT] No route matched, returning 405");
            method_not_allowed(response_out);
        }
    }
}

fn stream_random_chunks(response_out: ResponseOutparam) {
    // Create response with streaming headers
    let response = OutgoingResponse::new(
        Fields::from_list(&[
            ("content-type".to_string(), b"text/event-stream".to_vec()),
            ("cache-control".to_string(), b"no-cache".to_vec()),
        ])
        .unwrap(),
    );

    let body = response.body().expect("response should be writable");
    ResponseOutparam::set(response_out, Ok(response));

    let output_stream = body.write().expect("body should be writable");

    // Stream 20 chunks of random bytes, one every 0.5 seconds
    const TOTAL_CHUNKS: usize = 20;
    const CHUNK_SIZE: usize = 32;
    const INTERVAL_MS: u64 = 500;

    let mut last_time = Instant::now();

    for i in 0..TOTAL_CHUNKS {
        // Wait for the interval
        loop {
            let elapsed = last_time.elapsed();
            if elapsed >= Duration::from_millis(INTERVAL_MS) {
                last_time = Instant::now();
                break;
            }
            // Simple busy wait (in a real implementation, you might want to use pollable)
            std::thread::sleep(Duration::from_millis(10));
        }

        // Generate random bytes
        let random_bytes: Vec<u8> = (0..CHUNK_SIZE)
            .map(|_| generate_random_byte())
            .collect();

        // Format as hex for readability
        let hex_string = format!(
            "data: chunk {} - {}\n\n",
            i + 1,
            random_bytes
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join("")
        );

        // Write the chunk
        let bytes = hex_string.as_bytes();
        let mut offset = 0;

        while offset < bytes.len() {
            match output_stream.check_write() {
                Ok(n) if n > 0 => {
                    let to_write = std::cmp::min(n as usize, bytes.len() - offset);
                    match output_stream.write(&bytes[offset..offset + to_write]) {
                        Ok(()) => {
                            offset += to_write;
                        }
                        Err(_) => {
                            eprintln!("[COMPONENT] Write failed");
                            return;
                        }
                    }
                }
                Ok(_) => {
                    // Buffer full, wait a bit
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    eprintln!("[COMPONENT] Check write failed");
                    return;
                }
            }
        }

        // Flush after each chunk
        if let Err(_) = output_stream.flush() {
            eprintln!("[COMPONENT] Flush failed");
            return;
        }
    }

    // Finish the body
    drop(output_stream);
    OutgoingBody::finish(body, None).expect("outgoing-body.finish");
}

// Simple pseudo-random number generator (LCG)
fn generate_random_byte() -> u8 {
    use std::cell::Cell;
    thread_local! {
        static SEED: Cell<u64> = Cell::new(12345);
    }

    SEED.with(|seed| {
        let current = seed.get();
        let next = current.wrapping_mul(1103515245).wrapping_add(12345);
        seed.set(next);
        (next >> 16) as u8
    })
}

fn method_not_allowed(response_out: ResponseOutparam) {
    respond(405, response_out)
}

fn respond(status: u16, response_out: ResponseOutparam) {
    let response = OutgoingResponse::new(Fields::new());
    response
        .set_status_code(status)
        .expect("setting status code");

    let body = response.body().expect("response should be writable");
    ResponseOutparam::set(response_out, Ok(response));
    OutgoingBody::finish(body, None).expect("outgoing-body.finish");
}

bindings::export!(Component with_types_in bindings);