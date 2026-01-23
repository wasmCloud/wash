//! CLI command for creating new component projects from git repositories

use anyhow::{Context, bail};
use clap::Args;
use serde_json::json;
use tracing::{info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    new::{clone_template, extract_subfolder},
};

/// Create a new component project from a git repository
#[derive(Args, Debug, Clone)]
pub struct NewCommand {
    /// Git repository URL to clone
    #[clap(help = "Git repository URL to use as project template")]
    git: String,

    /// Project name and local directory to create (defaults to repository/subfolder name)
    #[clap(long)]
    name: Option<String>,

    /// Subdirectory within the git repository to use
    #[clap(long)]
    subfolder: Option<String>,

    /// Git reference (branch, tag, or commit) to checkout
    #[clap(long, name = "ref")]
    git_ref: Option<String>,
}

impl CliCommand for NewCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "new")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let project_name = self.get_project_name();
        // Explicitly use project_dir from context instead of relying on working directory
        let output_dir = ctx.project_dir().join(&project_name);

        if output_dir.exists() {
            bail!("Output directory already exists: {}", output_dir.display());
        }

        info!(
            "Creating new project '{}' from git repository: {}",
            project_name, self.git
        );

        // Clone the repository
        clone_template(&self.git, &output_dir, self.git_ref.as_deref())
            .await
            .context("failed to clone git repository")?;

        // Extract subfolder if specified
        if let Some(subfolder) = &self.subfolder {
            extract_subfolder(ctx, &output_dir, subfolder)
                .await
                .context("failed to extract subfolder")?;
        }

        Ok(CommandOutput::ok(
            format!(
                "Project '{project_name}' created successfully at {}",
                output_dir.display()
            ),
            Some(json!({
                "name": project_name,
                "repository": self.git,
                "subfolder": self.subfolder,
                "output_dir": output_dir,
            })),
        ))
    }
}

impl NewCommand {
    /// Get project name from CLI args or derive from repository/subfolder
    fn get_project_name(&self) -> String {
        if let Some(ref name) = self.name {
            return name.clone();
        }

        // Try to derive name from subfolder first, then from repository URL
        if let Some(subfolder) = &self.subfolder {
            subfolder
                .split('/')
                .next_back()
                .unwrap_or("new-project")
                .to_string()
        } else {
            self.git
                .split('/')
                .next_back()
                .unwrap_or("new-project")
                .trim_end_matches(".git")
                .to_string()
        }
    }
}
