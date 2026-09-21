use crate::utils::auth::read_email_from_codex_dir;
use crate::utils::config::Config;
use crate::utils::files::{create_auth_backup, write_bytes_preserve_permissions};
use crate::utils::transaction::DirectoryLock;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use colored::Colorize as _;
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::Value;
use std::time::Duration;

pub async fn execute(
    config: Config,
    name: String,
    force: bool,
    dry_run: bool,
    quiet: bool,
    passphrase: Option<String>,
) -> Result<()> {
    if dry_run {
        // A dry run only reads profiles; it must not create CODEX_HOME just
        // to hold a lock that protects writes.
        let selected = match name.as_str() {
            "-" => previous_profile_name(&config, quiet).await?,
            "auto" => match auto_switch(&config, true, quiet, passphrase.as_ref()).await? {
                Some(name) => name,
                None => return Ok(()),
            },
            _ => name,
        };
        do_load(config, selected, force, true, quiet, passphrase).await?;
        return Ok(());
    }

    // Fail before creating CODEX_HOME for names that cannot be loaded. The
    // selection is checked again after locking, so a concurrent change still
    // gets the authoritative result under the lock.
    if name != "-" && name != "auto" {
        let validated = ProfileName::try_from(name.as_str())
            .with_context(|| format!("Invalid profile name '{name}'"))?;
        if !config.profile_path_validated(&validated)?.exists() {
            anyhow::bail!(
                "Profile '{name}' not found. Use 'codexctl list' to see available profiles."
            );
        }
    } else if name == "-" && !config.profiles_dir().join(".previous_profile").exists() {
        anyhow::bail!(
            "No previous profile. Switch to a profile first before using 'codexctl load -'"
        );
    } else if name == "auto" && !config.profiles_dir().exists() {
        anyhow::bail!(
            "No profiles directory found. Create profiles first with: codexctl save <name>"
        );
    }

    // Selection, the auth swap, and both tracking markers form one locked operation.
    let _auth_lock = DirectoryLock::acquire(config.codex_dir(), ".codexctl_auth.lock")?;
    crate::commands::run::recover_interrupted_run_locked(config.codex_dir())?;

    let selected = match name.as_str() {
        "-" => previous_profile_name(&config, quiet).await?,
        "auto" => match auto_switch(&config, dry_run, quiet, passphrase.as_ref()).await? {
            Some(name) => name,
            None => return Ok(()),
        },
        _ => name,
    };
    let current_profile = get_current_profile_name(&config).await;

    let switched = do_load(
        config.clone(),
        selected.clone(),
        force,
        dry_run,
        quiet,
        passphrase,
    )
    .await?;

    record_switch_if_applied(&config, current_profile.as_deref(), &selected, switched).await;
    Ok(())
}

async fn record_switch_if_applied(
    config: &Config,
    previous: Option<&str>,
    selected: &str,
    switched: bool,
) {
    if !switched {
        return;
    }
    if let Some(previous) = previous {
        let _ = save_previous_profile(config, previous).await;
    }
    let _ = save_current_profile(config, selected).await;
}

