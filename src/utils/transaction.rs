use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use tempfile::TempDir;

const ORIGINAL_PREFIX: &str = ".codexctl_original_";

/// Advisory process lock. The lock file remains, but the lock is released on exit.
pub struct DirectoryLock(File);

impl DirectoryLock {
    pub fn try_acquire(dir: &Path, name: &str) -> Result<Option<Self>> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join(name))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self(file))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => {
                Err(error).context("Failed to lock directory")
            }
        }
    }

    pub fn acquire(dir: &Path, name: &str) -> Result<Self> {
        Self::try_acquire(dir, name)?
            .ok_or_else(|| anyhow::anyhow!("Another codexctl operation is using {}", dir.display()))
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn recovery_path(target: &Path) -> Result<PathBuf> {
    let parent = target.parent().context("Profile has no parent directory")?;
    let name = target.file_name().context("Profile has no name")?;
    Ok(parent.join(format!("{ORIGINAL_PREFIX}{}", name.to_string_lossy())))
}

fn path_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("Failed to inspect {}", path.display())),
    }
}

/// Caller must hold the profiles directory lock.
fn recover_target(target: &Path) -> Result<()> {
    let original = recovery_path(target)?;
    if !path_exists(&original)? {
        return Ok(());
    }
    if path_exists(target)? {
        // The second rename completed. Cleanup failure must not undo the new profile.
        if let Err(error) = remove_path(&original) {
            eprintln!(
                "Warning: could not remove previous profile at {}: {error}",
                original.display()
            );
        }
    } else {
        std::fs::rename(&original, target).with_context(|| {
            format!(
                "Failed to recover interrupted profile at {}",
                target.display()
            )
        })?;
        sync_dir(target.parent().unwrap())?;
    }
    Ok(())
}

/// Caller holds the profiles lock. Do not delete while an old copy could
/// later be mistaken for an interrupted replacement and resurrected.
pub(crate) fn prepare_profile_delete(target: &Path) -> Result<()> {
    recover_target(target)?;
    let original = recovery_path(target)?;
    if path_exists(&original)? {
        anyhow::bail!(
            "Cannot delete profile while previous copy remains at {}",
            original.display()
        );
    }
    Ok(())
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Repair any interrupted replacement before commands inspect profile paths.
pub fn recover_profiles(parent: &Path) -> Result<()> {
    let Some(_lock) = DirectoryLock::try_acquire(parent, ".codexctl_profiles.lock")? else {
        return Ok(());
    };
    for entry in std::fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(profile) = name.strip_prefix(ORIGINAL_PREFIX) else {
            if name.starts_with(".codexctl_profile_") && entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path()).with_context(|| {
                    format!(
                        "Failed to remove interrupted staging at {}",
                        entry.path().display()
                    )
                })?;
            }
            continue;
        };
        if ProfileName::try_from(profile).is_ok() {
            recover_target(&parent.join(profile))?;
        }
    }
    Ok(())
}

/// Prepare a profile beside its destination before replacing an existing profile.
pub struct ProfileTransaction {
    target: PathBuf,
    workspace: TempDir,
    _lock: DirectoryLock,
}

impl ProfileTransaction {
    pub fn new(target: &Path) -> Result<Self> {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let lock = DirectoryLock::acquire(parent, ".codexctl_profiles.lock")?;
        recover_target(target)?;
        let workspace = tempfile::Builder::new()
            .prefix(".codexctl_profile_")
            .tempdir_in(parent)?;
        std::fs::create_dir(workspace.path().join("staged"))?;
        Ok(Self {
            target: target.to_path_buf(),
            workspace,
            _lock: lock,
        })
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.workspace.path().join("staged")
    }

    /// Install a fully prepared profile, restoring the original on rename failure.
    /// A later invocation also restores it if the process stops between renames.
    pub fn commit(self) -> Result<()> {
        let original = recovery_path(&self.target)?;
        let parent = self
            .target
            .parent()
            .context("Profile has no parent directory")?;
        let had_original = match std::fs::symlink_metadata(&self.target) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error).context("Failed to inspect existing profile"),
        };
        if had_original {
            std::fs::rename(&self.target, &original)
                .context("Failed to preserve existing profile")?;
            sync_dir(parent)?;
            #[cfg(test)]
            if std::env::var_os("CODEXCTL_TEST_ABORT_AFTER_PROFILE_BACKUP").is_some() {
                std::process::abort();
            }
            #[cfg(test)]
            if let Some(ready) = std::env::var_os("CODEXCTL_TEST_PROFILE_READY") {
                std::fs::write(ready, b"ready")?;
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
        }
        if let Err(error) = std::fs::rename(self.staging_dir(), &self.target) {
            if had_original && let Err(restore_error) = std::fs::rename(&original, &self.target) {
                anyhow::bail!(
                    "Failed to install profile: {error}; failed to restore: {restore_error}. Original retained at {}",
                    original.display()
                );
            }
            return Err(error).context("Failed to install profile; original preserved");
        }
        if let Err(error) = sync_dir(parent) {
            eprintln!(
                "Warning: profile installed, but directory sync failed at {}: {error}",
                parent.display()
            );
        }
        // Installation is complete. Cleanup is best-effort: returning Err here
        // would falsely tell callers that the original profile was preserved.
        if had_original && let Err(error) = remove_path(&original) {
            eprintln!(
                "Warning: profile installed, but previous profile cleanup failed at {}: {error}",
                original.display()
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
        assert!(!recovery_path(&target).unwrap().exists());
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
        assert!(!recovery_path(&target).unwrap().exists());
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
        assert!(!recovery_path(&target).unwrap().exists());
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
        std::fs::write(transaction.staging_dir().join("auth.json"), "new").unwrap();
        let result = transaction.commit();
        // Restore fixture permissions before any assertion, including on root
        // runners where removing a non-writable directory can still succeed.
        let retained = recovery_path(&target).unwrap().join("locked");
        if retained.exists() {
            std::fs::set_permissions(retained, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(result.is_ok());
        assert_eq!(std::fs::read(target.join("auth.json")).unwrap(), b"new");
    }

    #[test]
    fn crash_helper() {
        let Some(target) = std::env::var_os("CODEXCTL_TEST_PROFILE_TARGET") else {
            return;
        };
        let target = PathBuf::from(target);
        let transaction = ProfileTransaction::new(&target).unwrap();
        std::fs::write(transaction.staging_dir().join("auth.json"), "replacement").unwrap();
        transaction.commit().unwrap();
    }

    #[test]
    fn aborted_writer_recovers_original_on_next_invocation() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("auth.json"), "original").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "utils::transaction::tests::crash_helper"])
            .env("CODEXCTL_TEST_PROFILE_TARGET", &target)
            .env("CODEXCTL_TEST_ABORT_AFTER_PROFILE_BACKUP", "1")
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(!target.exists());
        recover_profiles(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(target.join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn killed_writer_releases_lock_and_recovers_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("profile");
        let ready = dir.path().join("ready");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("auth.json"), "original").unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "utils::transaction::tests::crash_helper"])
            .env("CODEXCTL_TEST_PROFILE_TARGET", &target)
            .env("CODEXCTL_TEST_PROFILE_READY", &ready)
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready.exists(), "child did not move the original profile");
        assert!(DirectoryLock::acquire(dir.path(), ".codexctl_profiles.lock").is_err());
        recover_profiles(dir.path()).unwrap();
        assert!(!target.exists());
        child.kill().unwrap();
        child.wait().unwrap();
        recover_profiles(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(target.join("auth.json")).unwrap(),
            b"original"
        );
    }
}
