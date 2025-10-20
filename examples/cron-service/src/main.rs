wit_bindgen::generate!({
    world: "service"
});

fn main() {
    eprintln!("Starting cron-service with 1 second intervals...");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let _ = wasmcloud::example::cron::invoke();
    }
}
