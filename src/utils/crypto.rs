//! Encryption utilities for secure profile storage
//!
//! Uses age encryption with passphrase-based keys.
//! Empty/None passphrase = no encryption (backward compatible).

use anyhow::{Context, Result};
use std::io::{Read, Write};

/// Encrypt plaintext with optional passphrase
///
/// If passphrase is None or empty, returns plaintext unchanged (no encryption).
pub fn encrypt(plaintext: &[u8], passphrase: Option<&String>) -> Result<Vec<u8>> {
    let passphrase_str = match passphrase {
        Some(p) if !p.is_empty() => p.as_str(),
        _ => {
            // No encryption - return plaintext
            return Ok(plaintext.to_vec());
        }
    };

    let encryptor =
        age::Encryptor::with_user_passphrase(secrecy::SecretString::from(passphrase_str));
    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .context("Failed to create age encryptor")?;
    writer
        .write_all(plaintext)
        .context("Failed to write encrypted data")?;
    writer.finish().context("Failed to finalize encryption")?;

    // Prepend marker to indicate encryption
    let mut result = b"age-encrypted:v1\n".to_vec();
    result.extend(encrypted);
    Ok(result)
}

/// Decrypt ciphertext with optional passphrase
///
/// If data is not encrypted (no marker), returns plaintext unchanged.
/// If data is encrypted and passphrase is None/empty, returns error.
pub fn decrypt(ciphertext: &[u8], passphrase: Option<&String>) -> Result<Vec<u8>> {
    let marker = b"age-encrypted:v1\n";

    if !ciphertext.starts_with(marker) {
        // Not encrypted - return as-is (backward compatible)
        return Ok(ciphertext.to_vec());
    }

    let actual_ciphertext = &ciphertext[marker.len()..];

    let passphrase_str = passphrase
        .filter(|p| !p.is_empty())
        .context("Profile is encrypted but no passphrase provided")?;

    let decryptor =
        age::Decryptor::new(actual_ciphertext).context("Failed to parse encrypted data")?;

    let mut plaintext = vec![];
    decryptor
        .decrypt(std::iter::once(
            &age::scrypt::Identity::new(secrecy::SecretString::from(passphrase_str.as_str())) as _,
        ))
        .context("Failed to decrypt - wrong passphrase?")?
        .read_to_end(&mut plaintext)
        .context("Failed to read decrypted data")?;

    Ok(plaintext)
}

/// Check if data is encrypted
pub fn is_encrypted(data: &[u8]) -> bool {
    data.starts_with(b"age-encrypted:v1\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_profile_created_with_age_0_11() {
        use base64::Engine as _;
        // Produced by codexctl with age 0.11.3 using synthetic credentials.
        let encrypted = base64::engine::general_purpose::STANDARD
            .decode(include_str!("../../tests/fixtures/age-0.11-auth.b64").trim())
            .unwrap();
        let decrypted = decrypt(&encrypted, Some(&"fixture-passphrase".to_string())).unwrap();
        assert_eq!(decrypted, br#"{"api_key":"test-legacy-fixture"}"#);
    }

    #[test]
    fn test_no_passphrase_no_encryption() {
        let plaintext = b"hello world";
        let encrypted = encrypt(plaintext, None).unwrap();
        assert_eq!(encrypted, plaintext);
        assert!(!is_encrypted(&encrypted));
    }

    #[test]
    fn test_empty_passphrase_no_encryption() {
        let plaintext = b"hello world";
        let passphrase = String::new();
        let encrypted = encrypt(plaintext, Some(&passphrase)).unwrap();
        assert_eq!(encrypted, plaintext);
        assert!(!is_encrypted(&encrypted));
    }

    #[test]
    fn test_encrypt_decrypt() {
        let plaintext = b"sensitive auth data";
        let passphrase = String::from("my secret password");

        let encrypted = encrypt(plaintext, Some(&passphrase)).unwrap();
        assert!(is_encrypted(&encrypted));
        assert_ne!(encrypted, plaintext.to_vec());

        let decrypted = decrypt(&encrypted, Some(&passphrase)).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_passphrase() {
        let plaintext = b"sensitive auth data";
        let passphrase = String::from("correct password");
        let wrong_passphrase = String::from("wrong password");

        let encrypted = encrypt(plaintext, Some(&passphrase)).unwrap();
        assert!(decrypt(&encrypted, Some(&wrong_passphrase)).is_err());
    }

    #[test]
    fn test_backward_compatible_plaintext() {
        // Old plaintext profiles should still load
        let plaintext = b"{\"token\": \"abc123\"}";
        let decrypted = decrypt(plaintext, None).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
