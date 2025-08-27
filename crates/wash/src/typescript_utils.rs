//! Utilities for TypeScript project management and optimization
//!
//! This module provides functionality for optimizing TypeScript builds in development mode,
//! particularly around smart dependency installation based on package file changes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::fs;
use tracing::{debug, trace};

/// Information about package files for a TypeScript project
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageFileInfo {
    /// Path to package.json
    pub package_json: Option<PathBuf>,
    /// Modification time of package.json
    pub package_json_mtime: Option<SystemTime>,
    /// Path to lock file (package-lock.json, yarn.lock, or pnpm-lock.yaml)
    pub lock_file: Option<PathBuf>,
    /// Modification time of lock file
    pub lock_file_mtime: Option<SystemTime>,
    /// Path to node_modules directory
    pub node_modules: Option<PathBuf>,
    /// Whether node_modules directory exists
    pub node_modules_exists: bool,
    /// Detected package manager (npm, yarn, pnpm)
    pub package_manager: PackageManager,
}

/// Supported package managers for TypeScript projects
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PackageManager {
    /// npm (uses package-lock.json)
    Npm,
    /// Yarn (uses yarn.lock)
    Yarn,
    /// pnpm (uses pnpm-lock.yaml)
    Pnpm,
}

impl PackageManager {
    /// Get the lock file name for this package manager
    pub fn lock_file_name(&self) -> &'static str {
        match self {
            PackageManager::Npm => "package-lock.json",
            PackageManager::Yarn => "yarn.lock",
            PackageManager::Pnpm => "pnpm-lock.yaml",
        }
    }

    /// Get the command name for this package manager
    pub fn command_name(&self) -> &'static str {
        match self {
            PackageManager::Npm => "npm",
            PackageManager::Yarn => "yarn",
            PackageManager::Pnpm => "pnpm",
        }
    }
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.command_name())
    }
}

impl PackageFileInfo {
    /// Create a new PackageFileInfo by scanning a TypeScript project directory
    pub async fn scan(project_path: &Path) -> Result<Self> {
        debug!(path = %project_path.display(), "scanning TypeScript project for package files");

        let package_json_path = project_path.join("package.json");
        let (package_json, package_json_mtime) = if package_json_path.exists() {
            let mtime = fs::metadata(&package_json_path)
                .await
                .context("failed to get package.json metadata")?
                .modified()
                .context("failed to get package.json modification time")?;
            (Some(package_json_path), Some(mtime))
        } else {
            (None, None)
        };

        // Detect package manager and get lock file info
        let package_manager = Self::detect_package_manager(project_path).await;
        let lock_file_path = project_path.join(package_manager.lock_file_name());
        let (lock_file, lock_file_mtime) = if lock_file_path.exists() {
            let mtime = fs::metadata(&lock_file_path)
                .await
                .context("failed to get lock file metadata")?
                .modified()
                .context("failed to get lock file modification time")?;
            (Some(lock_file_path), Some(mtime))
        } else {
            (None, None)
        };

        // Check node_modules
        let node_modules_path = project_path.join("node_modules");
        let node_modules_exists = node_modules_path.exists() && node_modules_path.is_dir();
        let node_modules = if node_modules_exists {
            Some(node_modules_path)
        } else {
            None
        };

        let info = Self {
            package_json,
            package_json_mtime,
            lock_file,
            lock_file_mtime,
            node_modules,
            node_modules_exists,
            package_manager,
        };

        trace!(info = ?info, "scanned package file information");
        Ok(info)
    }

    /// Detect which package manager is being used in the project
    async fn detect_package_manager(project_path: &Path) -> PackageManager {
        // Check for lock files in order of preference
        if project_path.join("pnpm-lock.yaml").exists() {
            PackageManager::Pnpm
        } else if project_path.join("yarn.lock").exists() {
            PackageManager::Yarn
        } else {
            // Default to npm (package-lock.json may or may not exist yet)
            PackageManager::Npm
        }
    }

