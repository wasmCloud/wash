use crate::util::{
    DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT, DEFAULT_NATS_TIMEOUT,
};
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
    pub cluster_seed: Option<String>,

    #[serde(default = "default_nats_host")]
    pub ctl_host: String,
    #[serde(default = "default_nats_port")]
    pub ctl_port: String,
    pub ctl_jwt: Option<String>,
    pub ctl_seed: Option<String>,
    pub ctl_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    pub ctl_timeout: u64,

    #[serde(default = "default_lattice_prefix")]
    pub lattice_prefix: String,

    #[serde(default = "default_nats_host")]
    pub rpc_host: String,
    #[serde(default = "default_nats_port")]
    pub rpc_port: String,
    pub rpc_jwt: Option<String>,
    pub rpc_seed: Option<String>,
    pub rpc_credsfile: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    pub rpc_timeout: u64,
}

impl Default for WashContext {
    fn default() -> Self {
        WashContext {
            cluster_seed: None,
            ctl_host: DEFAULT_NATS_HOST.to_string(),
            ctl_port: DEFAULT_NATS_PORT.to_string(),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ctl_timeout: DEFAULT_NATS_TIMEOUT,
            lattice_prefix: DEFAULT_LATTICE_PREFIX.to_string(),
            rpc_host: DEFAULT_NATS_HOST.to_string(),
            rpc_port: DEFAULT_NATS_PORT.to_string(),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_credsfile: None,
            rpc_timeout: DEFAULT_NATS_TIMEOUT,
        }
    }
}

// Below are required functions for serde default derive with WashContext

fn default_nats_host() -> String {
    DEFAULT_NATS_HOST.to_string()
}

fn default_nats_port() -> String {
    DEFAULT_NATS_PORT.to_string()
}

fn default_lattice_prefix() -> String {
    DEFAULT_LATTICE_PREFIX.to_string()
}

fn default_timeout() -> u64 {
    DEFAULT_NATS_TIMEOUT
}
