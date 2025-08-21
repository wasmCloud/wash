//! Generated WIT bindings
use crate::Component;

wit_bindgen::generate!({
    world: "aspire-otel",
    generate_all,
});

export!(Component);
