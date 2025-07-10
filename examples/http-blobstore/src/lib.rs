use wasmcloud_component::{
    http, info,
    wasi::blobstore::{
        container::IncomingValue,
        types::{InputStream, OutgoingValue},
    },
};

struct Component;

enum ResponseBody {
    String(String),
    Stream(InputStream),
}

impl http::OutgoingBody for ResponseBody {
    fn write(
        self,
        body: wasmcloud_component::wasi::http::types::OutgoingBody,
        stream: wasmcloud_component::wasi::io::streams::OutputStream,
    ) -> std::io::Result<()> {
        match self {
            ResponseBody::String(s) => s.write(body, stream),
            ResponseBody::Stream(data) => InputStream::write(data, body, stream),
        }
    }
}

impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let container = match wasmcloud_component::wasi::blobstore::blobstore::get_container(
            &String::from("my-container-real"),
        ) {
            Ok(c) => c,
            Err(_) => match wasmcloud_component::wasi::blobstore::blobstore::create_container(
                &String::from("my-container-real"),
            ) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(http::Response::builder()
                        .status(500)
                        .body(ResponseBody::String(format!(
                            "failed to create container: {:?}",
                            e
                        )))
                        .expect("failed to create HTTP body for container error"));
                }
            },
        };

        let (_parts, mut body) = request.into_parts();

        let outgoing_value = OutgoingValue::new_outgoing_value();
        let mut output_stream = match outgoing_value.outgoing_value_write_body() {
            Ok(s) => s,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to create output stream: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for output stream error"));
            }
        };

        if let Err(e) = container.write_data(&String::from("hi.txt"), &outgoing_value) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to write data to container: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for write data error"));
        }

        if let Err(e) = std::io::copy(&mut body, &mut output_stream) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to write to output stream: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for write to output stream error"));
        }

        if let Err(e) = OutgoingValue::finish(outgoing_value) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to finish writing file: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for finish writing file error"));
        }

        let data = match container.get_data(&String::from("hi.txt"), 0, u64::MAX) {
            Ok(d) => d,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to read data from container: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for read data error"));
            }
        };

        let stream = match IncomingValue::incoming_value_consume_async(data) {
            Ok(s) => s,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to consume incoming value: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for consume incoming value error"));
            }
        };

        Ok(http::Response::new(ResponseBody::Stream(stream)))
    }
}

http::export!(Component);
