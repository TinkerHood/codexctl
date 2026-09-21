use crate::utils::config::Config;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use chrono::Local;
use colored::Colorize as _;

#[allow(clippy::needless_pass_by_value)]
pub fn execute(config: Config, name: Option<String>, quiet: bool) -> Result<()> {
    let codex_dir = config.codex_dir();
    let backup_dir = config.backup_dir();

    if !codex_dir.exists() {
        anyhow::bail!("Codex directory not found at {}", codex_dir.display());
    }

    let backup_path = if let Some(name) = name {
        let name = ProfileName::try_from(name.as_str()).context("Invalid backup name")?;
        let path = backup_dir.join(name.as_str());
        // Refuse existing names instead of merging into an older backup.
        std::fs::create_dir(&path).context("Failed to create backup; choose an unused name")?;
        if let Err(error) = crate::utils::files::copy_dir_recursive(codex_dir, &path) {
            let _ = std::fs::remove_dir_all(&path);
            return Err(error);
        }
        path
    } else {
        let prefix = Local::now().format("backup_%Y%m%d_%H%M%S_").to_string();
        let directory = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir_in(backup_dir)?;
        crate::utils::files::copy_dir_recursive(codex_dir, directory.path())?;
        directory.keep()
    };

    if !quiet {
        println!(
            "{} Backup created: {}",
            "✓".green().bold(),
            backup_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .cyan()
        );
        println!("  {}: {}", "Location".dimmed(), backup_path.display());
    }

    Ok(())
}
