use wasmcloud_component::http;

struct Component;

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        Ok(http::Response::new("Hello from wasmCloud!\n"))
    }
}

http::export!(Component);
