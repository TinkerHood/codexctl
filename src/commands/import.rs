use crate::utils::config::Config;
use crate::utils::transaction::ProfileTransaction;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use colored::Colorize as _;
use std::path::Path;

pub async fn execute(config: Config, name: String, data: String, quiet: bool) -> Result<()> {
    let profile_name = ProfileName::try_from(name.as_str())
        .with_context(|| format!("Invalid profile name '{name}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;
    let transaction = ProfileTransaction::new(&profile_dir)?;

    if profile_dir.exists() {
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

    import_into_transaction(&data, transaction).await?;

    if !quiet {
        println!(
            "{} Profile {} imported successfully",
            "✓".green().bold(),
            name.cyan()
        );
        println!("  {}: {}", "Location".dimmed(), profile_dir.display());
    }

    Ok(())
}

#[cfg(test)]
async fn import_profile(data: &str, profile_dir: &Path) -> Result<()> {
    import_into_transaction(data, ProfileTransaction::new(profile_dir)?).await
}

async fn import_into_transaction(data: &str, transaction: ProfileTransaction) -> Result<()> {
    // Decode and extract completely before replacing an existing profile.
    let decoded = STANDARD
        .decode(data)
        .with_context(|| "Failed to decode base64 data")?;

    // Decompress (gzip)
    let decompressed = decompress(&decoded)?;

    // Parse as tarball and extract
    extract_tarball(&decompressed, &transaction.staging_dir()).await?;
    let auth_path = transaction.staging_dir().join("auth.json");
    let auth = std::fs::symlink_metadata(&auth_path)
        .context("Imported profile must contain a regular auth.json file")?;
    if !auth.is_file() {
        anyhow::bail!("Imported profile must contain a regular auth.json file");
    }
    transaction.commit()?;

    Ok(())
}

fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut result = Vec::new();
    decoder.read_to_end(&mut result)?;
    Ok(result)
}

async fn extract_tarball(data: &[u8], dest: &Path) -> Result<()> {
    use std::io::Cursor;
    use tar::Archive;

    let cursor = Cursor::new(data);
    let mut archive = Archive::new(cursor);

    tokio::fs::create_dir_all(dest).await?;
    archive.set_preserve_permissions(true);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?;
        if !entry_path.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            anyhow::bail!(
                "Unsafe path in tarball: {} (path traversal rejected)",
                entry_path.display()
            );
        }
        entry.unpack_in(dest)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[tokio::test]
    async fn import_rejects_active_profile_writer_before_decision() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(Some(dir.path().to_path_buf())).unwrap();
        let _writer = crate::utils::transaction::DirectoryLock::acquire(
            dir.path(),
            ".codexctl_profiles.lock",
        )
        .unwrap();

        let error = execute(config, "work".to_string(), String::new(), true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Another codexctl operation"));
        assert!(!dir.path().join("work").exists());
    }

    #[tokio::test]
    async fn invalid_import_preserves_existing_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("work");
        std::fs::create_dir(&profile).unwrap();
        std::fs::write(profile.join("auth.json"), "original").unwrap();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(b"not a tar archive").unwrap();
        let mut empty_tar = tar::Builder::new(Vec::new());
        empty_tar.finish().unwrap();
        let mut empty_gzip =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        empty_gzip
            .write_all(&empty_tar.into_inner().unwrap())
            .unwrap();
        for invalid in [
            "!invalid-base64!".to_string(),
            STANDARD.encode(gzip.finish().unwrap()),
            STANDARD.encode(empty_gzip.finish().unwrap()),
        ] {
            assert!(import_profile(&invalid, &profile).await.is_err());
            assert_eq!(
                std::fs::read(profile.join("auth.json")).unwrap(),
                b"original"
            );
            assert!(!dir.path().join(".codexctl_original_work").exists());
        }
    }
}
