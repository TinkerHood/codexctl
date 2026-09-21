use crate::utils::config::Config;
use crate::utils::files::write_bytes_preserve_permissions;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use colored::Colorize as _;
use std::io::ErrorKind;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use tokio::process::Command;

pub async fn execute(
    config: Config,
    profile: String,
    passphrase: Option<String>,
    command: Vec<String>,
    quiet: bool,
) -> Result<ExitStatus> {
    let profile_name = ProfileName::try_from(profile.as_str())
        .with_context(|| format!("Invalid profile name '{profile}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;
    let codex_dir = config.codex_dir();

    if !profile_dir.exists() {
        anyhow::bail!("Profile '{profile}' not found");
    }

    if command.is_empty() {
        anyhow::bail!("No command specified to run");
    }

    let profile_auth = load_profile_auth(&profile_dir, &profile, passphrase.as_ref()).await?;
    let original_auth = apply_profile_auth(codex_dir, &profile_auth).await?;

    // Execute command
    let cmd = &command[0];
    let args = &command[1..];

    if !quiet {
        println!(
            "{} Running with profile {}: {}",
            "▶".cyan(),
            profile.green(),
            command.join(" ").dimmed()
        );
    }

    // Log to history
    let _ = crate::commands::history::log_command(&config, &profile, &command.join(" ")).await;

    let status_result = Command::new(cmd)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit())
        .status()
        .await
        .with_context(|| format!("Failed to execute command: {cmd}"));

    // Always restore original auth after command execution.
    restore_original_auth(codex_dir, original_auth)
        .await
        .context("Could not restore original auth after command execution")?;
    let status = status_result?;

    if !quiet {
        if status.success() {
            println!(
                "\n{} Command completed, restored original auth",
                "✓".green()
            );
        } else {
            println!(
                "\n{} Command exited with code {:?}",
                "!".yellow(),
                status.code()
            );
        }
    }

    Ok(status)
}

async fn load_profile_auth(
    profile_dir: &Path,
    profile_name: &str,
    passphrase: Option<&String>,
) -> Result<Vec<u8>> {
    let profile_auth_path = profile_dir.join("auth.json");
    if !profile_auth_path.exists() {
        anyhow::bail!("Profile '{profile_name}' does not contain auth.json");
    }

    let profile_auth = tokio::fs::read(&profile_auth_path)
        .await
        .with_context(|| format!("Failed to read {}", profile_auth_path.display()))?;

    if crate::utils::crypto::is_encrypted(&profile_auth) {
        return crate::utils::crypto::decrypt(&profile_auth, passphrase)
            .with_context(|| format!("Failed to decrypt profile '{profile_name}'"));
    }

    Ok(profile_auth)
}

async fn apply_profile_auth(codex_dir: &Path, profile_auth: &[u8]) -> Result<Option<Vec<u8>>> {
    tokio::fs::create_dir_all(codex_dir)
        .await
        .with_context(|| format!("Failed to create codex directory: {}", codex_dir.display()))?;

    let auth_path = codex_dir.join("auth.json");
    let original_auth = match tokio::fs::read(&auth_path).await {
        Ok(content) => Some(content),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => {
            return Err(e).with_context(|| {
                format!("Failed to read existing auth file: {}", auth_path.display())
            });
        }
    };

    write_bytes_preserve_permissions(&auth_path, profile_auth)
        .with_context(|| format!("Failed to apply profile auth to {}", auth_path.display()))?;

    Ok(original_auth)
}

async fn restore_original_auth(codex_dir: &Path, original_auth: Option<Vec<u8>>) -> Result<()> {
    let auth_path = codex_dir.join("auth.json");
    match original_auth {
        Some(content) => write_bytes_preserve_permissions(&auth_path, &content)
            .with_context(|| format!("Failed to restore {}", auth_path.display()))?,
        None => match tokio::fs::remove_file(&auth_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to remove {}", auth_path.display()));
            }
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_profile_auth_decrypts_encrypted_profile() {
        let dir = TempDir::new().unwrap();
        let plaintext = br#"{"api_key":"sk-test"}"#.to_vec();
        let encrypted =
            crate::utils::crypto::encrypt(&plaintext, Some(&"secret".to_string())).unwrap();
        tokio::fs::write(dir.path().join("auth.json"), encrypted)
            .await
            .unwrap();

        let auth = load_profile_auth(dir.path(), "encrypted", Some(&"secret".to_string()))
            .await
            .unwrap();
        assert_eq!(auth, plaintext);
    }

    #[tokio::test]
    async fn test_restore_original_auth_removes_temp_auth_when_none() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("auth.json"), b"temp")
            .await
            .unwrap();

        restore_original_auth(dir.path(), None).await.unwrap();
        assert!(!dir.path().join("auth.json").exists());
    }

    #[tokio::test]
    async fn test_apply_profile_auth_preserves_original_auth() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("auth.json"), b"original")
            .await
            .unwrap();

        let original = apply_profile_auth(dir.path(), b"replacement")
            .await
            .unwrap();
        assert_eq!(original, Some(b"original".to_vec()));
        assert_eq!(
            tokio::fs::read(dir.path().join("auth.json")).await.unwrap(),
            b"replacement"
        );
    }
}
