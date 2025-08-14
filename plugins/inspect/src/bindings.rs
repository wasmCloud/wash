//! Generated WIT bindings for the inspect plugin
use crate::Component;

wit_bindgen::generate!({
    path: "../../wit",
    world: "plugin-guest",
    generate_all,
});

export!(Component);
