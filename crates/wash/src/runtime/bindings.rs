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
        async: true,

        with: {
            "wasmcloud:wash": crate::runtime::bindings::plugin_host::wasmcloud::wash,
            // "wasi:logging": wasmcloud_runtime::capability::logging,
        }
    });
}

pub mod plugin_host {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "plugin-host",
        additional_derives: [serde::Serialize],
        async: true,
        with: {
            "wasmcloud:wash/types/runner": crate::runtime::types::Runner,
            "wasmcloud:wash/types/project-config": crate::runtime::types::ProjectConfig,
            "wasmcloud:wash/types/wash-config": crate::runtime::types::WashConfig,
            "wasmcloud:wash/types/context": crate::runtime::types::Context,
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
                HookType::BeforeDev => "BeforeDev",
                HookType::AfterDev => "AfterDev",
            };
            write!(f, "{s}")
        }
    }
}