/// Internal load implementation
#[allow(clippy::too_many_lines)]
async fn do_load(
    config: Config,
    name: String,
    force: bool,
    dry_run: bool,
    quiet: bool,
    passphrase: Option<String>,
) -> Result<bool> {
    let profile_name = ProfileName::try_from(name.as_str())
        .with_context(|| format!("Invalid profile name '{name}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;
    let codex_dir = config.codex_dir();

    if !profile_dir.exists() {
        anyhow::bail!("Profile '{name}' not found. Use 'codexctl list' to see available profiles.");
    }

    // Load profile metadata
    let meta_path = profile_dir.join("profile.json");
    let meta: crate::utils::profile::ProfileMeta = if meta_path.exists() {
        let content = tokio::fs::read_to_string(&meta_path).await?;
        serde_json::from_str(&content)
            .unwrap_or_else(|_| crate::utils::profile::ProfileMeta::new(name.clone(), None, None))
    } else {
        crate::utils::profile::ProfileMeta::new(name.clone(), None, None)
    };

    if dry_run {
        if !quiet {
            println!(
                "{} Dry run: Would load profile '{}'",
                "ℹ".blue(),
                name.cyan()
            );
            if let Some(e) = &meta.email {
                println!("  {}: {}", "Email".dimmed(), e);
            }
            println!(
                "  {}: {}",
                "Profile directory".dimmed(),
                profile_dir.display()
            );
            println!(
                "  {}: {}",
                "`Codex` directory".dimmed(),
                codex_dir.display()
            );
        }
        return Ok(false);
    }

    if !force && codex_dir.exists() && !quiet {
        let current_email = read_email_from_codex_dir(codex_dir).await;
        let target_email = meta.email.clone();

        if let (Some(current_email), Some(target_email)) = (current_email, target_email)
            && current_email != target_email
        {
            let confirm = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "Switch from {} to {}?",
                    current_email.yellow(),
                    target_email.green()
                ))
                .default(true)
                .interact()?;

            if !confirm {
                println!("Cancelled");
                return Ok(false);
            }
        }
    }

    // Create progress bar (unless quiet)
    let pb = if quiet {
        None
    } else {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .expect("Valid template"),
        );
        bar.set_message("Loading profile...");
        bar.enable_steady_tick(Duration::from_millis(100));
        Some(bar)
    };

    // Backup the live auth file before switching.
    if codex_dir.exists() {
        let backup_dir = config.backup_dir();
        let backup_path =
            create_auth_backup(codex_dir, backup_dir).context("Failed to create auth backup")?;
        if let (Some(bar), Some(path)) = (pb.as_ref(), backup_path) {
            bar.set_message(format!(
                "Backed up auth to {}...",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
        }
    }

    // Handle encrypted profiles
    let secret_passphrase = passphrase.filter(|p| !p.is_empty());

    // Load/decrypt auth.json from the profile.
    let auth_path = profile_dir.join("auth.json");
    if !auth_path.exists() {
        anyhow::bail!("Profile '{name}' does not contain auth.json");
    }
    let auth_content = tokio::fs::read(&auth_path).await?;
    let auth_to_apply = if crate::utils::crypto::is_encrypted(&auth_content) {
        crate::utils::crypto::decrypt(&auth_content, secret_passphrase.as_ref())
            .context("Failed to decrypt auth.json - wrong passphrase?")?
    } else {
        auth_content
    };

    // Keep all existing codex state (sessions/history/etc.) and only replace auth.json.
    tokio::fs::create_dir_all(codex_dir)
        .await
        .with_context(|| format!("Failed to create codex directory: {}", codex_dir.display()))?;
    if let Some(ref bar) = pb {
        bar.set_message("Switching auth profile...");
    }
    let target_auth = codex_dir.join("auth.json");
    write_bytes_preserve_permissions(&target_auth, &auth_to_apply)
        .context("Failed to write auth.json to codex directory")?;

    if let Some(bar) = pb {
        bar.finish_and_clear();
    }

    // Log to history
    let _ = crate::commands::history::log_command(&config, &name, "load").await;

    // Success message
    if !quiet {
        let encryption_status = if meta.encrypted {
            format!(" {}", "[encrypted]".cyan())
        } else {
            String::new()
        };
        println!(
            "{} Profile {} loaded successfully{}",
            "✓".green().bold(),
            name.cyan(),
            encryption_status
        );

        if let Some(e) = &meta.email {
            println!("  {}: {}", "Email".dimmed(), e.green());
        }
        println!(
            "  {}: {}",
            "Last saved".dimmed(),
            meta.updated_at.format("%Y-%m-%d %H:%M:%S")
        );
    }

    Ok(true)
}

/// Auto-switch to the best available profile based on quota/usage
#[allow(clippy::too_many_lines)]
async fn auto_switch(
    config: &Config,
    dry_run: bool,
    quiet: bool,
    passphrase: Option<&String>,
) -> Result<Option<String>> {
    use crate::utils::auth::extract_usage_info;

    let profiles_dir = config.profiles_dir();
    if !profiles_dir.exists() {
        anyhow::bail!(
            "No profiles directory found. Create profiles first with: codexctl save <name>"
        );
    }

    let mut entries = tokio::fs::read_dir(profiles_dir).await?;
    let mut profiles_with_usage = Vec::new();

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if name == "backups" || name.starts_with('.') {
            continue;
        }

        let auth_path = path.join("auth.json");
        if !auth_path.exists() {
            continue;
        }

        let auth_json = read_profile_auth_json(&auth_path, passphrase).await;

        if let Some(auth) = auth_json
            && let Ok(usage) = extract_usage_info(&auth)
        {
            let score = calculate_profile_score(&usage);
            profiles_with_usage.push((name, usage, score, path));
        }
    }

    if profiles_with_usage.is_empty() {
        anyhow::bail!("No profiles with valid usage information found");
    }

    profiles_with_usage.sort_by_key(|profile| std::cmp::Reverse(profile.2));

    if !quiet {
        println!("{}", "🔄 Auto Profile Switcher".cyan().bold());
        println!();
        println!("Available profiles (sorted by quota availability):");
        for (i, (name, usage, score, _)) in profiles_with_usage.iter().enumerate() {
            let indicator = if i == 0 { "→".green() } else { " ".into() };
            let plan_emoji = match usage.plan_type.as_str() {
                "team" => "👥",
                "enterprise" => "🏢",
                _ => "👤",
            };
            println!(
                "  {} {} {} {} (Score: {}, {} days remaining)",
                indicator,
                plan_emoji,
                name.cyan(),
                usage.email.dimmed(),
                score,
                usage
                    .subscription_end
                    .as_ref()
                    .and_then(|end| calculate_days_remaining(end).ok())
                    .map_or_else(|| "N/A".to_string(), |d| d.to_string())
            );
        }
        println!();
    }

    let (best_name, best_usage, _, _) = &profiles_with_usage[0];

    if dry_run {
        if !quiet {
            println!("{} Would auto-switch to: {}", "ℹ".blue(), best_name.cyan());
        }
        return Ok(None);
    }

    let codex_dir = config.codex_dir();
    if let Some(current_email) = read_email_from_codex_dir(codex_dir).await
        && current_email == best_usage.email
    {
        if !quiet {
            println!(
                "{} Already using the best profile: {} ({})",
                "✓".green(),
                best_name.cyan(),
                best_usage.email.green()
            );
        }
        return Ok(None);
    }

    if !quiet {
        println!(
            "{} Auto-switching to best profile: {} ({})",
            "→".cyan(),
            best_name.cyan(),
            best_usage.email.green()
        );
    }

    Ok(Some(best_name.clone()))
}

