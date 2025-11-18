use anyhow::Context as _;
use clap::Subcommand;
use tracing::instrument;
use tracing::{error, info, warn};
use walkdir::WalkDir;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{generate_default_config, local_config_path},
};
use std::io::{self};
use std::path::{Path, PathBuf};

#[instrument(skip(dir), fields(path = %dir.display()))]
fn get_all_paths_in_dir(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path() != dir)
        .map(|entry| entry.into_path())
        .collect();

    Ok(paths)
}
/// Create a new component project from a template, git repository, or local path
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Initialize a new configuration file for wash
    Init {
        #[clap(long)]
        /// Overwrite existing configuration
        force: bool,
        #[clap(long)]
        /// Overwrite global configuration instead of project
        global: bool,
    },
    /// Print the current version and local directories used by wash
    Info {},
    /// Print the current configuration file for wash
    Show {},
    // TODO(#27): validate config command
    /// Clean up wash directories and cached data
    #[clap(group = clap::ArgGroup::new("cleanup_targets")
        .required(true)
        .multiple(true))]
    Cleanup {
        /// Remove config directory
        #[clap(long, group = "cleanup_targets")]
        config: bool,
        /// Remove cache directory
        #[clap(long, group = "cleanup_targets")]
        cache: bool,
        /// Remove data directory
        #[clap(long, group = "cleanup_targets")]
        data: bool,
        /// Remove all wash directories (config + cache + data)
        #[clap(long, group = "cleanup_targets")]
        all: bool,
        /// Show what would be removed without actually deleting
        #[clap(long)]
        dry_run: bool,
    },
}

impl CliCommand for ConfigCommand {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            ConfigCommand::Init { force, global } => {
                let config_path = if *global {
                    ctx.config_path()
                } else {
                    local_config_path(
                        &std::env::current_dir().context("failed to get current dir")?,
                    )
                };

                // Global configuration should include templates, but local shouldn't by default
                generate_default_config(&config_path, *force, *global)
                    .await
                    .context("failed to initialize config")?;

