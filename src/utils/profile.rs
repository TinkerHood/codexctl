#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Profile metadata stored alongside the profile data
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub email: Option<String>,
    pub description: Option<String>,
    #[serde(default = "default_auth_mode")]
    pub auth_mode: String,
    pub version: String,
    pub encrypted: bool,
}

fn default_auth_mode() -> String {
    String::from("unknown")
}

impl ProfileMeta {
    #[must_use]
    pub fn new(name: String, email: Option<String>, description: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            name,
            created_at: now,
            updated_at: now,
            email,
            description,
            auth_mode: default_auth_mode(),
            version: String::from(env!("CARGO_PKG_VERSION")),
            encrypted: false,
        }
    }

    pub fn update(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// Full profile data including metadata and file list
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg(test)]
pub struct Profile {
    pub meta: ProfileMeta,
    pub files: std::collections::HashMap<String, Vec<u8>>, // filename -> content
}

#[cfg(test)]
impl Profile {
    #[must_use]
    pub fn new(name: String, email: Option<String>, description: Option<String>) -> Self {
        Self {
            meta: ProfileMeta::new(name, email, description),
            files: std::collections::HashMap::new(),
        }
    }

    pub fn add_file<P: AsRef<Path>>(&mut self, path: P, content: Vec<u8>) {
        let Some(filename) = path
            .as_ref()
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
        else {
            return;
        };
        self.files.insert(filename, content);
    }

    /// Save profile to disk
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or file writing fails
    #[cfg(test)]
    pub fn save_to_disk<P: AsRef<Path>>(&self, dir: P) -> Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create profile directory: {}", dir.display()))?;

        // Save metadata
        let meta_path = dir.join("profile.json");
        let meta_json = serde_json::to_string_pretty(&self.meta)
            .context("Failed to serialize profile metadata")?;
        crate::utils::files::write_bytes_preserve_permissions(&meta_path, meta_json.as_bytes())
            .with_context(|| format!("Failed to write metadata to {}", meta_path.display()))?;

        // Save files
        for (filename, content) in &self.files {
            let file_path = dir.join(filename);
            crate::utils::files::write_bytes_preserve_permissions(&file_path, content)
                .with_context(|| format!("Failed to write file: {}", file_path.display()))?;
        }

