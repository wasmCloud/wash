use crate::wasi::logging::logging;

wit_bindgen::generate!({
    world: "component",
    generate_all
});

struct Component;

impl exports::wasmcloud::example::receiver::Guest for Component {
    fn invoke() -> Result<(), String> {
        logging::log(logging::Level::Debug, "receiver", "invoke");
        Ok(())
    }
}

export!(Component);