                Ok(CommandOutput::ok(
                    "Configuration initialized successfully.".to_string(),
                    Some(serde_json::json!({
                        "message": "Configuration initialized successfully.",
                        "success": true,
                    })),
                ))
            }
            ConfigCommand::Info {} => {
                let version = env!("CARGO_PKG_VERSION");
                let data_dir = ctx.data_dir().display().to_string();
                let cache_dir = ctx.cache_dir().display().to_string();
                let config_dir = ctx.config_dir().display().to_string();
                let config_path = ctx.config_path().display().to_string();

                Ok(CommandOutput::ok(
                    format!(
                        "wash version: {version}\nData directory: {data_dir}\nCache directory: {cache_dir}\nConfig directory: {config_dir}\nConfig path: {config_path}"
                    ),
                    Some(serde_json::json!({
                        "version": version,
                        "data_dir": data_dir,
                        "cache_dir": cache_dir,
                        "config_dir": config_dir,
                        "config_path": config_path,
                    })),
                ))
            }
            ConfigCommand::Show {} => {
                let config = ctx
                    .ensure_config(None)
                    .await
                    .context("failed to load config")?;
                Ok(CommandOutput::ok(
                    serde_json::to_string_pretty(&config).context("failed to serialize config")?,
                    Some(serde_json::to_value(&config).context("failed to serialize config")?),
                ))
            }
            ConfigCommand::Cleanup {
                config,
                cache,
                data,
                all,
                dry_run,
            } => {
                let config_dir = ctx.config_dir();
                let cache_dir = ctx.cache_dir();
                let data_dir = ctx.data_dir();

                let mut cleanup_paths: Vec<PathBuf> = Vec::new();

                if *config || *all {
                    let config_paths: Vec<PathBuf> = get_all_paths_in_dir(config_dir.as_path())?;
                    cleanup_paths.extend(config_paths);
                }

                if *cache || *all {
                    let cache_paths: Vec<PathBuf> = get_all_paths_in_dir(cache_dir.as_path())?;
                    cleanup_paths.extend(cache_paths);
                }

                if *data || *all {
                    let data_paths: Vec<PathBuf> = get_all_paths_in_dir(data_dir.as_path())?;
                    cleanup_paths.extend(data_paths);
                }

                let cleanup_files: Vec<PathBuf> = cleanup_paths
                    .iter()
                    .filter(|p| p.is_file())
                    .cloned()
                    .collect();

                let mut cleanup_dirs: Vec<PathBuf> = cleanup_paths
                    .iter()
                    .filter(|p| p.is_dir())
                    .cloned()
                    .collect();

                // Sort to first delete the deepest directories
                cleanup_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

                // Gather all files as a string for output
                let files_summary = cleanup_paths
                    .iter()
                    .filter(|p| p.is_file())
                    .map(|p| p.display().to_string())
                    .collect::<Vec<String>>()
                    .join("\n");

                if cleanup_files.is_empty() {
                    return Ok(CommandOutput::ok(
                        "No files were found to clean up.",
                        Some(serde_json::json!({
                            "message": "No files were found to clean up.",
                            "success": true,
                        })),
                    ));
                }

                if *dry_run {
                    return Ok(CommandOutput::ok(
                        format!(
                            "Found {} files for cleanup (Dry Run):\n{}\n\n",
                            cleanup_files.len(),
                            files_summary
                        ),
                        Some(serde_json::json!({
                            "message": "Dry run executed successfully. No files were deleted.",
                            "success": true,
                            "file_count": cleanup_files.len(),
                            "files": files_summary
                        })),
                    ));
                }

                warn!(
                    "Found {} files for cleanup. Files to be deleted:\n{}\n\nDo you want to proceed with the deletion? (y/N)",
                    cleanup_files.len(),
                    files_summary
                );

                let mut successful_deletions = 0;
                let mut failed_paths = Vec::new();

                let mut confirmation = String::new();
                io::stdin().read_line(&mut confirmation)?;

                if !confirmation.trim().eq_ignore_ascii_case("y") {
                    return Ok(CommandOutput::ok(
                        format!("Skipped deletion of {} files", cleanup_files.len()),
                        Some(serde_json::json!({
                            "message": "File deletion skipped.",
                            "success": true,
                        })),
                    ));
                }

                for path in &cleanup_files {
                    match std::fs::remove_file(path) {
                        Ok(_) => {
                            info!("Successfully deleted file: {:?}", path.display());
                            successful_deletions += 1
                        }
                        Err(e) => {
                            error!("Failed to delete {} file: {}", path.display(), e);
                            failed_paths.push(path.clone());
                        }
                    }
                }

                for path in &cleanup_dirs {
                    match std::fs::remove_dir_all(path) {
                        Ok(_) => {
                            info!("Successfully deleted dir: {:?}", path.display());
                        }
                        Err(e) => {
                            error!("Failed to delete dir {}: {}", path.display(), e);
                            failed_paths.push(path.clone());
                        }
                    }
                }

                if !failed_paths.is_empty() {
                    return Ok(CommandOutput::error(
                        format!("Failed to delete {} files", failed_paths.len()),
                        Some(serde_json::json!({
                            "message": format!("Partial failure: Deleted {}/{} files.",
                              successful_deletions,
                              cleanup_files.len()),
                            "deleted": successful_deletions,
                            "failed_count": failed_paths.len(),
                            "success": false,
                        })),
                    ));
                }

                return Ok(CommandOutput::ok(
                    format!("Successfully deleted {successful_deletions} files"),
                    Some(serde_json::json!({
                        "message": format!("{successful_deletions} files deleted successfully."),
                        "deleted": successful_deletions,
                    })),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_get_all_paths_in_dir() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let root_path = temp_dir.path();

        let file1_path = root_path.join("file1.txt");
        let subdir_path = root_path.join("subdir");
        let file2_path = subdir_path.join("file2.log");
        let emptydir_path = root_path.join("empty");

        // Create the files and directories
        std::fs::write(&file1_path, "content")
            .expect(&format!("failed to create file {}", file1_path.display()));
        std::fs::create_dir(&subdir_path).expect(&format!(
            "failed to create directory {}",
            subdir_path.display()
        ));
        std::fs::write(&file2_path, "more content")
            .expect(&format!("failed to create file {}", file2_path.display()));
        std::fs::create_dir(&emptydir_path).expect(&format!(
            "failed to create directory {}",
            emptydir_path.display()
        ));

        let mut actual_paths = get_all_paths_in_dir(root_path).expect(&format!(
            "failed to get files from root path {}",
            root_path.display()
        ));

        let mut expected_files = vec![
            file1_path.to_path_buf(),
            subdir_path.to_path_buf(),
            file2_path.to_path_buf(),
            emptydir_path.to_path_buf(),
        ];

        // Sort to ensure the order of results doesn't cause the test to fail
        actual_paths.sort();
        expected_files.sort();

        assert_eq!(
            actual_paths, expected_files,
            "Actual files and expected files do not match."
        );

        // Explicitly check the count for clarity
        assert_eq!(
            actual_paths.len(),
            4,
            "Should have found exactly two files."
        );
    }
}
