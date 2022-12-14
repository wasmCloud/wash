//! ## Project generation from templates
//!
//! This module contains code for `wash new ...` commands
//! to creating a new project from a template.
//!
use std::{
    borrow::Borrow,
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use console::style;
use genconfig::{Config, CONFIG_FILE_NAME};
use indicatif::MultiProgress;
use project_variables::*;
use serde::Serialize;
use tempfile::TempDir;
use tokio::process::Command;
use weld_codegen::render::Renderer;

use crate::cli::CommandOutput;

pub mod emoji;
mod favorites;
mod genconfig;
mod git;
pub mod interactive;
pub mod project_variables;
mod template;

pub(crate) type TomlMap = std::collections::BTreeMap<String, toml::Value>;
pub(crate) type ParamMap = std::collections::BTreeMap<String, serde_json::Value>;
/// pattern for project name and identifier are the same:
/// start with letter, then letter/digit/underscore/dash
pub(crate) const PROJECT_NAME_REGEX: &str = r"^([a-zA-Z][a-zA-Z0-9_-]+)$";

/// Create a new project from template
#[derive(Debug, Clone, Subcommand)]
pub enum NewCliCommand {
    /// Generate actor project
    #[clap(name = "actor")]
    Actor(NewProjectArgs),

    /// Generate a new interface project
    #[clap(name = "interface")]
    Interface(NewProjectArgs),

    /// Generate a new capability provider project
    #[clap(name = "provider")]
    Provider(NewProjectArgs),
}

/// Type of project to be generated
#[derive(Debug, Clone, ValueEnum)]
pub(crate) enum ProjectKind {
    Actor,
    Interface,
    Provider,
}

impl From<&NewCliCommand> for ProjectKind {
    fn from(cmd: &NewCliCommand) -> ProjectKind {
        match cmd {
            NewCliCommand::Actor(_) => ProjectKind::Actor,
            NewCliCommand::Interface(_) => ProjectKind::Interface,
            NewCliCommand::Provider(_) => ProjectKind::Provider,
        }
    }
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ProjectKind::Actor => "actor",
                ProjectKind::Interface => "interface",
                ProjectKind::Provider => "provider",
            }
        )
    }
}

#[derive(Args, Debug, Default, Clone)]
pub struct NewProjectArgs {
    /// Project name
    #[clap(help = "Project name")]
    pub(crate) project_name: Option<String>,

    /// Github repository url. Requires 'git' to be installed in PATH.
    #[clap(long)]
    pub(crate) git: Option<String>,

    /// Optional subfolder of the git repository
    #[clap(long, alias = "subdir")]
    pub(crate) subfolder: Option<String>,

    /// Optional github branch. Defaults to "main"
    #[clap(long)]
    pub(crate) branch: Option<String>,

    /// Optional path for template project (alternative to --git)
    #[clap(short, long)]
    pub(crate) path: Option<PathBuf>,

    /// Optional path to file containing placeholder values
    #[clap(short, long)]
    pub(crate) values: Option<PathBuf>,

    /// Silent - do not prompt user. Placeholder values in the templates
    /// will be resolved from a '--values' file and placeholder defaults.
    #[clap(long)]
    pub(crate) silent: bool,

    /// Favorites file - to use for project selection
    #[clap(long)]
    pub(crate) favorites: Option<PathBuf>,

    /// Template name - name of template to use
    #[clap(short, long)]
    pub(crate) template_name: Option<String>,

    /// Don't run 'git init' on the new folder
    #[clap(long)]
    pub(crate) no_git_init: bool,
}

pub async fn handle_command(command: NewCliCommand) -> Result<CommandOutput> {
    validate(&command)?;

    let kind = ProjectKind::from(&command);
    let cmd = match command {
        NewCliCommand::Actor(gc) | NewCliCommand::Interface(gc) | NewCliCommand::Provider(gc) => gc,
    };

    // if user did not specify path to template dir or path to git repo,
    // pick one of the favorites for this kind
    let cmd = if cmd.path.is_none() && cmd.git.is_none() {
        let fav = favorites::pick_favorite(
            cmd.favorites.as_ref(),
            &kind,
            cmd.silent,
            cmd.template_name.as_ref(),
        )?;
        NewProjectArgs {
            path: fav.path.as_ref().map(PathBuf::from),
            git: fav.git,
            branch: fav.branch,
            subfolder: fav.subfolder,
            ..cmd
        }
    } else {
        cmd
    };

    make_project(kind, cmd).await?;
    Ok(CommandOutput::default())
}

