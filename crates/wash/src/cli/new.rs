//! CLI command for creating new component projects

use anyhow::{Context, bail};
use clap::Args;
use dialoguer::{Confirm, FuzzySelect, theme::ColorfulTheme};
use serde_json::json;
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, info, instrument, trace, warn};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::Config,
    new::{NewTemplate, TemplateLanguage, clone_template, copy_dir_recursive, extract_subfolder},
};

/// Create a new component project from a template, git repository, or local path
#[derive(Args, Debug, Clone)]
pub struct NewCommand {
    #[clap(
        help = "Project name and local directory to create, defaults to repository / subfolder name"
    )]
    name: Option<String>,

    /// Named template from configuration file
    #[clap(long, conflicts_with_all = &["git", "local"])]
    template_name: Option<String>,

    /// Git repository URL to use as template
    #[clap(long, conflicts_with_all = &["template_name", "local"])]
    git: Option<String>,

    /// Subdirectory within the git repository to use (only valid with --git)
    #[clap(long, requires = "git")]
    subfolder: Option<String>,

    /// Local filesystem path to use as template  
    #[clap(long, conflicts_with_all = &["template_name", "git"])]
    local: Option<PathBuf>,
}

impl CliCommand for NewCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "new")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let config = ctx.ensure_config(None).unwrap_or_else(|e| {
            warn!(error = %e, "Failed to load global configuration");
            Config::default()
        });

        // Determine template source from CLI arguments
        if let Ok(template_source) = self.get_template_source(&config) {
            // Determine project name and output directory
            let project_name = self.get_project_name(&template_source)?;
            let output_dir = PathBuf::from(&project_name);

            info!(
                "Creating new project '{}' from template: {}",
                project_name,
                self.describe_template_source(&template_source)
            );

            // Clone/copy template based on source type
            self.process_template(ctx, &template_source, &output_dir)
                .await?;

            Ok(CommandOutput::ok(
                format!(
                    "Project '{project_name}' created successfully at {}",
                    output_dir.display()
                ),
                None,
            ))
        } else {
            trace!("no template, git repository, or local path specified, prompting user");
            self.interactive_select(ctx, &config).await
        }
    }
}

impl NewCommand {
    async fn interactive_select(
        &self,
        ctx: &CliContext,
        config: &Config,
    ) -> anyhow::Result<CommandOutput> {
        let theme = ColorfulTheme::default();
        let language = FuzzySelect::with_theme(&theme)
            .with_prompt("What programming language do you want to use?")
            .items(&["Any", "Rust", "TinyGo", "TypeScript"])
            .default(0)
            .interact()
            .context("failed to prompt for template selection")?;
        let language = match language {
            0 => None, // Any language
            1 => Some(TemplateLanguage::Rust),
            2 => Some(TemplateLanguage::TinyGo),
            3 => Some(TemplateLanguage::TypeScript),
            _ => None, // Fallback case, should not happen
        };

        trace!(?language, "user selected language");

        let filtered_templates: Vec<NewTemplate> = config
            .templates
            .iter()
            .filter(|t| {
                if let Some(lang) = &language {
                    &t.language == lang
                } else {
                    true // Include all languages if "Any" is selected
                }
            })
            .cloned()
            .collect();

        let selection = {
            let items = filtered_templates
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>();

            if items.is_empty() {
                bail!("No templates available for the selected language");
            }

            FuzzySelect::with_theme(&theme)
                .with_prompt("Which template would you like to use?")
                .items(&items)
                .default(0)
                .interact()
                .context("failed to prompt for template selection")?
        };
        trace!(selection, "user selected template");

        let template = filtered_templates
            .get(selection)
            .ok_or_else(|| anyhow::anyhow!("no template selected"))?;

        Confirm::with_theme(&theme)
            .with_prompt(format!(
                "You selected the template '{}' ({}). Do you want to proceed?",
                template.name, template.repository
            ))
            .default(true)
            .interact()
            .context("failed to confirm template selection")?;

        let output_dir = PathBuf::from(&template.name);
        self.process_template(ctx, &template.into(), &output_dir)
            .await
            .context("failed to process selected template")?;

        Ok(CommandOutput::ok(
            format!(
                "Successfully created project from template {template_name}",
                template_name = template.name
            ),
            Some(json!({
                "name": template.name,
                "repository": template.repository,
                "language": template.language,
                "output_dir": output_dir
            })),
        ))
    }

