//! Functions for creating new projects from git repositories

use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::process::Command;
use tracing::{debug, error, info, instrument};

use crate::cli::CliContext;

/// Extract a specific subfolder from the cloned template
#[instrument(level = "debug", skip_all)]
pub(crate) async fn extract_subfolder(
    ctx: &CliContext,
    output_dir: &Path,
    subfolder: &str,
) -> anyhow::Result<()> {
    let subfolder_path = output_dir.join(subfolder);

    if tokio::fs::metadata(&subfolder_path).await.is_err() {
        bail!("Subfolder '{subfolder}' does not exist in cloned repository");
    }

    let metadata = tokio::fs::metadata(&subfolder_path)
        .await
        .context("failed to read subfolder metadata")?;

    if !metadata.is_dir() {
        bail!("subfolder '{subfolder}' is not a directory");
    }

    info!(subfolder = %subfolder, "extracting subfolder");

    // Create temporary directory for extraction
    let temp_dir = ctx.cache_dir().join("wash_new_temp_dir");

    // Move subfolder contents to temp directory
    copy_dir_recursive(&subfolder_path, &temp_dir).await?;

    // Remove original directory
    tokio::fs::remove_dir_all(output_dir)
        .await
        .context("failed to remove original directory")?;

    // Move temp directory to final location
    tokio::fs::rename(&temp_dir, output_dir)
        .await
        .context("failed to move extracted subfolder")?;

    info!(subfolder, "successfully extracted subfolder",);
    Ok(())
}

/// Clone a repository from a git URL
///
/// NOTE: This requires the `git` command to be available in the system PATH. This should
/// already be checked by the doctor command.
#[instrument(level = "debug", skip_all)]
pub(crate) async fn clone_template(
    url: &str,
    output_dir: &Path,
    git_ref: Option<&str>,
) -> anyhow::Result<()> {
    debug!(
        url,
        output_dir = %output_dir.display(),
        "cloning repository",
    );

    info!(url, "cloning git repository");

    let mut cmd = Command::new("git");
    let mut args = vec!["clone".to_string(), url.to_string()];

    // Add branch/tag reference if specified
    if let Some(git_ref) = git_ref {
        args.insert(1, "--branch".to_string());
        args.insert(2, git_ref.to_string());
        info!("Using git reference: {}", git_ref);
    }

    args.push(output_dir.to_string_lossy().to_string());
    cmd.args(&args);

    let output = cmd
        .output()
        .await
        .context("failed to execute git clone command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!(stderr = %stderr, "Git clone failed");
        bail!("Git clone failed: {stderr}");
    }

    info!(output_dir = %output_dir.display(), "Successfully cloned repository");

    // Remove .git directory to avoid confusion
    let git_dir = output_dir.join(".git");
    if git_dir.exists() {
        debug!("Removing .git directory from cloned repository");
        tokio::fs::remove_dir_all(&git_dir)
            .await
            .context("Failed to remove .git directory")?;
    }

    Ok(())
}

/// Recursively copy a directory using tokio::fs. Note that the boxing is necessary to allow for async recursion.
pub(crate) fn copy_dir_recursive<'a>(
    src: impl AsRef<std::path::Path> + Send + 'a,
    dst: impl AsRef<std::path::Path> + Send + 'a,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let src = src.as_ref();
        let dst = dst.as_ref();

        let src_metadata = tokio::fs::metadata(src)
            .await
            .with_context(|| format!("Failed to read source path: {src}", src = src.display()))?;

        if !src_metadata.is_dir() {
            bail!("Source is not a directory: {src}", src = src.display());
        }

        tokio::fs::create_dir_all(dst)
            .await
            .with_context(|| format!("Failed to create directory: {dst}", dst = dst.display()))?;

        let mut entries = tokio::fs::read_dir(src)
            .await
            .with_context(|| format!("Failed to read directory: {src}", src = src.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .context("Failed to read directory entry")?
        {
            let path = entry.path();
            let name = entry.file_name();
            let dst_path = dst.join(&name);

            // Skip .git directories
            if name == ".git" {
                debug!("Skipping .git directory");
                continue;
            }

            let metadata = entry
                .metadata()
                .await
                .context("Failed to read entry metadata")?;

            if metadata.is_dir() {
                copy_dir_recursive(&path, &dst_path).await?;
            } else {
                tokio::fs::copy(&path, &dst_path).await.with_context(|| {
                    format!(
                        "Failed to copy file {} to {}",
                        path.display(),
                        dst_path.display()
                    )
                })?;
            }
        }

        Ok(())
    })
}
