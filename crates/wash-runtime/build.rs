use std::env;
use std::fs;
use std::path::PathBuf;

fn workspace_dir() -> anyhow::Result<PathBuf> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let mut current_path = PathBuf::from(manifest_dir);

    // Search upwards for the workspace root
    loop {
        if current_path.join("Cargo.lock").exists() {
            println!("cargo:rustc-env=WORKSPACE_ROOT={}", current_path.display());
            return Ok(current_path);
        }

        if let Some(parent) = current_path.parent() {
            current_path = parent.to_path_buf();
        } else {
            anyhow::bail!("Could not find workspace root")
        }
    }
}
fn main() {
    let out_dir = PathBuf::from(
        env::var("OUT_DIR").expect("failed to look up `OUT_DIR` from environment variables"),
    );
    let workspace_dir = workspace_dir().expect("failed to get workspace dir");
    println!("cargo:rerun-if-changed=tests/proto");
    println!("cargo:rerun-if-changed=proto");
    let top_proto_dir = workspace_dir.join("proto");
    let proto_dir = top_proto_dir.join("wasmcloud/runtime/v2");

    // Compile Runtime API protos if directory exists
    if proto_dir.exists() {
        let proto_dir_files =
            fs::read_dir(&proto_dir).expect("failed to list files in `proto_dir`");
        let proto_files: Vec<PathBuf> = proto_dir_files
            .into_iter()
            .map(|file| file.expect("failed to read proto file").path())
            .collect();

        let descriptor_file = out_dir.join("runtime.bin");

        tonic_prost_build::configure()
            .compile_well_known_types(true)
            .file_descriptor_set_path(&descriptor_file)
            .extern_path(".google.protobuf", "::pbjson_types")
            .compile_protos(&proto_files, &[top_proto_dir])
            .expect("failed to compile Runtime API protos");

        // Generate serde bindings for the Runtime API
        let descriptor_bytes =
            std::fs::read(&descriptor_file).expect("failed to read descriptor file");

        pbjson_build::Builder::new()
            .register_descriptors(&descriptor_bytes)
            .expect("failed to register descriptor")
            .build(&[".wasmcloud.runtime.v2"])
            .expect("failed to build Runtime API protos");
    }

    // Compile test proto files if they exist
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let test_proto_file = crate_dir.join("tests/proto/helloworld.proto");

    println!("cargo:rerun-if-changed={}", test_proto_file.display());

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .build_transport(true)
        .compile_protos(&[&test_proto_file], &[&crate_dir.join("tests/proto")])
        .expect("failed to compile test protos");
}