/// Read `auth.json` from a profile, decrypting when needed.
///
/// Returns `None` if the file cannot be read, decrypted, or parsed.
async fn read_profile_auth_json(
    auth_path: &std::path::Path,
    passphrase: Option<&String>,
) -> Option<Value> {
    let raw = tokio::fs::read(auth_path).await.ok()?;
    let plain = if crate::utils::crypto::is_encrypted(&raw) {
        crate::utils::crypto::decrypt(&raw, passphrase).ok()?
    } else {
        raw
    };
    serde_json::from_slice::<Value>(&plain).ok()
}

/// Calculate a score for profile priority (higher = better)
#[allow(clippy::cast_possible_truncation)]
fn calculate_profile_score(usage: &crate::utils::auth::UsageInfo) -> i32 {
    let mut score = 0;

    score += match usage.plan_type.as_str() {
        "enterprise" => 100,
        "team" => 50,
        _ => 0,
    };

    if let Some(end) = &usage.subscription_end
        && let Ok(days) = calculate_days_remaining(end)
    {
        score += days.min(30) as i32;
    }

    score
}

fn calculate_days_remaining(iso_date: &str) -> anyhow::Result<i64> {
    use chrono::{DateTime, Utc};

    let end_date = DateTime::parse_from_rfc3339(iso_date)
        .map_err(|e| anyhow::anyhow!("Failed to parse date: {e}"))?;

    let now = Utc::now();
    let duration = end_date.with_timezone(&Utc) - now;

    Ok(duration.num_days())
}

