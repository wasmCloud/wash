wit_bindgen::generate!({
    world: "component"
});

struct Component;

impl exports::wasmcloud::example::cron::Guest for Component {
    fn invoke() -> Result<(), String> {
        eprintln!("Hello from the cron-component!");
        Ok(())
    }
}

export!(Component);
