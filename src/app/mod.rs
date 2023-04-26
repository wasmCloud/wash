use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::{bail, Result};
use async_nats::{Client, Message};
use clap::{Args, Subcommand};
use serde_json::json;
use wadm::server::{
    DeleteModelRequest, DeleteModelResponse, DeployModelRequest, DeployModelResponse,
    GetModelRequest, GetModelResponse, GetResult, ModelSummary, PutModelResponse, PutResult,
    UndeployModelRequest, VersionResponse,
};
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::{DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT};
use wash_lib::context::{
    fs::{load_context, ContextDir},
    ContextManager,
};

use crate::{
    appearance::spinner::Spinner,
    ctl::ConnectionOpts,
    ctx::{context_dir, ensure_host_config_context},
};

const WADM_API_PREFIX: &str = "wadm.api";

mod output;

/// A helper enum to easily refer to model operations and then use the
/// [ToString](ToString) implementation for NATS topic formation
pub(crate) enum ModelOperation {
    List,
    Get,
    History,
    Delete,
    Put,
    Deploy,
    Undeploy,
}

impl ToString for ModelOperation {
    fn to_string(&self) -> String {
        match self {
            ModelOperation::List => "list",
            ModelOperation::Get => "get",
            ModelOperation::History => "versions",
            ModelOperation::Delete => "del",
            ModelOperation::Put => "put",
            ModelOperation::Deploy => "deploy",
            ModelOperation::Undeploy => "undeploy",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum AppCliCommand {
    /// List application specifications available within the lattice
    #[clap(name = "list")]
    List(ListCommand),
    /// Retrieve the details for a specific version of an app specification
    #[clap(name = "get")]
    Get(GetCommand),
    /// Retrieve the version history of a given model within the lattice
    #[clap(name = "history")]
    History(HistoryCommand),
    /// Delete a model version
    #[clap(name = "del")]
    Delete(DeleteCommand),
    /// Puts a model version into the store
    #[clap(name = "put")]
    Put(PutCommand),
    /// Deploy an app (start a deployment monitor)
    #[clap(name = "deploy")]
    Deploy(DeployCommand),
    /// Undeploy an application (stop the deployment monitor)
    #[clap(name = "undeploy")]
    Undeploy(UndeployCommand),
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ListCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct UndeployCommand {
    /// Name of the app specification to undeploy
    #[clap(name = "name")]
    model_name: String,

    /// Whether or not to delete resources that are undeployed. Defaults to remove managed resources
    #[clap(long = "non-destructive")]
    non_destructive: bool,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct DeployCommand {
    /// Name of the app specification to deploy
    #[clap(name = "name")]
    model_name: String,

    /// Version of the app specification to deploy, defaults to the latest created version
    #[clap(name = "version")]
    version: Option<String>,

    /// Force deployment of this manifest, overwriting the existing version if present
    #[clap(short = 'f', long = "force")]
    force: bool,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct DeleteCommand {
    /// Name of the app specification to delete
    #[clap(name = "name")]
    model_name: String,

    #[clap(long = "delete-all")]
    /// Whether or not to delete all app versions, defaults to `false`
    delete_all: bool,

    /// Version of the app specification to delete. Not required if --delete-all is supplied
    #[clap(name = "version", required_unless_present("delete_all"))]
    version: Option<String>,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct PutCommand {
    /// Input filename (JSON or YAML) containing app specification
    source: PathBuf,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct GetCommand {
    /// The name of the app spec to retrieve
    #[clap(name = "name")]
    model_name: String,

    /// The version of the app spec to retrieve. If left empty, retrieves the latest version
    #[clap(name = "version")]
    version: Option<String>,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Args, Debug, Clone)]
pub(crate) struct HistoryCommand {
    /// The name of the app spec
    #[clap(name = "name")]
    model_name: String,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

pub(crate) async fn handle_command(
    command: AppCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    use AppCliCommand::*;
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        List(cmd) => {
            sp.update_spinner_message("Querying app spec list ...".to_string());
            let results = get_models(cmd).await?;
            list_models_output(results)
        }
        Get(cmd) => {
            sp.update_spinner_message("Querying app spec details ... ".to_string());
            let results = get_model_details(cmd).await?;
            show_model_output(results)
        }
        History(cmd) => {
            sp.update_spinner_message("Querying app revision history ... ".to_string());
            let results = get_model_history(cmd).await?;
            show_model_history(results)
        }
        Delete(cmd) => {
            sp.update_spinner_message("Deleting app version ... ".to_string());
            let results = delete_model_version(cmd).await?;
            show_del_results(results)
        }
        Put(cmd) => {
            sp.update_spinner_message("Uploading app specification ... ".to_string());
            let results = put_model(cmd).await?;
            show_put_results(results)
        }
        Deploy(cmd) => {
            sp.update_spinner_message("Deploying application ... ".to_string());
            let results = deploy_model(cmd).await?;
            show_deploy_results(results)
        }
        Undeploy(cmd) => {
            sp.update_spinner_message("Undeploying application ... ".to_string());
            let results = undeploy_model(cmd).await?;
            show_undeploy_results(results)
        }
    };
    sp.finish_and_clear();

    Ok(out)
}

async fn undeploy_model(cmd: UndeployCommand) -> Result<DeployModelResponse> {
    let res = model_request(
        cmd.opts,
        ModelOperation::Undeploy,
        Some(&cmd.model_name),
        serde_json::to_vec(&UndeployModelRequest {
            non_destructive: cmd.non_destructive,
        })?,
    )
    .await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn deploy_model(cmd: DeployCommand) -> Result<DeployModelResponse> {
    // If the model name is a file on disk, apply it and then deploy
    let name = if tokio::fs::metadata(&cmd.model_name).await.is_ok() {
        let put_res = put_model(PutCommand {
            source: PathBuf::from(&cmd.model_name),
            opts: cmd.opts.to_owned(),
        })
        .await?;

        match put_res.result {
            PutResult::Created | PutResult::NewVersion => put_res.name,
            // TODO: Might be better to have a `PutVersion::AlreadyExists` variant
            PutResult::Error if cmd.force => {
                // TODO: Care about this
                let delete_res = delete_model_version(DeleteCommand {
                    //TODO: Name and version here are empty, need that info
                    model_name: put_res.name.to_owned(),
                    version: Some(put_res.current_version.to_owned()),
                    delete_all: false,
                    opts: cmd.opts.to_owned(),
                })
                .await?;
                let put_res = put_model(PutCommand {
                    source: PathBuf::from(&cmd.model_name),
                    opts: cmd.opts.to_owned(),
                })
                .await?;

                put_res.name
            }
            _ => bail!("Could not put manifest to deploy {}", put_res.message),
        }
    } else {
        cmd.model_name
    };

    let res = model_request(
        cmd.opts,
        ModelOperation::Deploy,
        Some(&name),
        serde_json::to_vec(&DeployModelRequest {
            version: cmd.version,
        })?,
    )
    .await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn put_model(cmd: PutCommand) -> Result<PutModelResponse> {
    let raw = std::fs::read_to_string(&cmd.source)?;
    let res = model_request(cmd.opts, ModelOperation::Put, None, raw.as_bytes().to_vec()).await?;
    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn get_model_history(cmd: HistoryCommand) -> Result<VersionResponse> {
    let res = model_request(
        cmd.opts,
        ModelOperation::History,
        Some(&cmd.model_name),
        vec![],
    )
    .await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn get_model_details(cmd: GetCommand) -> Result<GetModelResponse> {
    let res = model_request(
        cmd.opts,
        ModelOperation::Get,
        Some(&cmd.model_name),
        serde_json::to_vec(&GetModelRequest {
            version: cmd.version,
        })?,
    )
    .await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn delete_model_version(cmd: DeleteCommand) -> Result<DeleteModelResponse> {
    let res = model_request(
        cmd.opts,
        ModelOperation::Delete,
        Some(&cmd.model_name),
        serde_json::to_vec(&DeleteModelRequest {
            version: cmd.version.unwrap_or_default(),
            delete_all: cmd.delete_all,
        })?,
    )
    .await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

async fn get_models(cmd: ListCommand) -> Result<Vec<ModelSummary>> {
    let res = model_request(cmd.opts, ModelOperation::List, None, vec![]).await?;

    serde_json::from_slice(&res.payload).map_err(|e| anyhow::anyhow!(e))
}

fn list_models_output(results: Vec<ModelSummary>) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("apps".to_string(), json!(results));
    CommandOutput::new(output::list_models_table(results), map)
}

fn show_model_output(md: GetModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("model".to_string(), json!(md));
    if md.result == GetResult::Success {
        let yaml = serde_yaml::to_string(&md.manifest).unwrap();
        CommandOutput::new(yaml, map)
    } else {
        CommandOutput::new(md.message, map)
    }
}

fn show_put_results(results: PutModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_undeploy_results(results: DeployModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("results".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_del_results(results: DeleteModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("deleted".to_string(), json!(results));
    CommandOutput::new(results.message, map)
}

fn show_deploy_results(results: DeployModelResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("acknowledged".to_string(), json!(results));
    // TODO: more info here, wadm just returns "Deployed model"
    CommandOutput::new(results.message, map)
}

fn show_model_history(results: VersionResponse) -> CommandOutput {
    let mut map = HashMap::new();
    map.insert("revisions".to_string(), json!(results));
    CommandOutput::new(output::list_revisions_table(results.versions), map)
}

async fn nats_client_from_opts(opts: ConnectionOpts) -> Result<(Client, Duration)> {
    // Attempt to load a context, falling back on the default if not supplied
    let ctx = if let Some(context) = opts.context {
        Some(load_context(context)?)
    } else if let Ok(ctx_dir) = context_dir(None) {
        let ctx_dir = ContextDir::new(ctx_dir)?;
        ensure_host_config_context(&ctx_dir)?;
        Some(ctx_dir.load_default_context()?)
    } else {
        None
    };

    let ctl_host = opts.ctl_host.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_host.clone())
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
    });

    let ctl_port = opts.ctl_port.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
    });

    let ctl_jwt = if opts.ctl_jwt.is_some() {
        opts.ctl_jwt
    } else {
        ctx.as_ref().map(|c| c.ctl_jwt.clone()).unwrap_or_default()
    };

    let ctl_seed = if opts.ctl_seed.is_some() {
        opts.ctl_seed
    } else {
        ctx.as_ref().map(|c| c.ctl_seed.clone()).unwrap_or_default()
    };

    let ctl_credsfile = if opts.ctl_credsfile.is_some() {
        opts.ctl_credsfile
    } else {
        ctx.as_ref()
            .map(|c| c.ctl_credsfile.clone())
            .unwrap_or_default()
    };

    let nc =
        crate::util::nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
            .await?;

    let timeout = Duration::from_millis(opts.timeout_ms);

    Ok((nc, timeout))
}

/// Helper function to make a NATS request given connection options, an operation, optional name, and bytes
async fn model_request(
    opts: ConnectionOpts,
    operation: ModelOperation,
    object_name: Option<&str>,
    bytes: Vec<u8>,
) -> Result<Message> {
    let (nc, timeout) = nats_client_from_opts(opts.clone()).await?;

    // Topic is of the form of wadm.api.<lattice>.<category>.<operation>.<OPTIONAL: object_name>
    // We let callers of this function dictate the topic after the prefix + lattice
    let topic = format!(
        "{WADM_API_PREFIX}.{}.model.{}{}",
        opts.lattice_prefix
            .unwrap_or_else(|| DEFAULT_LATTICE_PREFIX.to_string()),
        operation.to_string(),
        object_name
            .map(|name| format!(".{name}"))
            .unwrap_or_default()
    );

    match tokio::time::timeout(timeout, nc.request(topic, bytes.into())).await {
        Ok(Ok(res)) => Ok(res),
        Ok(Err(e)) => bail!("Error making model request: {}", e),
        Err(e) => bail!("model_request timed out:  {}", e),
    }
}
