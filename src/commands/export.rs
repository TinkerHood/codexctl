use crate::utils::config::Config;
use crate::utils::files::write_bytes_preserve_permissions;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use colored::Colorize as _;

pub async fn execute(config: Config, name: String, quiet: bool) -> Result<()> {
    let profile_name = ProfileName::try_from(name.as_str())
        .with_context(|| format!("Invalid profile name '{name}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;

    if !profile_dir.exists() {
        anyhow::bail!("Profile '{name}' not found");
    }

    // Create tarball
    let export_filename = format!("{name}.export.txt");
    let tarball = create_tarball(&profile_dir)?;

    // Compress (gzip)
    let compressed = compress(&tarball)?;

    // Encode base64
    let encoded = STANDARD.encode(&compressed);

    if !quiet {
        println!("{} Profile {} exported\n", "✓".green().bold(), name.cyan());
    }

    println!("{encoded}");

    // Also save to file
    let export_path = private_exports_dir(config.profiles_dir())?.join(export_filename);
    write_export_file(&export_path, encoded.as_bytes())?;

    if !quiet {
        println!("\n  {}: {}", "Saved to".dimmed(), export_path.display());
        println!(
            "  {}: Copy the base64 string above to import on another machine",
            "Tip".yellow()
        );
    }

    Ok(())
}

fn compress(data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data)?;
    Ok(encoder.finish()?)
}

fn private_exports_dir(profiles_dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let path = profiles_dir.join(".exports");
    let create_result = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            std::fs::DirBuilder::new().mode(0o700).create(&path)
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir(&path)
        }
    };
    match create_result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Failed to create exports directory"),
    }

    let metadata =
        std::fs::symlink_metadata(&path).context("Failed to inspect exports directory")?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("Exports path is not a directory: {}", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("Exports directory is not private: {}", path.display());
        }
    }
    Ok(path)
}

fn write_export_file(path: &std::path::Path, data: &[u8]) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file() {
                anyhow::bail!(
                    "Export destination is not a regular file: {}",
                    path.display()
                );
            }
            // An older export may have been written with default public file
            // permissions. Restrict it before the atomic replacement preserves
            // those permissions.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let mut permissions = metadata.permissions();
                permissions.set_mode(0o600);
                std::fs::set_permissions(path, permissions)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Failed to inspect export destination"),
    }
    write_bytes_preserve_permissions(path, data)
}

fn is_root_export(path: &std::path::Path, root: &std::path::Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative.components().count() == 1
            && relative
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".export.txt"))
    })
}

fn create_tarball(dir: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Cursor;
    use tar::Builder;

    let mut buf = Vec::new();
    {
        let cursor = Cursor::new(&mut buf);
        let mut builder = Builder::new(cursor);
        builder.append_dir(".", dir)?;
        for entry in walkdir::WalkDir::new(dir)
            .min_depth(1)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_file() || !is_root_export(entry.path(), dir)
            })
        {
            let entry = entry?;
            let relative = entry.path().strip_prefix(dir)?;
            let archive_path = std::path::Path::new(".").join(relative);
            if entry.file_type().is_dir() {
                builder.append_dir(&archive_path, entry.path())?;
            } else {
                builder.append_path_with_name(entry.path(), &archive_path)?;
            }
        }
        builder.finish()?;
    }

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn tarball_omits_legacy_export_and_keeps_profile_contents() {
        let temp = tempfile::tempdir().unwrap();
        let profile = temp.path().join("work");
        std::fs::create_dir(&profile).unwrap();
        std::fs::create_dir(profile.join("sessions")).unwrap();
        std::fs::write(profile.join("auth.json"), "test auth").unwrap();
        std::fs::write(profile.join("sessions/session.json"), "test session").unwrap();
        std::fs::write(profile.join("work.export.txt"), "old export").unwrap();
        std::fs::write(profile.join("renamed.export.txt"), "older export").unwrap();
        std::fs::write(profile.join("sessions/keep.export.txt"), "session data").unwrap();
        std::fs::create_dir(profile.join("notes.export.txt")).unwrap();
        std::fs::write(profile.join("notes.export.txt/note.md"), "keep me").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("auth.json", profile.join("alias.export.txt")).unwrap();

        let tarball = create_tarball(&profile).unwrap();
        let mut archive = tar::Archive::new(tarball.as_slice());
        let mut paths = Vec::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().into_owned();
            if path.ends_with("auth.json") {
                let mut contents = String::new();
                entry.read_to_string(&mut contents).unwrap();
                assert_eq!(contents, "test auth");
            }
            paths.push(path);
        }

        assert!(paths.iter().any(|path| path.ends_with("auth.json")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("sessions/session.json"))
        );
        assert!(!paths.iter().any(|path| path.ends_with("work.export.txt")));
        assert!(
            !paths
                .iter()
                .any(|path| path.ends_with("renamed.export.txt"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("sessions/keep.export.txt"))
        );
        assert!(paths.iter().any(|path| path.ends_with("notes.export.txt")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("notes.export.txt/note.md"))
        );
        #[cfg(unix)]
        assert!(paths.iter().any(|path| path.ends_with("alias.export.txt")));
    }

    #[tokio::test]
    async fn export_saves_in_reserved_directory_without_profile_collision() {
        let temp = tempfile::tempdir().unwrap();
        let config = Config::new(Some(temp.path().to_path_buf())).unwrap();
        let profile = temp.path().join("work");
        std::fs::create_dir(&profile).unwrap();
        std::fs::write(profile.join("auth.json"), "test auth").unwrap();
        let colliding_profile = temp.path().join("work.export.txt");
        std::fs::create_dir(&colliding_profile).unwrap();
        std::fs::write(colliding_profile.join("auth.json"), "other auth").unwrap();

        execute(config, "work".to_string(), true).await.unwrap();

        assert!(temp.path().join(".exports/work.export.txt").is_file());
        assert_eq!(
            std::fs::read(colliding_profile.join("auth.json")).unwrap(),
            b"other auth"
        );
        assert!(colliding_profile.is_dir());
        assert!(!profile.join("work.export.txt").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(temp.path().join(".exports"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn exports_directory_cannot_be_a_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join(".exports")).unwrap();

        assert!(private_exports_dir(temp.path()).is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn export_file_is_private_even_when_replacing_public_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("work.export.txt");

        write_export_file(&path, b"first").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_export_file(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
