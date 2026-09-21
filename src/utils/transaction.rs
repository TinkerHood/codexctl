use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use tempfile::TempDir;

/// Prepare a profile beside its destination before replacing an existing profile.
/// Ordinary commit errors restore the original; this is not a crash-safe transaction.
pub struct ProfileTransaction {
    target: PathBuf,
    workspace: TempDir,
}

impl ProfileTransaction {
    pub fn new(target: &Path) -> Result<Self> {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let workspace = tempfile::Builder::new()
            .prefix(".codexctl_profile_")
            .tempdir_in(parent)?;
        std::fs::create_dir(workspace.path().join("staged"))?;
        Ok(Self {
            target: target.to_path_buf(),
            workspace,
        })
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.workspace.path().join("staged")
    }

    /// Install a fully prepared profile, restoring the original on rename failure.
    pub fn commit(self) -> Result<()> {
        let original = self.workspace.path().join("original");
        let had_original = match std::fs::symlink_metadata(&self.target) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).context("Failed to inspect existing profile"),
        };
        if had_original {
            std::fs::rename(&self.target, &original)
                .context("Failed to preserve existing profile")?;
        }
        if let Err(error) = std::fs::rename(self.staging_dir(), &self.target) {
            if had_original && let Err(restore_error) = std::fs::rename(&original, &self.target) {
                // Keep the only surviving copy when recovery itself fails.
                let recovery = self.workspace.keep().join("original");
                anyhow::bail!(
                    "Failed to install profile: {error}; failed to restore: {restore_error}. Original retained at {}",
                    recovery.display()
                );
            }
            return Err(error).context("Failed to install profile; original preserved");
        }
        // Installation is complete. Cleanup is best-effort: returning Err here
        // would falsely tell callers that the original profile was preserved.
        let workspace = self.workspace.keep();
        if let Err(error) = std::fs::remove_dir_all(&workspace) {
            eprintln!(
                "Warning: profile installed, but previous profile cleanup failed at {}: {error}",
                workspace.display()
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_profile_without_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("stale"), "old").unwrap();
        let transaction = ProfileTransaction::new(&target).unwrap();
        std::fs::write(transaction.staging_dir().join("auth.json"), "new").unwrap();
        transaction.commit().unwrap();
        assert_eq!(std::fs::read(target.join("auth.json")).unwrap(), b"new");
        assert!(!target.join("stale").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_preparation_preserves_original_and_cleans_staging() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("auth.json"), "original").unwrap();
        {
            let transaction = ProfileTransaction::new(&target).unwrap();
            std::fs::write(transaction.staging_dir().join("auth.json"), "partial").unwrap();
        }
        assert_eq!(
            std::fs::read(target.join("auth.json")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_commit_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("auth.json"), "original").unwrap();
        let transaction = ProfileTransaction::new(&target).unwrap();
        std::fs::remove_dir(transaction.staging_dir()).unwrap();
        assert!(transaction.commit().is_err());
        assert_eq!(
            std::fs::read(target.join("auth.json")).unwrap(),
            b"original"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_failure_keeps_successful_install_and_reports_retained_workspace() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        let locked = target.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("old"), "old").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let transaction = ProfileTransaction::new(&target).unwrap();
        let workspace = transaction.workspace.path().to_path_buf();
        std::fs::write(transaction.staging_dir().join("auth.json"), "new").unwrap();
        let result = transaction.commit();
        // Restore fixture permissions before any assertion, including on root
        // runners where removing a non-writable directory can still succeed.
        let retained = workspace.join("original/locked");
        if retained.exists() {
            std::fs::set_permissions(retained, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(result.is_ok());
        assert_eq!(std::fs::read(target.join("auth.json")).unwrap(), b"new");
    }
}