fn validate(command: &NewCliCommand) -> Result<()> {
    let cmd = match command {
        NewCliCommand::Actor(gc) | NewCliCommand::Interface(gc) | NewCliCommand::Provider(gc) => gc,
    };

    if cmd.path.is_some() && (cmd.git.is_some() || cmd.subfolder.is_some() || cmd.branch.is_some())
    {
        return Err(anyhow!("Error in 'new {}' options: You may use --path or --git ( --branch, --subfolder ) to specify a template source, but not both. If neither is specified, you will be prompted to select a project template.",
            &ProjectKind::from(command)
        ));
    }
    if let Some(name) = &cmd.project_name {
        crate::generate::project_variables::validate_project_name(name)?;
    }
    if let Some(path) = &cmd.path {
        if !path.is_dir() {
            return Err(anyhow!(
                "Error in --path option: '{}' is not an existing directory",
                &path.display()
            ));
        }
    }
    if let Some(path) = &cmd.values {
        if !path.is_file() {
            return Err(anyhow!(
                "Error in --values option: '{}' is not an existing file",
                &path.display()
            ));
        }
    }
    if let Some(path) = &cmd.favorites {
        if !path.is_file() {
            return Err(anyhow!(
                "Error in --favorites option: '{}' is not an existing file",
                &path.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn any_msg(s1: &str, s2: &str) -> anyhow::Error {
    anyhow!(
        "{} {} {}",
        emoji::ERROR,
        style(s1).bold().red(),
        style(s2).bold().red()
    )
}

pub(crate) fn any_warn(s: &str) -> anyhow::Error {
    anyhow!("{} {}", emoji::WARN, style(s).bold().red())
}

pub(crate) async fn make_project(
    kind: ProjectKind,
    args: NewProjectArgs,
) -> std::result::Result<(), anyhow::Error> {
    // load optional values file
    let mut values = if let Some(values_file) = &args.values {
        let bytes = fs::read(values_file)
            .with_context(|| format!("reading values file {}", &values_file.display()))?;
        let tm = toml::from_slice::<TomlMap>(&bytes)
            .with_context(|| format!("parsing values file {}", &values_file.display()))?;
        if let Some(toml::Value::Table(values)) = tm.get("values") {
            toml_to_json(values)?
        } else {
            ParamMap::default()
        }
    } else {
        ParamMap::default()
    };

    let project_name =
        resolve_project_name(&values.get("project-name"), &args.project_name.as_ref())?;
    values.insert(
        "project-name".into(),
        project_name.user_input.clone().into(),
    );
    values.insert(
        "project-type".into(),
        serde_json::Value::String(kind.to_string()),
    );
    let project_dir = resolve_project_dir(&project_name)?;

    // select the template from args or a favorite file,
    // and copy its contents into a local folder
    let (template_base_dir, template_folder) = prepare_local_template(&args).await?;

    // read configuration file `project-generate.toml` from template.
    let project_config_path = fs::canonicalize(
        locate_project_config_file(CONFIG_FILE_NAME, &template_base_dir, &args.subfolder)
            .with_context(|| {
                format!(
                    "Invalid template folder: Required configuration file `{}` is missing.",
                    CONFIG_FILE_NAME
                )
            })?,
    )?;
    let mut config = Config::from_path(&project_config_path)?;
    // prevent copying config file to project dir by adding it to the exclude list
    config.exclude(
        if project_config_path.starts_with(&template_folder) {
            project_config_path.strip_prefix(&template_folder)?
        } else {
            &project_config_path
        }
        .to_string_lossy()
        .to_string(),
    );

    // resolve all project values, prompting if necessary,
    // and expanding templates in default values
    let renderer = Renderer::default();
    let undefined = fill_project_variables(&config, &mut values, &renderer, args.silent, |slot| {
        crate::generate::interactive::variable(slot)
    })?;
    if !undefined.is_empty() {
        return Err(any_msg("The following variables were not defined. Either add them to the --values file, or disable --silent: {}",
            &undefined.join(",")
        ));
    }

    println!(
        "{} {} {}",
        emoji::WRENCH,
        style("Generating template").bold(),
        style("...").bold()
    );

    let template_config = config.template.unwrap_or_default();
    let mut pbar = MultiProgress::new();
    template::process_template_dir(
        &template_folder,
        &project_dir,
        &template_config,
        &renderer,
        &values,
        &mut pbar,
    )
    .map_err(|e| any_msg("generating project from templates:", &e.to_string()))?;

    if !args.no_git_init {
        let cmd_out = Command::new("git")
            .args(["init", "--initial-branch", "main", "."])
            .current_dir(tokio::fs::canonicalize(&project_dir).await?)
            .spawn()?
            .wait_with_output()
            .await?;
        if !cmd_out.status.success() {
            return Err(anyhow!(
                "git init error: {}",
                String::from_utf8_lossy(&cmd_out.stderr)
            ));
        }
    }

    pbar.clear().ok();

    println!(
        "{} {} {} {}",
        emoji::SPARKLE,
        style("Done!").bold().green(),
        style("New project created").bold(),
        style(&project_dir.display()).underlined()
    );
    Ok(())
}

// convert from TOML map to JSON map
fn toml_to_json<T: Serialize>(map: &T) -> Result<ParamMap> {
    let s = serde_json::to_string(map)?;
    let value: ParamMap = serde_json::from_str(&s)?;
    Ok(value)
}

/// Finds template configuration in subfolder or a parent.
/// Returns error if no configuration was found
fn locate_project_config_file<T>(
    name: &str,
    template_folder: T,
    subfolder: &Option<String>,
) -> Result<PathBuf>
where
    T: AsRef<Path>,
{
    let template_folder = template_folder.as_ref().to_path_buf();
    let mut search_folder = subfolder
        .as_ref()
        .map_or_else(|| template_folder.to_owned(), |s| template_folder.join(s));
    loop {
        let file_path = search_folder.join(name.borrow());
        if file_path.exists() {
            return Ok(file_path);
        }
        if search_folder == template_folder {
            return Err(any_msg("File not found within template", ""));
        }
        search_folder = search_folder
            .parent()
            .ok_or_else(|| {
                any_msg(
                    "Missing Config:",
                    &format!(
                        "did not find {} in {} or any of its parents.",
                        &search_folder.display(),
                        CONFIG_FILE_NAME
                    ),
                )
            })?
            .to_path_buf();
    }
}

pub(crate) async fn prepare_local_template(args: &NewProjectArgs) -> Result<(TempDir, PathBuf)> {
    let (template_base_dir, template_folder) = match (&args.git, &args.path) {
        (Some(url), None) => {
            let template_base_dir = tempfile::tempdir()
                .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;
            git::clone_git_template(git::CloneTemplate {
                clone_tmp: template_base_dir.path().to_path_buf(),
                repo_url: url.to_string(),
                sub_folder: args.subfolder.clone(),
                repo_branch: args.branch.clone().unwrap_or_else(|| "main".to_string()),
            })
            .await?;
            let template_folder = resolve_template_dir(&template_base_dir, args)?;
            (template_base_dir, template_folder)
        }
        (None, Some(_)) => {
            let template_base_dir = copy_path_template_into_temp(args)?;
            let template_folder = template_base_dir.path().into();
            (template_base_dir, template_folder)
        }
        _ => {
            return Err(anyhow!(
                "{} {} {} {}",
                style("Please specify either").bold(),
                style("--git <repo>").bold().yellow(),
                style("or").bold(),
                style("--path <path>").bold().yellow(),
            ))
        }
    };
    Ok((template_base_dir, template_folder))
}

fn resolve_template_dir(template_base_dir: &TempDir, args: &NewProjectArgs) -> Result<PathBuf> {
    match &args.subfolder {
        Some(subfolder) => {
            let template_base_dir = fs::canonicalize(template_base_dir.path())
                .map_err(|e| any_msg("Invalid template path:", &e.to_string()))?;
            let mut template_dir = template_base_dir.clone();
            // NOTE(thomastaylor312): Yeah, this is weird, but if you just `join` the PathBuf here
            // then you end up with mixed slashes, which doesn't work when file paths are
            // canonicalized on Windows
            template_dir.extend(PathBuf::from(subfolder).iter());
            let template_dir = fs::canonicalize(template_dir)
                .map_err(|e| any_msg("Invalid subfolder path:", &e.to_string()))?;

            if !template_dir.starts_with(&template_base_dir) {
                return Err(any_msg(
                    "Subfolder Error:",
                    "Invalid subfolder. Must be part of the template folder structure.",
                ));
            }
            if !template_dir.is_dir() {
                return Err(any_msg(
                    "Subfolder Error:",
                    "The specified subfolder must be a valid folder.",
                ));
            }

            println!(
                "{} {} `{}`{}",
                emoji::WRENCH,
                style("Using template subfolder").bold(),
                style(subfolder).bold().yellow(),
                style("...").bold()
            );
            Ok(template_dir)
        }
        None => Ok(template_base_dir.path().to_owned()),
    }
}

fn copy_path_template_into_temp(args: &NewProjectArgs) -> Result<TempDir> {
    let path_clone_dir = tempfile::tempdir()
        .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;
    // args.path is already Some() when we get here
    let path = args.path.as_ref().unwrap();
    if !path.is_dir() {
        return Err(any_msg(&format!("template path {} not found - please try another template or fix the favorites path", &path.display()),""));
    }
    copy_dir_all(path, path_clone_dir.path())
        .with_context(|| format!("copying template project from {}", &path.display()))?;
    Ok(path_clone_dir)
}

pub(crate) fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    fn check_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
        if !dst.as_ref().exists() {
            return Ok(());
        }

        for src_entry in fs::read_dir(src)? {
            let src_entry = src_entry?;
            let dst_path = dst.as_ref().join(src_entry.file_name());
            let entry_type = src_entry.file_type()?;

            if entry_type.is_dir() {
                check_dir_all(src_entry.path(), dst_path)?;
            } else if entry_type.is_file() {
                if dst_path.exists() {
                    return Err(any_msg(
                        "File already exists:",
                        &dst_path.display().to_string(),
                    ));
                }
            } else {
                return Err(any_warn("Symbolic links not supported"));
            }
        }
        Ok(())
    }
    fn copy_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
        fs::create_dir_all(&dst)?;
        for src_entry in fs::read_dir(src)? {
            let src_entry = src_entry?;
            let dst_path = dst.as_ref().join(src_entry.file_name());
            let entry_type = src_entry.file_type()?;
            if entry_type.is_dir() {
                copy_dir_all(src_entry.path(), dst_path)?;
            } else if entry_type.is_file() {
                fs::copy(src_entry.path(), dst_path)?;
            }
        }
        Ok(())
    }

    check_dir_all(&src, &dst)?;
    copy_all(src, dst)
}

pub(crate) fn resolve_project_dir(name: &ProjectName) -> Result<PathBuf> {
    let dir_name = name.kebab_case();

    let project_dir = std::env::current_dir()
        .unwrap_or_else(|_e| ".".into())
        .join(dir_name);

    if project_dir.exists() {
        Err(any_msg("Target directory already exists.", "aborting!"))
    } else {
        Ok(project_dir)
    }
}

fn resolve_project_name(
    value: &Option<&serde_json::Value>,
    arg: &Option<&String>,
) -> Result<ProjectName> {
    match (value, arg) {
        (_, Some(arg_name)) => Ok(ProjectName::new(arg_name.as_str())),
        (Some(serde_json::Value::String(val_name)), _) => Ok(ProjectName::new(val_name)),
        _ => Ok(ProjectName::new(interactive::name()?)),
    }
}

/// Stores user inputted name and provides convenience methods
/// for handling casing.
pub(crate) struct ProjectName {
    pub(crate) user_input: String,
}

impl ProjectName {
    pub(crate) fn new(name: impl Into<String>) -> ProjectName {
        ProjectName {
            user_input: name.into(),
        }
    }

    pub(crate) fn kebab_case(&self) -> String {
        use heck::ToKebabCase as _;
        self.user_input.to_kebab_case()
    }
}