    /// Determine the template source from CLI arguments
    fn get_template_source(&self, config: &Config) -> anyhow::Result<TemplateSource> {
        match (&self.template_name, &self.git, &self.local) {
            (Some(name), None, None) => {
                // Find named template in config
                let template = config
                    .templates
                    .iter()
                    .find(|t| t.name == *name)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Template '{name}' not found in configuration")
                    })?;

                Ok(TemplateSource::Named(template))
            }
            (None, Some(url), None) => Ok(TemplateSource::Git {
                url: url.clone(),
                subfolder: self.subfolder.clone(),
                git_ref: None,
            }),
            (None, None, Some(path)) => Ok(TemplateSource::Local(path.clone())),
            (None, None, None) => Err(anyhow::anyhow!(
                "One of --template-name, --git, or --local must be specified"
            )),
            _ => Err(anyhow::anyhow!(
                "Only one of --template-name, --git, or --local can be specified"
            )),
        }
    }

    /// Get project name, with fallback logic
    fn get_project_name(&self, template_source: &TemplateSource) -> anyhow::Result<String> {
        if let Some(ref name) = self.name {
            return Ok(name.clone());
        }

        // Generate project name from template source
        match template_source {
            TemplateSource::Named(template) => Ok(template.name.clone()),
            TemplateSource::Git { url, subfolder, .. } => {
                if let Some(subfolder) = subfolder {
                    Ok(subfolder
                        .split('/')
                        .next_back()
                        .unwrap_or("new-project")
                        .to_string())
                } else {
                    Ok(url
                        .split('/')
                        .next_back()
                        .unwrap_or("new-project")
                        .trim_end_matches(".git")
                        .to_string())
                }
            }
            TemplateSource::Local(path) => Ok(path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("new-project")
                .to_string()),
        }
    }

    /// Describe template source for logging
    fn describe_template_source(&self, template_source: &TemplateSource) -> String {
        match template_source {
            TemplateSource::Named(repo) => {
                format!("named template ({repo_name})", repo_name = repo.name)
            }
            TemplateSource::Git {
                url,
                subfolder,
                git_ref,
            } => {
                let mut desc = if let Some(subfolder) = subfolder {
                    format!("git repository {url} (subfolder: {subfolder})")
                } else {
                    format!("git repository {url}")
                };
                if let Some(git_ref) = git_ref {
                    desc.push_str(&format!(" (ref: {git_ref})"));
                }
                desc
            }
            TemplateSource::Local(path) => format!("local path {path}", path = path.display()),
        }
    }

    /// Process template based on source type
    #[instrument(level = "debug", skip_all)]
    async fn process_template(
        &self,
        ctx: &CliContext,
        template_source: &TemplateSource,
        output_dir: &PathBuf,
    ) -> anyhow::Result<()> {
        match template_source {
            TemplateSource::Named(NewTemplate {
                repository: url,
                subfolder: None,
                git_ref,
                ..
            })
            | TemplateSource::Git {
                url,
                subfolder: None,
                git_ref,
            } => clone_template(url, output_dir, git_ref.as_deref()).await,
            TemplateSource::Named(NewTemplate {
                repository: url,
                subfolder: Some(subfolder),
                git_ref,
                ..
            })
            | TemplateSource::Git {
                url,
                subfolder: Some(subfolder),
                git_ref,
            } => {
                clone_template(url, output_dir, git_ref.as_deref()).await?;
                extract_subfolder(ctx, output_dir, subfolder).await
            }
            TemplateSource::Local(path) => {
                self.copy_local_template(&path.to_string_lossy(), output_dir)
                    .await
            }
        }
    }

    /// Copy a local directory as template
    #[instrument(level = "debug", skip_all)]
    async fn copy_local_template(
        &self,
        template_path: &str,
        output_dir: &PathBuf,
    ) -> anyhow::Result<()> {
        debug!(
            "Copying local template from: {} to: {}",
            template_path,
            output_dir.display()
        );

        if fs::metadata(output_dir).await.is_ok() {
            bail!(
                "Output directory already exists: {output_dir}",
                output_dir = output_dir.display()
            );
        }

        copy_dir_recursive(template_path, output_dir).await?;

        info!(
            "Successfully copied local template to {}",
            output_dir.display()
        );
        Ok(())
    }
}

/// Template source specification
#[derive(Debug, Clone)]
pub enum TemplateSource {
    /// Named template from configuration file
    Named(NewTemplate),
    /// Git repository with optional subfolder and git ref
    Git {
        url: String,
        subfolder: Option<String>,
        git_ref: Option<String>,
    },
    /// Local filesystem path
    Local(PathBuf),
}

// TODO: support file templates?
// All templates are git repositories for now.
impl From<&NewTemplate> for TemplateSource {
    fn from(source: &NewTemplate) -> Self {
        TemplateSource::Git {
            url: source.repository.to_owned(),
            subfolder: source.subfolder.to_owned(),
            git_ref: source.git_ref.to_owned(),
        }
    }
}
