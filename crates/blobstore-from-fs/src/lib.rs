use wasmcloud_component::http;

struct Component;

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let container = wasmcloud_component::wasi::blobstore::blobstore::create_container(
            &String::from("my-container-real"),
        )
        .expect("should create container");

        Ok(http::Response::new(
            container.name().expect("should have name"),
        ))
    }
}

http::export!(Component);
