pub mod dev {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "dev",
        async: true,
        trappable_imports: true,
        with: {
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
           "wasi:io": wasmtime_wasi::bindings::io,
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
        additional_derives: [serde::Serialize],
        async: {
            only_imports: ["nonexistent"],
        },

        with: {
            "wasmcloud:wash/types/runner": crate::runtime::types::Runner,
            "wasmcloud:wash/types/project-config": crate::runtime::types::ProjectConfig,
            "wasmcloud:wash/types/wash-config": crate::runtime::types::WashConfig,
            "wasmcloud:wash/types/context": crate::runtime::types::Context,
            "wasi:io": wasmtime_wasi::bindings::io,
        }
    });

    // Using crate imports here to keep the top-level `use` statements clean
    use std::fmt::Display;
    use wasmcloud::wash::types::HookType;

    impl Display for HookType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let s = match self {
                HookType::Unknown => "Unknown",
                HookType::Error => "Error",
                HookType::BeforeLogin => "BeforeLogin",
                HookType::AfterLogin => "AfterLogin",
                HookType::BeforeBuild => "BeforeBuild",
                HookType::AfterBuild => "AfterBuild",
                HookType::BeforePush => "BeforePush",
                HookType::AfterPush => "AfterPush",
                HookType::DevRegister => "DevRegister",
                HookType::BeforeDevBuild => "BeforeDevBuild",
                HookType::BeforeDevServe => "BeforeDevServe",
                HookType::BeforeDevShutdown => "BeforeDevShutdown",
            };
            write!(f, "{s}")
        }
    }
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
