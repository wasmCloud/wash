//! Generated WIT bindings
use crate::Component;

wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin-guest",
    generate_all,
});

export!(Component);
