use tonic::Request;
use wasmcloud_grpc_client::GrpcEndpoint;

use hello_world::greeter_client::GreeterClient;
use hello_world::HelloRequest;

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

struct Component;

impl wasmcloud_component::http::Server for Component {
    fn handle(
        _req: wasmcloud_component::http::IncomingRequest,
    ) -> wasmcloud_component::http::Result<
        wasmcloud_component::http::Response<impl wasmcloud_component::http::OutgoingBody>,
    > {
        // Use tokio to run the async code in a blocking context
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| {
                wasmcloud_component::http::ErrorCode::InternalError(Some(format!(
                    "failed to create tokio runtime: {}",
                    e
                )))
            })?;

        runtime.block_on(async {
            eprintln!("Starting gRPC client...");

            // Parse the gRPC endpoint URI from config or use default
            let endpoint_uri = std::env::var("GRPC_SERVER_URI")
                .unwrap_or_else(|_| "http://[::1]:50051".to_string());

            eprintln!("Connecting to gRPC server: {}", endpoint_uri);

            let endpoint_uri = endpoint_uri.parse().map_err(|e| {
                wasmcloud_component::http::ErrorCode::InternalError(Some(format!(
                    "failed to parse endpoint URI: {}",
                    e
                )))
            })?;

            // Create the gRPC endpoint wrapper
            let endpoint = GrpcEndpoint::new(endpoint_uri);

            // Create the gRPC client
            let mut client = GreeterClient::new(endpoint);

            // Make the gRPC call
            let request = Request::new(HelloRequest {
                name: "wasmCloud".to_string(),
            });

            eprintln!("Sending gRPC request...");
            let response = client.say_hello(request).await.map_err(|e| {
                wasmcloud_component::http::ErrorCode::InternalError(Some(format!(
                    "gRPC call failed: {}",
                    e
                )))
            })?;

            let message = response.into_inner().message;
            eprintln!("gRPC Response: {}", message);

            Ok(wasmcloud_component::http::Response::new(message))
        })
    }
}

wasmcloud_component::http::export!(Component);
