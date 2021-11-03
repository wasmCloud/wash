use crate::util::Result;
use crate::util::{
    convert_error, convert_rpc_error, extract_arg_value, format_output, json_str_to_msgpack_bytes,
    msgpack_to_json_val, nats_client_from_opts, Output, OutputKind,
};
use serde_json::json;
use std::fs::File;
use std::io::Error;
use std::io::{Read, Write};
use std::ops::RangeBounds;
use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;
mod context;
use crate::generate::interactive::prompt_for_choice;
use context::{IndexJson, WashContext};

const INDEX_JSON: &str = "index.json";

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
        New(cmd) => handle_new(cmd),
        _ => Ok("you didn't add the match arm yet".to_string()),
    }
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) enum CtxCommand {
    /// Show all stored contexts
    #[structopt(name = "list")]
    List(ListCommand),
    /// Delete a context
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
    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct NewCommand {
    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Name of the context, must be a valid filename
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
    #[structopt(flatten)]
    pub(crate) output: Output,
}

fn handle_list(cmd: ListCommand) -> Result<String> {
    let dir = context_dir(cmd.directory)?;
    let index = get_index(dir.clone())?;
    let contexts: Vec<String> = get_contexts(dir.clone())?
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .unwrap_or_default()
                .to_os_string()
                .into_string()
                .ok()
        })
        .collect();

    let output_contexts = if cmd.output.kind == OutputKind::Text {
        contexts
            .iter()
            .map(|f| {
                //TODO: calling to_string again? nono
                if f == index.default() {
                    //TODO: Bold?
                    format!("{} (default)", f)
                } else {
                    f.to_string()
                }
            })
            .collect()
    } else {
        contexts
    };

    Ok(format_output(
        format!(
            "====== Contexts found in {} ======\n{}",
            dir.display(),
            output_contexts.join("\n")
        ),
        json!({ "contexts": output_contexts, "default": index.default() }),
        &cmd.output.kind,
    ))
}

fn handle_default(cmd: DefaultCommand) -> Result<String> {
    let dir = context_dir(cmd.directory)?;

    let contexts: Vec<String> = get_contexts(dir.clone())?
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .unwrap_or_default()
                .to_os_string()
                .into_string()
                .ok()
        })
        .collect();

    let new_default = if let Some(default_name) = cmd.name {
        default_name
    } else {
        let default = get_index(dir.clone()).ok().map(|i| i.default().to_owned());
        println!("default context: {:?}", default);

        let entry = crate::generate::project_variables::StringEntry {
            default,
            choices: Some(contexts.clone()),
            regex: None,
        };
        let choice = prompt_for_choice(&entry, "Select a default context:")?;
        contexts
            .get(choice)
            .map(|c| c.to_string())
            .unwrap_or_default()
    };

    if contexts.contains(&new_default) && set_default_context(dir, new_default).is_ok() {
        Ok("New default set".to_string())
    } else {
        Ok("failed to set new default".to_string())
    }
}

fn handle_new(cmd: NewCommand) -> Result<String> {
    //TODO: if this line of code, the dir initializer, is at the start, just do it in handle_command
    let dir = context_dir(cmd.directory.clone())?;
    let default_context = WashContext::default();

    //TODO: sanitize cmd.name
    let filename = format!("{}.json", cmd.name);
    let context_path = dir.join(filename);
    serde_json::to_writer(&File::create(context_path)?, &default_context)
        .map(|_| format!("Created context {} with default values", cmd.name).to_string())
        .map_err(|e| e.into())
}

fn get_index(context_dir: PathBuf) -> Result<IndexJson> {
    let mut buf = vec![];
    let mut f = std::fs::File::open(context_dir.join(INDEX_JSON))?;
    f.read_to_end(&mut buf)?;
    serde_json::from_slice::<IndexJson>(&buf).map_err(|e| e.into())
}

/// Loads the default context, according to index.json, into a WashContext object
pub(crate) fn get_default_context(context_dir: PathBuf) -> Result<WashContext> {
    let index = get_index(context_dir.clone())?;
    load_context(context_dir.join(format!("{}.json", index.default())))
}

fn set_default_context(context_dir: PathBuf, default_context: String) -> Result<()> {
    let index = IndexJson::new(default_context);
    let context_path = context_dir.join(INDEX_JSON);
    serde_json::to_writer(&File::create(context_path)?, &index).map_err(|e| e.into())
}

fn load_context(context_path: PathBuf) -> Result<WashContext> {
    let mut buf = vec![];
    let mut f = std::fs::File::open(context_path)?;
    f.read_to_end(&mut buf)?;
    serde_json::from_slice::<WashContext>(&buf).map_err(|e| e.into())
}

fn get_contexts(context_dir: PathBuf) -> Result<Vec<PathBuf>> {
    let paths = std::fs::read_dir(context_dir.clone()).map_err(|e| {
        format!(
            "Error: {}, please ensure directory {} exists",
            e,
            context_dir.display()
        )
    })?;

    let index = std::ffi::OsString::from(INDEX_JSON);
    Ok(paths
        .filter_map(|path| {
            // TODO: Clean this up a bit, variables confusing
            if let Ok(dir_entry) = path {
                let path = dir_entry.path();
                let filename = dir_entry.file_name();
                match path.extension().map(|os| os.to_str()).unwrap_or_default() {
                    // Don't include index in the list of contexts
                    Some("json") if filename == index => None,
                    Some("json") => Some(path),
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect())
}

fn context_dir(cmd_dir: Option<PathBuf>) -> Result<PathBuf> {
    Ok(if let Some(dir) = cmd_dir {
        dir
    } else {
        dirs::home_dir()
            .ok_or_else(|| {
                Error::new(
                    std::io::ErrorKind::NotFound,
                    "$HOME not found, please set $HOME or $WASH_KEYS for autogenerated keys",
                )
            })?
            .join(".wash/contexts")
    })
}

// #[cfg(test)]
// mod test {
// }
