//! Generated WIT bindings for a blobstore host component
use crate::Component;

wit_bindgen::generate!({
    world: "blobstore-filesystem",
    with: {
        "wasi:blobstore/blobstore@0.2.0-draft": generate,
        "wasi:blobstore/container@0.2.0-draft": generate,
        "wasi:blobstore/types@0.2.0-draft": generate,
        "wasi:io/error@0.2.1": ::wasi::io::error,
        "wasi:io/poll@0.2.1": ::wasi::io::poll,
        "wasi:io/streams@0.2.1": ::wasi::io::streams,
        "wasi:logging/logging@0.1.0-draft": generate,
        "wasmcloud:wash/types@0.0.2": generate,
        "wasmcloud:wash/plugin@0.0.2": generate,
    },
});

export!(Component);