        Ok(())
    }

    /// Save profile to disk with optional encryption
    ///
    /// Encrypts sensitive files (auth.json) if passphrase is provided.
    /// # Errors
    pub fn save_to_disk_encrypted<P: AsRef<Path>>(
        &self,
        dir: P,
        passphrase: Option<&String>,
    ) -> Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Failed to create profile directory: {}", dir.display()))?;

        // Determine if we're encrypting
        let should_encrypt: bool = passphrase.is_some_and(|p| !p.is_empty());

        // Save metadata (always plaintext)
        let meta_path = dir.join("profile.json");
        let mut meta = self.meta.clone();
        meta.encrypted = should_encrypt;
        meta.update();
        let meta_json =
            serde_json::to_string_pretty(&meta).context("Failed to serialize profile metadata")?;
        crate::utils::files::write_bytes_preserve_permissions(&meta_path, meta_json.as_bytes())
            .with_context(|| format!("Failed to write metadata to {}", meta_path.display()))?;

        // Save files (encrypt auth.json if passphrase provided)
        for (filename, content) in &self.files {
            let file_path = dir.join(filename);

            let final_content = if should_encrypt && filename == "auth.json" {
                crate::utils::crypto::encrypt(content, passphrase)?
            } else {
                content.clone()
            };

            crate::utils::files::write_bytes_preserve_permissions(&file_path, &final_content)
                .with_context(|| format!("Failed to write file: {}", file_path.display()))?;
        }

        Ok(())
    }

    /// Load profile from disk with optional decryption
    ///
    /// Decrypts auth.json if profile is encrypted and passphrase is provided.
    /// # Errors
    /// Load profile from disk
    ///
    /// # Errors
    #[cfg(test)]
    pub fn load_from_disk<P: AsRef<Path>>(dir: P) -> Result<Self> {
        let dir = dir.as_ref();
        let meta_path = dir.join("profile.json");

        let meta_json = std::fs::read_to_string(&meta_path)
            .with_context(|| format!("Failed to read metadata from {}", meta_path.display()))?;
        let meta: ProfileMeta =
            serde_json::from_str(&meta_json).context("Failed to parse profile metadata")?;

        let mut files = std::collections::HashMap::new();

        // Load critical files using modern if-let chains and let-else
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip if not a file or if it's the metadata file
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            if name == std::ffi::OsStr::new("profile.json") {
                continue;
            }

            let filename = name.to_string_lossy().to_string();
            let content = std::fs::read(&path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?;
            files.insert(filename, content);
        }

        Ok(Self { meta, files })
    }

    /// Extract email from auth.json if available
    #[must_use]
    #[cfg(test)]
    pub fn extract_email(&self) -> Option<String> {
        let auth_content = self.files.get("auth.json")?;
        let auth_str = std::str::from_utf8(auth_content).ok()?;
        let auth_json: serde_json::Value = serde_json::from_str(auth_str).ok()?;
        crate::utils::auth::extract_email_from_auth_json(&auth_json)
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn test_profile_meta_new() {
        let meta = ProfileMeta::new(
            String::from("test-profile"),
            Some(String::from("test@example.com")),
            Some(String::from("Test description")),
        );

        assert_eq!(meta.name, "test-profile");
        assert_eq!(meta.email, Some("test@example.com".to_string()));
        assert_eq!(meta.description, Some("Test description".to_string()));
        assert_eq!(meta.auth_mode, "unknown");
        assert_eq!(meta.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_profile_meta_update() {
        let mut meta = ProfileMeta::new(String::from("test"), None, None);
        let original_updated = meta.updated_at;

        // Small delay to ensure time changes
        std::thread::sleep(std::time::Duration::from_millis(10));
        meta.update();

        assert!(meta.updated_at > original_updated);
    }

    #[test]
    fn test_profile_new() {
        let profile = Profile::new(
            String::from("my-profile"),
            Some(String::from("user@example.com")),
            Some(String::from("My description")),
        );

        assert_eq!(profile.meta.name, "my-profile");
        assert!(profile.files.is_empty());
    }

    #[test]
    fn test_profile_add_file() {
        let mut profile = Profile::new(String::from("test"), None, None);
        let content = b"test content".to_vec();

        profile.add_file("test.txt", content.clone());

        assert_eq!(profile.files.get("test.txt"), Some(&content));
    }

    #[test]
    fn test_profile_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let profile_dir = temp_dir.path().join("test-profile");

        // Create and save profile
        let mut profile = Profile::new(
            String::from("test-profile"),
            Some(String::from("test@example.com")),
            Some(String::from("Description")),
        );
        profile.add_file("auth.json", b"{\"token\": \"test\"}".to_vec());
        profile.add_file("config.toml", b"model = \"gpt-4\"".to_vec());

        profile.save_to_disk(&profile_dir).unwrap();

        // Verify files exist
        assert!(profile_dir.exists());
        assert!(profile_dir.join("profile.json").exists());
        assert!(profile_dir.join("auth.json").exists());
        assert!(profile_dir.join("config.toml").exists());

        // Load profile back
        let loaded = Profile::load_from_disk(&profile_dir).unwrap();
        assert_eq!(loaded.meta.name, "test-profile");
        assert_eq!(
            loaded.files.get("auth.json"),
            Some(&b"{\"token\": \"test\"}".to_vec())
        );
    }

    #[test]
    fn test_extract_email_from_jwt() {
        // Create a mock JWT token with email in payload
        let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
        let payload = URL_SAFE_NO_PAD.encode(b"{\"email\":\"jwt-test@example.com\"}");
        let signature = "dummy-signature";

        let mock_jwt = format!("{header}.{payload}.{signature}");

        let mock_auth = format!("{{\"tokens\": {{\"id_token\": \"{mock_jwt}\"}}}}");

        let mut profile = Profile::new(String::from("test"), None, None);
        profile.add_file("auth.json", mock_auth.into_bytes());

        let email = profile.extract_email();
        assert_eq!(email, Some("jwt-test@example.com".to_string()));
    }

    #[test]
    fn test_extract_email_no_auth() {
        let profile = Profile::new(String::from("test"), None, None);
        assert!(profile.extract_email().is_none());
    }

    #[test]
    fn test_extract_email_invalid_jwt() {
        let mock_auth = r#"{"tokens": {"id_token": "invalid.jwt.format"}}"#;

        let mut profile = Profile::new(String::from("test"), None, None);
        profile.add_file("auth.json", mock_auth.as_bytes().to_vec());

        // Should return None for invalid JWT
        assert!(profile.extract_email().is_none());
    }
}
