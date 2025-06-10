pub mod dev {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "dev",
        async: true,
        trappable_imports: true,
        with: {
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
        },
    });
}

pub mod plugin_guest {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "plugin-guest",
        // Flag this as "possibly async" which will cause the exports to be
        // generated as async, but none of the imports here are async since
        // all the blocking-ness happens in wasi:io
        async: {
            only_imports: ["nonexistent"],
        },

        with: {
            "wasmcloud:wash/types/runner": crate::runtime::types::Runner,
            "wasmcloud:wash/types/project-config": crate::runtime::types::ProjectConfig,
            "wasmcloud:wash/types/wash-config": crate::runtime::types::WashConfig,
            "wasmcloud:wash/types/context": crate::runtime::types::Context,
        }
    });
}

pub mod plugin_host {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "plugin-host",
        async: true,
        with: {
            "wasmcloud:wash/types": crate::runtime::bindings::plugin_guest::wasmcloud::wash::types,
        }
    });
}
