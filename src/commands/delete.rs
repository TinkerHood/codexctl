use crate::utils::config::Config;
use crate::utils::transaction::DirectoryLock;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use colored::Colorize as _;

pub async fn execute(config: Config, name: String, force: bool, quiet: bool) -> Result<()> {
    let profile_name = ProfileName::try_from(name.as_str())
        .with_context(|| format!("Invalid profile name '{name}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;

    let _profiles_lock = DirectoryLock::acquire(config.profiles_dir(), ".codexctl_profiles.lock")?;
    crate::utils::transaction::prepare_profile_delete(&profile_dir)?;

    if !profile_dir.exists() {
        anyhow::bail!("Profile '{name}' not found");
    }

    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Delete profile '{}' permanently?", name.yellow()))
            .default(false)
            .interact()?;

        if !confirm {
            if !quiet {
                println!("Cancelled");
            }
            return Ok(());
        }
    }

    tokio::fs::remove_dir_all(&profile_dir).await?;

    if !quiet {
        println!("{} Profile {} deleted", "✓".green().bold(), name.cyan());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delete_rejects_active_profile_writer() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let profile = dir.path().join("work");
        std::fs::create_dir(&profile).unwrap();
        std::fs::write(profile.join("auth.json"), b"original").unwrap();
        let _writer = DirectoryLock::acquire(dir.path(), ".codexctl_profiles.lock").unwrap();

        let error = execute(config, "work".to_string(), true, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Another codexctl operation"));
        assert_eq!(
            std::fs::read(profile.join("auth.json")).unwrap(),
            b"original"
        );
    }
}
