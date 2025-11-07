//! Generated WIT bindings for a keyvalue host component

wit_bindgen::generate!({
    world: "keyvalue-filesystem",
    with: {
        "wasi:keyvalue/store@0.2.0-draft": generate,
        "wasi:keyvalue/atomics@0.2.0-draft": generate,
        "wasi:keyvalue/batch@0.2.0-draft": generate,
        "wasmcloud:wash/types@0.0.2": generate,
        "wasmcloud:wash/plugin@0.0.2": generate,
    },
});