/// Get the name of the currently loaded profile
async fn get_current_profile_name(config: &Config) -> Option<String> {
    let marker = config.profiles_dir().join(".current_profile");
    if marker.exists()
        && let Ok(content) = tokio::fs::read_to_string(&marker).await
    {
        let name = content.trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    // Fallback: try to identify from email in auth.json
    let codex_dir = config.codex_dir();
    if let Some(email) = read_email_from_codex_dir(codex_dir).await
        && let Ok(mut entries) = tokio::fs::read_dir(config.profiles_dir()).await
    {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir()
                || path
                    .file_name()
                    .is_none_or(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }

            let name = path.file_name()?.to_string_lossy().to_string();
            let meta_path = path.join("profile.json");
            if let Ok(content) = tokio::fs::read_to_string(&meta_path).await
                && let Ok(meta) =
                    serde_json::from_str::<crate::utils::profile::ProfileMeta>(&content)
                && meta.email.as_ref() == Some(&email)
            {
                return Some(name);
            }
        }
    }

    None
}

/// Save the current profile name as "previous" for quick-switch
async fn save_previous_profile(config: &Config, name: &str) -> anyhow::Result<()> {
    let marker = config.profiles_dir().join(".previous_profile");
    tokio::fs::write(&marker, name).await?;
    Ok(())
}

/// Save the current profile name as "current"
async fn save_current_profile(config: &Config, name: &str) -> anyhow::Result<()> {
    let marker = config.profiles_dir().join(".current_profile");
    tokio::fs::write(&marker, name).await?;
    Ok(())
}

/// Load the previous profile (quick-switch with `-`)
async fn previous_profile_name(config: &Config, quiet: bool) -> Result<String> {
    let marker = config.profiles_dir().join(".previous_profile");

    if !marker.exists() {
        anyhow::bail!(
            "No previous profile. Switch to a profile first before using 'codexctl load -'"
        );
    }

    let previous_name = tokio::fs::read_to_string(&marker).await?;
    let previous_name = previous_name.trim();

    if previous_name.is_empty() {
        anyhow::bail!("No previous profile recorded");
    }

    if !quiet {
        println!(
            "{} Quick-switching to previous profile: {}",
            "↔".cyan(),
            previous_name.cyan()
        );
    }

    Ok(previous_name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_and_invalid_profile_do_not_create_codex_home() {
        let dir = tempfile::tempdir().unwrap();
        let codex_dir = dir.path().join("codex-home");
        let config = Config::for_test(dir.path().join("profiles"), codex_dir.clone()).unwrap();
        std::fs::create_dir(config.profiles_dir().join("ready")).unwrap();

        execute(config.clone(), "ready".into(), false, true, true, None)
            .await
            .unwrap();
        assert!(!codex_dir.exists());

        assert!(
            execute(
                config.clone(),
                "invalid/name".into(),
                false,
                false,
                true,
                None
            )
            .await
            .is_err()
        );
        assert!(!codex_dir.exists());

        assert!(
            execute(config, "missing".into(), false, false, true, None)
                .await
                .is_err()
        );
        assert!(!codex_dir.exists());
    }

    #[tokio::test]
    async fn no_op_load_keeps_both_tracking_markers() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        tokio::fs::write(dir.path().join(".current_profile"), "current")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join(".previous_profile"), "previous")
            .await
            .unwrap();

        record_switch_if_applied(&config, Some("current"), "canceled", false).await;

        assert_eq!(
            tokio::fs::read_to_string(dir.path().join(".current_profile"))
                .await
                .unwrap(),
            "current"
        );
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join(".previous_profile"))
                .await
                .unwrap(),
            "previous"
        );
    }

    #[tokio::test]
    async fn completed_load_records_previous_and_current_under_one_decision() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        record_switch_if_applied(&config, Some("first"), "second", true).await;
        assert_eq!(previous_profile_name(&config, true).await.unwrap(), "first");
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join(".current_profile"))
                .await
                .unwrap(),
            "second"
        );
    }
}
