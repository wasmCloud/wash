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

    use wasmcloud::wash::types::HookType;
    impl ToString for HookType {
        fn to_string(&self) -> String {
            match self {
                HookType::Unknown => "Unknown".to_string(),
                HookType::Error => "Error".to_string(),
                HookType::BeforeLogin => "BeforeLogin".to_string(),
                HookType::AfterLogin => "AfterLogin".to_string(),
                HookType::BeforeBuild => "BeforeBuild".to_string(),
                HookType::AfterBuild => "AfterBuild".to_string(),
                HookType::BeforePush => "BeforePush".to_string(),
                HookType::AfterPush => "AfterPush".to_string(),
                HookType::DevRegister => "DevRegister".to_string(),
                HookType::BeforeDevBuild => "BeforeDevBuild".to_string(),
                HookType::BeforeDevServe => "BeforeDevServe".to_string(),
                HookType::BeforeDevShutdown => "BeforeDevShutdown".to_string(),
            }
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
