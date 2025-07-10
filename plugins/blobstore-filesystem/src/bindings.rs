//! Generated WIT bindings for a blobstore host component
use crate::Component;

wit_bindgen::generate!({
    path: "../../wit",
    world: "blobstore-host",
    with: {
        "wasi:io/error@0.2.1": ::wasi::io::error,
        "wasi:io/poll@0.2.1": ::wasi::io::poll,
        "wasi:io/streams@0.2.1": ::wasi::io::streams,
        "wasi:blobstore/blobstore@0.2.0-draft": generate,
        "wasi:blobstore/container@0.2.0-draft": generate,
        "wasi:blobstore/types@0.2.0-draft": generate,
    },
});

export!(Component);
