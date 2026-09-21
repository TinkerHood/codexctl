use crate::utils::auth::read_email_from_codex_dir;
use crate::utils::config::Config;
use crate::utils::files::{
    copy_profile_files, get_critical_files, write_bytes_preserve_permissions,
};
use crate::utils::profile::ProfileMeta;
use crate::utils::transaction::ProfileTransaction;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use colored::Colorize as _;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub async fn execute(
    config: Config,
    name: String,
    description: Option<String>,
    force: bool,
    quiet: bool,
    passphrase: Option<String>,
) -> Result<()> {
    let profile_name = ProfileName::try_from(name.as_str())
        .with_context(|| format!("Invalid profile name '{name}'"))?;
    let codex_dir = config.codex_dir();
    let profile_dir = config.profile_path_validated(&profile_name)?;

    if !codex_dir.exists() {
        anyhow::bail!(
            "Codex directory not found at {}. Is Codex CLI installed?",
            codex_dir.display()
        );
    }

    if !codex_dir.join("auth.json").is_file() {
        anyhow::bail!("Codex directory does not contain auth.json; existing profile preserved");
    }

    if profile_dir.exists() && !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Profile '{}' already exists. Overwrite?",
                name.yellow()
            ))
            .default(false)
            .interact()?;

        if !confirm {
            if !quiet {
                println!("Cancelled");
            }
            return Ok(());
        }
    }
    let transaction = ProfileTransaction::new(&profile_dir)?;
    let staging_dir = transaction.staging_dir();

    // Create progress bar with modern styling (unless quiet)
    let pb = if quiet {
        None
    } else {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Valid template"),
        );
        bar.set_message("Saving profile...");
        bar.enable_steady_tick(Duration::from_millis(100));
        Some(bar)
    };

    // Extract email from existing auth.json if present
    let email = read_email_from_codex_dir(codex_dir).await;
    let auth_mode = detect_live_auth_mode(codex_dir).await;

    // Copy critical files
    let files_to_copy = get_critical_files();
    let copied = copy_profile_files(codex_dir, &staging_dir, files_to_copy)
        .with_context(|| "Failed to copy profile files")?;

    // Handle passphrase
    let secret_passphrase = passphrase.filter(|p| !p.is_empty());
    let is_encrypted = secret_passphrase.is_some();

    // Encrypt auth.json in-place only, preserving file permissions.
    if let Some(pass) = secret_passphrase.as_ref() {
        let auth_path = staging_dir.join("auth.json");
        if auth_path.exists() && auth_path.is_file() {
            let auth_content = tokio::fs::read(&auth_path)
                .await
                .context("Failed to read auth.json for encryption")?;
            let encrypted = crate::utils::crypto::encrypt(&auth_content, Some(pass))
                .context("Failed to encrypt auth.json")?;
            write_bytes_preserve_permissions(&auth_path, &encrypted)
                .context("Failed to write encrypted auth.json")?;
        }
    }

    // Save metadata without rewriting copied profile files.
    let mut meta = ProfileMeta::new(name.clone(), email.clone(), description.clone());
    meta.auth_mode = auth_mode;
    meta.encrypted = is_encrypted;
    meta.update();
    let mut meta_json = serde_json::to_vec_pretty(&meta).context("Failed to serialize metadata")?;
    meta_json.push(b'\n');
    let meta_path = staging_dir.join("profile.json");
    write_bytes_preserve_permissions(&meta_path, &meta_json)
        .context("Failed to write profile metadata")?;

    transaction.commit()?;

    if let Some(bar) = pb {
        bar.finish_and_clear();
    }

    // Success message
    if !quiet {
        let encryption_status = if is_encrypted {
            format!(" {}", "[encrypted]".cyan())
        } else {
            String::new()
        };
        println!(
            "{} Profile {} saved successfully{}",
            "✓".green().bold(),
            name.cyan(),
            encryption_status
        );

        if let Some(e) = email {
            println!("  {}: {}", "Email".dimmed(), e);
        }
        println!("  {}: {}", "Location".dimmed(), profile_dir.display());
        println!("  {}: {} files", "Files".dimmed(), copied.len());
    }

    Ok(())
}

async fn detect_live_auth_mode(codex_dir: &std::path::Path) -> String {
    let auth_path = codex_dir.join("auth.json");
    let Ok(content) = tokio::fs::read_to_string(&auth_path).await else {
        return "unknown".to_string();
    };
    let Ok(auth_json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return "unknown".to_string();
    };
    crate::utils::auth::detect_auth_mode(&auth_json)
}