    /// Check if dependencies have changed since the last check
    ///
    /// Returns true if:
    /// - Package.json has been modified since last_check
    /// - Lock file has been modified since last_check  
    /// - node_modules doesn't exist (needs fresh install)
    /// - This is the first check (last_check is None)
    pub fn have_dependencies_changed(&self, last_check: Option<&PackageFileInfo>) -> bool {
        let Some(last_check) = last_check else {
            debug!("no previous package check found, considering dependencies changed");
            return true;
        };

        // If node_modules doesn't exist, we definitely need to install
        if !self.node_modules_exists {
            debug!("node_modules directory does not exist, considering dependencies changed");
            return true;
        }

        // Check if package.json has changed
        if self.package_json_mtime != last_check.package_json_mtime {
            debug!(
                current = ?self.package_json_mtime,
                previous = ?last_check.package_json_mtime,
                "package.json modification time changed"
            );
            return true;
        }

        // Check if lock file has changed
        if self.lock_file_mtime != last_check.lock_file_mtime {
            debug!(
                current = ?self.lock_file_mtime,
                previous = ?last_check.lock_file_mtime,
                "lock file modification time changed"
            );
            return true;
        }

        debug!("no package file changes detected, dependencies considered unchanged");
        false
    }

    /// Get the recommended install command arguments for this package manager
    pub fn get_install_args(&self) -> Vec<String> {
        match self.package_manager {
            PackageManager::Npm => vec!["install".to_string()],
            PackageManager::Yarn => vec!["install".to_string()],
            PackageManager::Pnpm => vec!["install".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[tokio::test]
    async fn test_package_manager_detection() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Test npm detection (default)
        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert_eq!(info.package_manager, PackageManager::Npm);

        // Test yarn detection
        fs::write(project_path.join("yarn.lock"), "# Yarn lock file")
            .await
            .unwrap();
        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert_eq!(info.package_manager, PackageManager::Yarn);

        // Test pnpm detection (takes precedence)
        fs::write(project_path.join("pnpm-lock.yaml"), "# pnpm lock file")
            .await
            .unwrap();
        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert_eq!(info.package_manager, PackageManager::Pnpm);
    }

    #[tokio::test]
    async fn test_dependency_change_detection() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Create initial package.json
        fs::write(project_path.join("package.json"), r#"{"name": "test"}"#)
            .await
            .unwrap();
        fs::create_dir_all(project_path.join("node_modules"))
            .await
            .unwrap();

        let initial_info = PackageFileInfo::scan(project_path).await.unwrap();

        // Should detect change when no previous info
        assert!(initial_info.have_dependencies_changed(None));

        // Should not detect change when files are same
        let same_info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(!same_info.have_dependencies_changed(Some(&initial_info)));

        // Should detect change when node_modules is deleted
        fs::remove_dir_all(project_path.join("node_modules"))
            .await
            .unwrap();
        let no_modules_info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(no_modules_info.have_dependencies_changed(Some(&initial_info)));

        // Recreate node_modules and add package-lock.json
        fs::create_dir_all(project_path.join("node_modules"))
            .await
            .unwrap();
        fs::write(
            project_path.join("package-lock.json"),
            r#"{"lockfileVersion": 2}"#,
        )
        .await
        .unwrap();

        let with_lock_info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(with_lock_info.have_dependencies_changed(Some(&initial_info)));
    }

    #[tokio::test]
    async fn test_package_file_scanning() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Test empty directory
        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(info.package_json.is_none());
        assert!(info.lock_file.is_none());
        assert!(!info.node_modules_exists);

        // Add package.json
        fs::write(project_path.join("package.json"), r#"{"name": "test"}"#)
            .await
            .unwrap();
        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(info.package_json.is_some());
        assert!(info.package_json_mtime.is_some());

        // Add lock file and node_modules
        fs::write(project_path.join("package-lock.json"), "{}")
            .await
            .unwrap();
        fs::create_dir_all(project_path.join("node_modules"))
            .await
            .unwrap();

        let info = PackageFileInfo::scan(project_path).await.unwrap();
        assert!(info.package_json.is_some());
        assert!(info.lock_file.is_some());
        assert!(info.node_modules_exists);
        assert_eq!(info.package_manager, PackageManager::Npm);
    }

    #[test]
    fn test_package_manager_properties() {
        assert_eq!(PackageManager::Npm.lock_file_name(), "package-lock.json");
        assert_eq!(PackageManager::Yarn.lock_file_name(), "yarn.lock");
        assert_eq!(PackageManager::Pnpm.lock_file_name(), "pnpm-lock.yaml");

        assert_eq!(PackageManager::Npm.command_name(), "npm");
        assert_eq!(PackageManager::Yarn.command_name(), "yarn");
        assert_eq!(PackageManager::Pnpm.command_name(), "pnpm");
    }
}
