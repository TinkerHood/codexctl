use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use walkdir::WalkDir;

use crate::utils::config::Config;

/// Copy files from source to destination, preserving directory structure
///
/// # Errors
///
/// Returns an error if any file operation fails
pub fn copy_profile_files(src: &Path, dst: &Path, files: &[&str]) -> Result<Vec<String>> {
    let mut copied = Vec::new();

    for pattern in files {
        let src_path = src.join(pattern);

        if src_path.is_file() {
            let dst_path = dst.join(pattern);
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    src_path.display(),
                    dst_path.display()
                )
            })?;
            if let Ok(metadata) = std::fs::metadata(&src_path) {
                let _ = std::fs::set_permissions(&dst_path, metadata.permissions());
            }
            copied.push((*pattern).to_string());
        } else if src_path.is_dir() {
            // Copy entire directory
            copy_dir_recursive(&src_path, &dst.join(pattern))?;
            copied.push(format!("{pattern}/ (directory)"));
        }
    }

    Ok(copied)
}

/// Recursively copy directory
///
/// # Errors
///
/// Returns an error if any directory creation or file copy fails
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create directory: {}", dst.display()))?;
    if let Ok(metadata) = std::fs::metadata(src) {
        let _ = std::fs::set_permissions(dst, metadata.permissions());
    }

    for entry in WalkDir::new(src) {
        let entry = entry.with_context(|| format!("Failed to read {}", src.display()))?;
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(src) else {
            continue;
        };
        let destination = dst.join(relative);

        if path.is_file() {
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(path, &destination)?;
            if let Ok(metadata) = std::fs::metadata(path) {
                let _ = std::fs::set_permissions(&destination, metadata.permissions());
            }
        } else if path.is_dir() && path != src {
            std::fs::create_dir_all(&destination)?;
            if let Ok(metadata) = std::fs::metadata(path) {
                let _ = std::fs::set_permissions(&destination, metadata.permissions());
            }
        }
    }

    Ok(())
}

/// Create a backup containing only the live `auth.json`.
///
/// Returns `Ok(None)` when there is no live auth file to back up.
///
/// # Errors
///
/// Returns an error if the auth file exists but cannot be copied.
pub fn create_auth_backup(codex_dir: &Path, backup_dir: &Path) -> Result<Option<PathBuf>> {
    use chrono::Local;

    let auth_path = codex_dir.join("auth.json");
    if !auth_path.exists() {
        return Ok(None);
    }

    std::fs::create_dir_all(backup_dir).with_context(|| {
        format!(
            "Failed to create backup directory: {}",
            backup_dir.display()
        )
    })?;

    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let backup = tempfile::Builder::new()
        .prefix(&format!("auth_backup_{timestamp}_"))
        .tempdir_in(backup_dir)
        .context("Failed to create auth backup directory")?;
    let backup_path = backup.path();

    let backup_auth_path = backup_path.join("auth.json");
    std::fs::copy(&auth_path, &backup_auth_path).with_context(|| {
        format!(
            "Failed to copy {} to {}",
            auth_path.display(),
            backup_auth_path.display()
        )
    })?;
    if let Ok(metadata) = std::fs::metadata(&auth_path) {
        let _ = std::fs::set_permissions(&backup_auth_path, metadata.permissions());
    }

    Ok(Some(backup.keep()))
}

/// Write bytes to an existing path while preserving existing filesystem permissions.
///
/// Uses a temp-file + rename strategy to avoid partial writes.
///
/// # Errors
///
/// Returns an error if write, rename, or permission operations fail.
pub fn write_bytes_preserve_permissions(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;

    let existing_permissions = match std::fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to inspect {}", path.display()));
        }
    };
    // NamedTempFile creates private files and removes them on every error path.
    let mut file = tempfile::Builder::new()
        .prefix(".codexctl_write_")
        .tempfile_in(parent)
        .with_context(|| format!("Failed to create temp file in {}", parent.display()))?;
    file.write_all(data).context("Failed to write temp file")?;

    if let Some(perms) = existing_permissions {
        file.as_file()
            .set_permissions(perms)
            .context("Failed to preserve file permissions")?;
    }
    file.as_file()
        .sync_all()
        .context("Failed to sync temp file")?;
    file.persist(path)
        .with_context(|| format!("Failed to atomically replace {}", path.display()))?;

    Ok(())
}

