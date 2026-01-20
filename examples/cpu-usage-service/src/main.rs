use crate::wasi::logging::logging;

wit_bindgen::generate!({
    world: "service",
    generate_all
});

// NOTE: This example is a `tokio::main` to show how you can use an async main, but
// it can just be a synchronous main as well.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    eprintln!("Starting cron-service with 1 second intervals...");
    let system = sysinfo::System::new();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        logging::log(
            logging::Level::Info,
            "service",
            &format!("CPU usage: {}", system.global_cpu_usage()),
        );
    }
}
