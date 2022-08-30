//! Generate shell completion files
//!
use crate::CommandOutput;
use anyhow::anyhow;
use anyhow::Result;
use clap::{Args, Subcommand};
use clap_complete::{generator::generate_to, shells::Shell};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Args)]
pub(crate) struct CompletionOpts {
    /// Output directory (default '.')
    #[clap(short = 'd', long = "dir")]
    dir: Option<PathBuf>,

    /// Shell
    #[clap(name = "shell", subcommand)]
    shell: ShellSelection,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum ShellSelection {
    /// generate completions for Zsh
    Zsh,
    /// generate completions for Bash
    Bash,
    /// generate completions for Fish
    Fish,
    /// generate completions for PowerShell
    PowerShell,
}

pub(crate) fn handle_command(
    opts: CompletionOpts,
    mut command: clap::builder::Command<'_>,
) -> Result<CommandOutput> {
    let output_dir = opts.dir.unwrap_or_else(|| PathBuf::from("."));

    let shell = match opts.shell {
        ShellSelection::Zsh => Shell::Zsh,
        ShellSelection::Bash => Shell::Bash,
        ShellSelection::Fish => Shell::Fish,
        ShellSelection::PowerShell => Shell::PowerShell,
    };

    match generate_to(shell, &mut command, "wash", &output_dir) {
        Ok(path) => {
            let mut map = HashMap::new();
            map.insert(
                "path".to_string(),
                path.to_string_lossy().to_string().into(),
            );
            Ok(CommandOutput::new(
                format!("Generated completion file: {}", path.display()),
                map,
            ))
        }
        Err(e) => Err(anyhow!(
            "generating shell completion file in folder '{}': {}",
            output_dir.display(),
            e
        )),
    }
}