/// Get critical files to sync.
///
/// Delegates to [`Config::critical_files`] to keep the list in one place.
#[must_use]
pub fn get_critical_files() -> &'static [&'static str] {
    Config::critical_files()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_copy_dir_recursive() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();

        // Create source structure
        let sub_dir = src_dir.path().join("subdir");
        std::fs::create_dir(&sub_dir).unwrap();
        std::fs::write(src_dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(sub_dir.join("file2.txt"), "content2").unwrap();

        // Copy
        copy_dir_recursive(src_dir.path(), dst_dir.path()).unwrap();

        // Verify
        assert!(dst_dir.path().join("file1.txt").exists());
        assert!(dst_dir.path().join("subdir/file2.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dst_dir.path().join("file1.txt")).unwrap(),
            "content1"
        );
    }

    #[test]
    fn test_copy_profile_files() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();

        // Create source files
        std::fs::write(src_dir.path().join("auth.json"), "{\"token\": \"test\"}").unwrap();
        std::fs::write(src_dir.path().join("config.toml"), "model = \"gpt-4\"").unwrap();

        // Create sessions directory
        let sessions_dir = src_dir.path().join("sessions");
        std::fs::create_dir(&sessions_dir).unwrap();
        std::fs::write(sessions_dir.join("session1.json"), "{}").unwrap();

        // Copy
        let files = vec!["auth.json", "config.toml", "sessions/"];
        let copied = copy_profile_files(src_dir.path(), dst_dir.path(), &files).unwrap();

        // Verify
        assert!(copied.contains(&"auth.json".to_string()));
        assert!(copied.contains(&"config.toml".to_string()));
        // Check for directory (with trailing slash stripped)
        assert!(copied.iter().any(|c| c.starts_with("sessions")));
        assert!(dst_dir.path().join("auth.json").exists());
        assert!(dst_dir.path().join("sessions/session1.json").exists());
    }

    #[test]
    fn test_create_auth_backup() {
        let codex_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();
        std::fs::write(codex_dir.path().join("auth.json"), "{\"token\":\"secret\"}").unwrap();
        std::fs::write(codex_dir.path().join("sessions.json"), "{}").unwrap();

        let backup_path = create_auth_backup(codex_dir.path(), backup_dir.path())
            .unwrap()
            .expect("backup should exist");

        assert!(backup_path.join("auth.json").exists());
        assert!(!backup_path.join("sessions.json").exists());
    }

    #[test]
    fn test_create_auth_backup_without_auth_file() {
        let codex_dir = TempDir::new().unwrap();
        let backup_dir = TempDir::new().unwrap();

        let backup = create_auth_backup(codex_dir.path(), backup_dir.path()).unwrap();
        assert!(backup.is_none());
    }

    #[test]
    fn test_get_critical_files() {
        let files = get_critical_files();
        assert!(!files.is_empty());
        assert!(files.contains(&"auth.json"));
        assert!(files.contains(&"config.toml"));
    }

    #[test]
    fn failed_write_cleans_up_temporary_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("existing-directory");
        std::fs::create_dir(&target).unwrap();
        assert!(write_bytes_preserve_permissions(&target, b"secret").is_err());
        assert!(target.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn successive_backups_preserve_both_versions() {
        let source = TempDir::new().unwrap();
        let backups = TempDir::new().unwrap();
        std::fs::write(source.path().join("auth.json"), "first").unwrap();
        let first = create_auth_backup(source.path(), backups.path())
            .unwrap()
            .unwrap();
        std::fs::write(source.path().join("auth.json"), "second").unwrap();
        let second = create_auth_backup(source.path(), backups.path())
            .unwrap()
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first.join("auth.json")).unwrap(), b"first");
        assert_eq!(std::fs::read(second.join("auth.json")).unwrap(), b"second");
    }

    #[cfg(unix)]
    #[test]
    fn new_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        write_bytes_preserve_permissions(&path, b"secret").unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn recursive_copy_reports_missing_source() {
        let dir = TempDir::new().unwrap();
        assert!(copy_dir_recursive(&dir.path().join("missing"), &dir.path().join("copy")).is_err());
    }

    #[test]
    fn test_write_bytes_preserve_permissions_preserves_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let original = b"line1\r\nline2\n\xff\xfebinary";
        std::fs::write(&path, original).unwrap();

        let updated = b"new\r\ncontent\n\x00\xff";
        write_bytes_preserve_permissions(&path, updated).unwrap();

        let actual = std::fs::read(&path).unwrap();
        assert_eq!(actual, updated);
    }

    #[cfg(unix)]
    #[test]
    fn test_write_bytes_preserve_permissions_preserves_mode_unix() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, b"{\"token\":\"x\"}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        write_bytes_preserve_permissions(&path, b"{\"token\":\"y\"}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
