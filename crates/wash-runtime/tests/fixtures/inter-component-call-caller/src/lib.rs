mod bindings {
    wit_bindgen::generate!({
        world: "component",
        generate_all
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::http::types::{
        Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
    },
};

use crate::bindings::wasmcloud::example::middleware;

struct Component;

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        match middleware::invoke() {
            Ok(_) => {
                let response = OutgoingResponse::new(Fields::new());
                response.set_status_code(200).unwrap();
                let body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));

                let stream = body.write().unwrap();
                stream.blocking_write_and_flush("".as_bytes()).unwrap();
                drop(stream);
                OutgoingBody::finish(body, None).unwrap();
            }
            Err(e) => {
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

bindings::export!(Component with_types_in bindings);
