use crate::generate::{interactive::prompt_for_choice, project_variables::StringEntry};
use crate::util::{format_output, Output, OutputKind, Result};
use serde_json::json;
use std::fs::File;
use std::io::{BufReader, Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::Command;
use structopt::clap::AppSettings;
use structopt::StructOpt;
pub mod context;
use context::{DefaultContext, WashContext};

const INDEX_JSON: &str = "index.json";
const CTX_DIR: &str = ".wash/contexts";

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

pub(crate) async fn handle_command(ctxcmd: CtxCommand) -> Result<String> {
    use CtxCommand::*;
    match ctxcmd {
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

    /// Name of the context, must be a valid filename
    #[structopt(name = "name")]
    pub(crate) name: String,

    /// Location of context files for managing. Defaults to $WASH_CONTEXTS ($HOME/.wash/contexts)
    #[structopt(long = "directory", env = "WASH_CONTEXTS", hide_env_values = true)]
    directory: Option<PathBuf>,
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
            "====== Contexts found in {} ======\n{}",
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
    let default_context = WashContext::default();

    let filename = format!("{}.json", cmd.name);
    let options = sanitize_filename::Options {
        truncate: true,
        windows: true,
        replacement: "_",
    };

    // Ensure filename doesn't include uphill/downhill\ slashes, or reserved prefixes
    let sanitized = sanitize_filename::sanitize_with_options(filename, options);
    let context_path = dir.join(sanitized.clone());
    let res = serde_json::to_writer(&File::create(context_path)?, &default_context)
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
    let index = get_index(context_dir)?;
    load_context(&context_path_from_name(context_dir, &index.name))
}

/// Load a file and return a Result containing the WashContext object.
pub(crate) fn load_context(context_path: &Path) -> Result<WashContext> {
    let file = std::fs::File::open(context_path)?;
    let reader = BufReader::new(file);
    let ctx = serde_json::from_reader(reader)?;
    Ok(ctx)
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
        dirs::home_dir()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::NotFound,
                    "Context directory not found, please set $HOME or $WASH_CONTEXTS for managed contexts",
                )
            })?
            .join(CTX_DIR)
    };
    // Ensure user supplied context exists
    if std::fs::metadata(&dir).is_err() {
        let _ = std::fs::create_dir(&dir);
    }
    Ok(dir)
}

/// Helper function to properly format the path to a context JSON file
fn context_path_from_name(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.json", name))
}

// #[cfg(test)]
// mod test {
// }
