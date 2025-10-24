wit_bindgen::generate!({
    world: "service"
});

// NOTE: This example is a `tokio::main` to show how you can use an async main, but
// it can just be a synchronous main as well.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    eprintln!("Starting cron-service with 1 second intervals...");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let _ = wasmcloud::example::cron::invoke();
    }
}
