use wasmcloud_component::http;
wit_bindgen::generate!({ generate_all });

use crate::example::action::action::run;
struct Component;

http::export!(Component);

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let result = run();

        match result {
            Ok(res) => Ok(http::Response::new(format!("{}\n", res))),
            Err(e) => Ok(http::Response::new(format!("Error: {}\n", e))),
        }
    }
}
