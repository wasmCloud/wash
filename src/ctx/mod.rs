use crate::{
    cfg::cfg_dir,
    generate::{
        interactive::{prompt_for_choice, user_question},
        project_variables::StringEntry,
    },
    id::ClusterSeed,
    util::{
        format_output, Output, OutputKind, Result, DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST,
        DEFAULT_NATS_PORT, DEFAULT_NATS_TIMEOUT,
    },
};
use log::warn;
use serde_json::json;
use std::{
    fs::File,
    io::{BufReader, Error, ErrorKind},
    path::{Path, PathBuf},
    process::Command,
};
use structopt::{clap::AppSettings, StructOpt};
pub mod context;
use context::{DefaultContext, WashContext};

const CTX_DIR: &str = "contexts";
const INDEX_JSON: &str = "index.json";
const HOST_CONFIG_PATH: &str = ".wash/host_config.json";
const HOST_CONFIG_NAME: &str = "host_config";

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "ctx")]
pub(crate) struct CtxCli {
    #[structopt(flatten)]
    command: CtxCommand,
}

impl CtxCli {
    pub(crate) fn command(self) -> CtxCommand {
        self.command
    }
}

pub(crate) async fn handle_command(ctx_cmd: CtxCommand) -> Result<String> {
    use CtxCommand::*;
    match ctx_cmd {
        List(cmd) => handle_list(cmd),
        Default(cmd) => handle_default(cmd),
        Edit(cmd) => handle_edit(cmd),
        New(cmd) => handle_new(cmd),
        Del(cmd) => handle_del(cmd),
    }
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) enum CtxCommand {
    /// Lists all stored contexts (JSON files) found in the context directory, with the exception of index.json
    #[structopt(name = "list")]
    List(ListCommand),
    /// Delete a stored context
    #[structopt(name = "del")]
    Del(DelCommand),
    /// Create a new context
    #[structopt(name = "new")]
    New(NewCommand),
    /// Set the default context
    #[structopt(name = "default")]
    Default(DefaultCommand),
    /// Edit a context directly using a text editor
    #[structopt(name = "edit")]
    Edit(EditCommand),
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct ListCommand {
    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct DelCommand {
    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,

    /// Name of the context to delete. If not supplied, the user will be prompted to select an existing context
    #[structopt(name = "name")]
    name: Option<String>,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct NewCommand {
    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Name of the context, will be sanitized to ensure it's a valid filename
    #[structopt(name = "name", required_unless("interactive"))]
    pub(crate) name: Option<String>,

    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,

    /// Create the context in an interactive terminal prompt, instead of an autogenerated default context
    #[structopt(long = "interactive", short = "i")]
    interactive: bool,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct DefaultCommand {
    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,

    /// Name of the context to use for default. If not supplied, the user will be prompted to select a default
    #[structopt(name = "name")]
    name: Option<String>,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct EditCommand {
    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,

    /// Name of the context to edit, if not supplied the user will be prompted to select a context
    #[structopt(name = "name")]
    pub(crate) name: Option<String>,

    /// Your terminal text editor of choice. This editor must be present in your $PATH, or an absolute filepath.
    #[structopt(short = "e", long = "editor", env = "EDITOR")]
    pub(crate) editor: String,
}

/// Lists all JSON files found in the context directory, with the exception of `index.json`
/// Being present in this list does not guarantee a valid context
fn handle_list(cmd: ListCommand) -> Result<String> {
    let dir = context_dir(cmd.directory)?;
    let _ = ensure_host_config_context(&dir);

    let index = get_index(&dir).ok();
    let contexts = context_filestems_from_path(get_contexts(&dir)?);

    let output_contexts = match cmd.output.kind {
        OutputKind::Text if index.is_some() => contexts
            .into_iter()
            .map(|f| {
                if f == index.as_ref().unwrap().name {
                    format!("{} (default)", f)
                } else {
                    f
                }
            })
            .collect(),
        _ => contexts,
    };

    Ok(format_output(
        format!(
            "== Contexts found in {} ==\n{}",
            dir.display(),
            output_contexts.join("\n")
        ),
        json!({ "contexts": output_contexts, "default": index.map(|i| i.name).unwrap_or_else(|| "N/A".to_string()) }),
        &cmd.output.kind,
    ))
}

/// Handles selecting a default context, which can be selected in the terminal or provided as an argument
fn handle_default(cmd: DefaultCommand) -> Result<String> {
    let dir = context_dir(cmd.directory)?;
    let _ = ensure_host_config_context(&dir);
    let contexts = get_contexts(&dir)?;

    let new_default = cmd.name.unwrap_or_else(|| {
        select_context(contexts.clone(), &dir, "Select a default context:").unwrap_or_default()
    });

    if contexts.contains(&context_path_from_name(&dir, &new_default)) {
        let res = set_default_context(&dir, new_default)
            .map(|_| "Set new context successfully".to_string())?;
        Ok(format_output(
            res,
            json!({"success": true}),
            &cmd.output.kind,
        ))
    } else {
        Err(Error::new(
            ErrorKind::NotFound,
            "Failed to set new default, context supplied was not found".to_string(),
        )
        .into())
    }
}

/// Handles deleting an existing context
fn handle_del(cmd: DelCommand) -> Result<String> {
    let dir = context_dir(cmd.directory)?;
    let contexts = get_contexts(&dir)?;

    let ctx_to_delete = cmd.name.unwrap_or_else(|| {
        select_context(contexts.clone(), &dir, "Select a context to delete:").unwrap_or_default()
    });

    let path = context_path_from_name(&dir, &ctx_to_delete);

    if contexts.contains(&path) {
        let res = std::fs::remove_file(path).map(|_| "Removed file successfully".to_string())?;
        Ok(format_output(
            res,
            json!({"success": true}),
            &cmd.output.kind,
        ))
    } else {
        Err(Error::new(
            ErrorKind::NotFound,
            "Failed to delete, no context found".to_string(),
        )
        .into())
    }
}

/// Handles creating a new context by writing the default WashContext object to the specified path
fn handle_new(cmd: NewCommand) -> Result<String> {
    let dir = context_dir(cmd.directory.clone())?;

    let new_context = if cmd.interactive {
        prompt_for_context()?
    } else {
        WashContext::named(cmd.name.unwrap())
    };

    let filename = format!("{}.json", new_context.name);
    let options = sanitize_filename::Options {
        truncate: true,
        windows: true,
        replacement: "_",
    };

    // Ensure filename doesn't include uphill/downhill\ slashes, or reserved prefixes
    let sanitized = sanitize_filename::sanitize_with_options(filename, options);
    let context_path = dir.join(sanitized.clone());
    let res = serde_json::to_writer(&File::create(context_path)?, &new_context)
        .map(|_| format!("Created context {} with default values", sanitized))?;
    Ok(format_output(
        res,
        json!({"success": true}),
        &cmd.output.kind,
    ))
}

/// Handles editing a context by opening the JSON file in the user's text editor of choice
fn handle_edit(cmd: EditCommand) -> Result<String> {
    let dir = context_dir(cmd.directory.clone())?;
    let editor = which::which(cmd.editor)?;

    let _ = ensure_host_config_context(&dir);

    let ctx = if let Some(ctx) = cmd.name {
        // Ensure user supplied context exists
        std::fs::metadata(context_path_from_name(&dir, &ctx))
            .ok()
            .map(|_| ctx)
    } else {
        let contexts = get_contexts(&dir)?;
        select_context(contexts, &dir, "Select a context to edit:")
    };

    if let Some(ctx_name) = ctx {
        if ctx_name == HOST_CONFIG_NAME {
            warn!("Edits to the host_config context will be overwritten, make changes to the host config instead");
        }
        Command::new(editor)
            .arg(&context_path_from_name(&dir, &ctx_name))
            .status()
            .map(|_exit_status| "Finished editing context successfully".to_string())
            .map_err(|e| e.into())
    } else {
        Err(Error::new(
            ErrorKind::NotFound,
            "Unable to find context supplied, please ensure it exists".to_string(),
        )
        .into())
    }
}

/// Takes an absolute path to a directory containing an index.json and returns an DefaultContext object
fn get_index(context_dir: &Path) -> Result<DefaultContext> {
    let file = std::fs::File::open(context_dir.join(INDEX_JSON))?;
    let reader = BufReader::new(file);
    let index = serde_json::from_reader(reader)?;
    Ok(index)
}

/// Set the default context by overwriting the current index.json file in the contexts directory
fn set_default_context(context_dir: &Path, default_context: String) -> Result<()> {
    let index = DefaultContext::new(default_context);
    let context_path = context_dir.join(INDEX_JSON);
    serde_json::to_writer(&File::create(context_path)?, &index).map_err(|e| e.into())
}

/// Loads the default context, according to index.json, into a WashContext object
pub(crate) fn get_default_context(context_dir: &Path) -> Result<WashContext> {
    match get_index(context_dir) {
        Ok(index) if index.name == HOST_CONFIG_NAME => {
            create_host_config_context(context_dir)?;
            load_context(&context_path_from_name(context_dir, HOST_CONFIG_NAME))
        }
        Ok(index) => load_context(&context_path_from_name(context_dir, &index.name)),
        _ => {
            create_host_config_context(context_dir)?;
            set_default_context(context_dir, HOST_CONFIG_NAME.to_string())?;
            load_context(&context_path_from_name(context_dir, HOST_CONFIG_NAME))
        }
    }
}

/// Load a file and return a Result containing the WashContext object.
pub(crate) fn load_context(context_path: &Path) -> Result<WashContext> {
    let file = std::fs::File::open(context_path)?;
    let reader = BufReader::new(file);
    let ctx = serde_json::from_reader(reader)?;
    Ok(ctx)
}

/// Ensures the host config context exists, setting it as a default if a default context
/// is not configured. The `host_config` context will be overwritten with the values found
/// in the `~/.wash/host_config.json` file.
fn ensure_host_config_context(context_dir: &Path) -> Result<()> {
    create_host_config_context(context_dir)?;

    // No default context is set, set to `host_config` context
    if get_default_context(context_dir).is_err() {
        set_default_context(context_dir, HOST_CONFIG_NAME.to_string())?;
    }
    Ok(())
}

/// Load the host configuration file and create a context called `host_config` from it
fn create_host_config_context(context_dir: &Path) -> Result<()> {
    let host_config_path = home_dir()?.join(HOST_CONFIG_PATH);
    let host_config_ctx = WashContext {
        name: HOST_CONFIG_NAME.to_string(),
        ..load_context(&host_config_path)?
    };
    let output_ctx_path = context_path_from_name(context_dir, HOST_CONFIG_NAME);
    serde_json::to_writer(&File::create(output_ctx_path)?, &host_config_ctx).map_err(|e| e.into())
}

/// Given a context directory, retrieve all contexts in the form of their absolute paths
fn get_contexts(context_dir: &Path) -> Result<Vec<PathBuf>> {
    let paths = std::fs::read_dir(context_dir).map_err(|e| {
        format!(
            "Error: {}, please ensure directory {} exists",
            e,
            context_dir.display()
        )
    })?;

    let index = std::ffi::OsString::from(INDEX_JSON);
    Ok(paths
        .filter_map(|p| {
            if let Ok(ctx_entry) = p {
                let path = ctx_entry.path();
                let ctx_filename = ctx_entry.file_name();
                match path.extension().map(|os| os.to_str()).unwrap_or_default() {
                    // Don't include index in the list of contexts
                    Some("json") if ctx_filename == index => None,
                    Some("json") => Some(path),
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect())
}

/// Converts a list of absolute paths to contexts to their human-readable filestems
fn context_filestems_from_path(contexts: Vec<PathBuf>) -> Vec<String> {
    contexts
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .unwrap_or_default()
                .to_os_string()
                .into_string()
                .ok()
        })
        .collect()
}

/// Given an optional supplied directory, determine the context directory either from the supplied
/// directory or using the home directory and the predefined `.wash/contexts` folder.
pub(crate) fn context_dir(cmd_dir: Option<PathBuf>) -> Result<PathBuf> {
    let dir = if let Some(dir) = cmd_dir {
        dir
    } else {
        cfg_dir()?.join(CTX_DIR)
    };

    // Ensure user supplied context exists
    if std::fs::metadata(&dir).is_err() {
        let _ = std::fs::create_dir_all(&dir);
    }
    Ok(dir)
}

fn home_dir() -> Result<PathBuf> {
    Ok(dirs::home_dir().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "Context directory not found, please set $HOME or $WASH_CONTEXTS for managed contexts",
        )
    })?)
}

/// Helper function to properly format the path to a context JSON file
fn context_path_from_name(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", name))
}

/// Prompts the user with the provided `contexts` choices and returns the user's response.
/// This can be used to determine which context to delete, edit, or set as a default, for example
fn select_context(contexts: Vec<PathBuf>, dir: &Path, prompt: &str) -> Option<String> {
    let default = get_index(dir).ok().map(|i| i.name);

    let choices: Vec<String> = context_filestems_from_path(contexts);

    let entry = StringEntry {
        default,
        choices: Some(choices.clone()),
        regex: None,
    };

    if let Ok(choice) = prompt_for_choice(&entry, prompt) {
        choices.get(choice).map(|c| c.to_string())
    } else {
        None
    }
}

/// Prompts the user interactively for context values, returning a constructed context
fn prompt_for_context() -> Result<WashContext> {
    let name = user_question(
        "What do you want to name the context?",
        &Some("default".to_string()),
    )?;

    let cluster_seed = match user_question(
        "What cluster seed do you want to use to sign invocations?",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s.parse::<ClusterSeed>()?),
        _ => None,
    };
    let ctl_host = user_question(
        "What is the control interface connection host?",
        &Some(DEFAULT_NATS_HOST.to_string()),
    )?;
    let ctl_port = user_question(
        "What is the control interface connection port?",
        &Some(DEFAULT_NATS_PORT.to_string()),
    )?;
    let ctl_jwt = match user_question(
        "Enter your JWT that you use to authenticate to the control interface connection, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let ctl_seed = match user_question(
        "Enter your user seed that you use to authenticate to the control interface connection, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let ctl_credsfile = match user_question(
        "Enter the absolute path to control interface connection credsfile, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let ctl_timeout = user_question(
        "What should the control interface timeout be (in seconds)?",
        &Some(DEFAULT_NATS_TIMEOUT.to_string()),
    )?;
    let ctl_lattice_prefix = user_question(
        "What is the control interface connection lattice prefix?",
        &Some(DEFAULT_LATTICE_PREFIX.to_string()),
    )?;
    let rpc_host = user_question(
        "What is the RPC host?",
        &Some(DEFAULT_NATS_HOST.to_string()),
    )?;
    let rpc_port = user_question(
        "What is the RPC connection port?",
        &Some(DEFAULT_NATS_PORT.to_string()),
    )?;
    let rpc_jwt = match user_question(
        "Enter your JWT that you use to authenticate to the RPC connection, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let rpc_seed = match user_question(
        "Enter your user seed that you use to authenticate to the RPC connection, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let rpc_credsfile = match user_question(
        "Enter the absolute path to RPC connection credsfile, if applicable",
        &Some("".to_string()),
    ) {
        Ok(s) if s.is_empty() => None,
        Ok(s) => Some(s),
        _ => None,
    };
    let rpc_timeout = user_question(
        "What should the RPC timeout be (in seconds)?",
        &Some(DEFAULT_NATS_TIMEOUT.to_string()),
    )?;
    let rpc_lattice_prefix = user_question(
        "What is the RPC connection lattice prefix?",
        &Some(DEFAULT_LATTICE_PREFIX.to_string()),
    )?;

    Ok(WashContext::new(
        name,
        cluster_seed,
        ctl_host,
        ctl_port.parse().unwrap_or_default(),
        ctl_jwt,
        ctl_seed,
        ctl_credsfile.map(PathBuf::from),
        ctl_timeout.parse()?,
        ctl_lattice_prefix,
        rpc_host,
        rpc_port.parse().unwrap_or_default(),
        rpc_jwt,
        rpc_seed,
        rpc_credsfile.map(PathBuf::from),
        rpc_timeout.parse()?,
        rpc_lattice_prefix,
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    // Enumerates all options of ctx subcommands to ensure
    // changes are not made to the ctx API
    fn test_ctx_comprehensive() {
        let new = CtxCli::from_iter_safe(&[
            "ctx",
            "new",
            "my_name",
            "--interactive",
            "--output",
            "json",
            "--directory",
            "./contexts",
        ])
        .unwrap();
        match new.command {
            CtxCommand::New(cmd) => {
                assert_eq!(cmd.directory.unwrap(), PathBuf::from("./contexts"));
                assert!(cmd.interactive);
                assert_eq!(cmd.output.kind, OutputKind::Json);
                assert_eq!(cmd.name.unwrap(), "my_name");
            }
            _ => panic!("ctx constructed incorrect command"),
        }

        let edit = CtxCli::from_iter_safe(&[
            "ctx",
            "edit",
            "my_context",
            "--editor",
            "vim",
            "--directory",
            "./contexts",
        ])
        .unwrap();
        match edit.command {
            CtxCommand::Edit(cmd) => {
                assert_eq!(cmd.directory.unwrap(), PathBuf::from("./contexts"));
                assert_eq!(cmd.editor, "vim");
                assert_eq!(cmd.name.unwrap(), "my_context");
            }
            _ => panic!("ctx constructed incorrect command"),
        }

        let del = CtxCli::from_iter_safe(&[
            "ctx",
            "del",
            "my_context",
            "--output",
            "text",
            "--directory",
            "./contexts",
        ])
        .unwrap();
        match del.command {
            CtxCommand::Del(cmd) => {
                assert_eq!(cmd.directory.unwrap(), PathBuf::from("./contexts"));
                assert_eq!(cmd.output.kind, OutputKind::Text);
                assert_eq!(cmd.name.unwrap(), "my_context");
            }
            _ => panic!("ctx constructed incorrect command"),
        }

        let list = CtxCli::from_iter_safe(&[
            "ctx",
            "list",
            "--output",
            "json",
            "--directory",
            "./contexts",
        ])
        .unwrap();
        match list.command {
            CtxCommand::List(cmd) => {
                assert_eq!(cmd.directory.unwrap(), PathBuf::from("./contexts"));
                assert_eq!(cmd.output.kind, OutputKind::Json);
            }
            _ => panic!("ctx constructed incorrect command"),
        }

        let default = CtxCli::from_iter_safe(&[
            "ctx",
            "default",
            "host_config",
            "--output",
            "text",
            "--directory",
            "./contexts",
        ])
        .unwrap();
        match default.command {
            CtxCommand::Default(cmd) => {
                assert_eq!(cmd.directory.unwrap(), PathBuf::from("./contexts"));
                assert_eq!(cmd.output.kind, OutputKind::Text);
                assert_eq!(cmd.name.unwrap(), "host_config");
            }
            _ => panic!("ctx constructed incorrect command"),
        }
    }
}
