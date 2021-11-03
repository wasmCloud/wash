use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Deserialize, Serialize, Debug)]
pub(crate) struct DefaultContext {
    /// Name of the default context
    pub name: String,
}

impl DefaultContext {
    pub fn new(name: String) -> Self {
        DefaultContext { name }
    }
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub(crate) struct WashContext {
    cluster_seed: String,

    #[serde(default = "default_nats_host")]
    ctl_host: String,
    #[serde(default = "default_nats_port")]
    ctl_port: u16,
    ctl_jwt: Option<String>,
    ctl_seed: Option<String>,
    ctl_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    ctl_timeout: u64,

    #[serde(default = "default_lattice_prefix")]
    lattice_prefix: String,

    #[serde(default = "default_nats_host")]
    rpc_host: String,
    #[serde(default = "default_nats_port")]
    rpc_port: u16,
    rpc_jwt: Option<String>,
    rpc_seed: Option<String>,
    rpc_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    rpc_timeout: u64,
}

impl Default for WashContext {
    fn default() -> Self {
        WashContext {
            cluster_seed: "".to_string(),
            ctl_host: default_nats_host(),
            ctl_port: default_nats_port(),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_timeout: default_timeout(),
            lattice_prefix: default_lattice_prefix(),
            rpc_host: default_nats_host(),
            rpc_port: default_nats_port(),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_credsfile: None,
            rpc_timeout: default_timeout(),
        }
    }
}

const DEFAULT_NATS_HOST: &str = "127.0.0.1";
const DEFAULT_NATS_PORT: u16 = 4222;
const DEFAULT_LATTICE_PREFIX: &str = "default";
const DEFAULT_TIMEOUT: u64 = 2_000;

fn default_nats_host() -> String {
    DEFAULT_NATS_HOST.to_string()
}

fn default_nats_port() -> u16 {
    DEFAULT_NATS_PORT
}

fn default_lattice_prefix() -> String {
    DEFAULT_LATTICE_PREFIX.to_string()
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT
}
