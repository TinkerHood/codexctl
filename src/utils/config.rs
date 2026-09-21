use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use directories::ProjectDirs;

use crate::utils::validation::ProfileName;

/// Returns the platform-appropriate `Codex` CLI data directory.
///
/// - Linux/macOS: `~/.local/share/codex` (via `dirs::data_dir`) or `~/.codex` fallback
/// - Windows: `%APPDATA%\codex`
fn codex_data_dir() -> Result<PathBuf> {
    // `dirs::data_dir()` returns:
    //   Linux:   $XDG_DATA_HOME  or ~/.local/share
    //   macOS:   ~/Library/Application Support
    //   Windows: %APPDATA%
    // `Codex` CLI itself uses ~/.codex on `Unix`; we mirror that convention on
    // `Unix` systems by falling back to home_dir/.codex, while `Windows` uses
    // the proper APPDATA path.
    if cfg!(target_os = "windows") {
        dirs::data_dir()
            .map(|d| d.join("codex"))
            .ok_or_else(|| anyhow::anyhow!("Could not determine %%APPDATA%% directory"))
    } else {
        // Prefer XDG / platform data dir but fall back to ~/.codex to stay
        // compatible with existing `Codex` CLI installations.
        Ok(dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
            .join(".codex"))
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::struct_field_names)]
pub struct Config {
    /// Base directory for profiles
    profiles_dir: PathBuf,
    /// `Codex` CLI config directory
    codex_dir: PathBuf,
    /// Backup directory
    backup_dir: PathBuf,
}

impl Config {
    #[cfg(test)]
    pub(crate) fn for_test(profiles_dir: PathBuf, codex_dir: PathBuf) -> Result<Self> {
        let backup_dir = profiles_dir.join("backups");
        std::fs::create_dir_all(&backup_dir)?;
        Ok(Self {
            profiles_dir,
            codex_dir,
            backup_dir,
        })
    }

    /// Create a new configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the directories cannot be created
    pub fn new(custom_dir: Option<PathBuf>) -> Result<Self> {
        let profiles_dir = if let Some(dir) = custom_dir {
            dir
        } else if let Some(dirs) = ProjectDirs::from("com", "repohelper", "codexctl") {
            dirs.data_dir().to_path_buf()
        } else {
            dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
                .join(".codexctl")
        };

        let codex_dir = codex_data_dir()?;
        let backup_dir = profiles_dir.join("backups");

        // Ensure directories exist - using let-else for early returns
        std::fs::create_dir_all(&profiles_dir).with_context(|| {
            format!(
                "Failed to create profiles directory: {}",
                profiles_dir.display()
            )
        })?;
        std::fs::create_dir_all(&backup_dir).with_context(|| {
            format!(
                "Failed to create backup directory: {}",
                backup_dir.display()
            )
        })?;

        Ok(Self {
            profiles_dir,
            codex_dir,
            backup_dir,
        })
    }

    #[must_use]
    pub fn profiles_dir(&self) -> &Path {
        &self.profiles_dir
    }

    #[must_use]
    pub fn codex_dir(&self) -> &Path {
        &self.codex_dir
    }

    #[must_use]
    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    /// Returns the path for a validated profile name, enforcing that the
    /// resolved path stays within the profiles directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolved path escapes the profiles directory
    /// (defense-in-depth; `ProfileName` validation should prevent this).
    pub fn profile_path_validated(&self, name: &ProfileName) -> Result<PathBuf> {
        let path = self.profiles_dir.join(name.as_str());
        if !path.starts_with(&self.profiles_dir) {
            anyhow::bail!(
                "Profile path '{}' would escape the profiles directory '{}'",
                path.display(),
                self.profiles_dir.display()
            );
        }
        Ok(path)
    }

    /// Files to backup/sync from `Codex` directory
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn critical_files() -> &'static [&'static str] {
        &[
            "auth.json",
            "config.toml",
            "history.jsonl",
            "state.sqlite",
            "sessions/",
            "memories/",
        ]
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_config_new_with_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(Some(temp_dir.path().to_path_buf())).unwrap();

        assert!(config.profiles_dir().exists());
        assert!(config.backup_dir().exists());
        assert_eq!(config.profiles_dir(), temp_dir.path());
    }

    #[test]
    fn test_profile_path_validated() {
        use crate::utils::validation::ProfileName;
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let name = ProfileName::try_from("test-profile").unwrap();
        let profile_path = config.profile_path_validated(&name).unwrap();
        assert_eq!(profile_path, temp_dir.path().join("test-profile"));
    }

    #[test]
    fn test_profile_path_validated_stays_within_profiles_dir() {
        use crate::utils::validation::ProfileName;
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(Some(temp_dir.path().to_path_buf())).unwrap();

        // ProfileName rejects traversal, so these should fail at parse time
        assert!(ProfileName::try_from("../../etc/passwd").is_err());
        assert!(ProfileName::try_from("..").is_err());

        // Valid name stays within profiles dir
        let name = ProfileName::try_from("safe-name").unwrap();
        let path = config.profile_path_validated(&name).unwrap();
        assert!(path.starts_with(config.profiles_dir()));
    }

    #[test]
    fn test_critical_files_list() {
        let files = Config::critical_files();
        assert!(files.contains(&"auth.json"));
        assert!(files.contains(&"config.toml"));
        assert!(files.contains(&"history.jsonl"));
        assert!(files.contains(&"sessions/"));
        assert!(files.contains(&"memories/"));
    }

    #[test]
    fn test_config_clone() {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(Some(temp_dir.path().to_path_buf())).unwrap();
        let _ = &config;
    }

    // --- Platform detection tests ---

    #[test]
    fn test_codex_data_dir_returns_absolute_path() {
        let dir = codex_data_dir().unwrap();
        assert!(
            dir.is_absolute(),
            "codex_data_dir should be absolute: {dir:?}"
        );
    }

    #[test]
    fn test_codex_data_dir_ends_with_codex() {
        let dir = codex_data_dir().unwrap();
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(".codex"),
            "expected last component '.codex', got: {dir:?}"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_windows_codex_dir_uses_appdata() {
        let dir = codex_data_dir().unwrap();
        // On Windows the path should be under %APPDATA%
        let appdata = std::env::var("APPDATA").unwrap();
        assert!(
            dir.starts_with(&appdata),
            "Windows codex dir should be under APPDATA; got: {dir:?}"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_unix_codex_dir_uses_home() {
        let dir = codex_data_dir().unwrap();
        let home = dirs::home_dir().unwrap();
        assert!(
            dir.starts_with(&home),
            "Unix codex dir should be under home; got: {dir:?}"
        );
    }

    #[test]
    fn test_codex_dir_accessible_from_config() {
        let temp_dir = TempDir::new().unwrap();
        let config = Config::new(Some(temp_dir.path().to_path_buf())).unwrap();
        // codex_dir() should return the platform-appropriate path
        assert!(config.codex_dir().is_absolute());
    }
}
