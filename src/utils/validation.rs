use std::fmt;

use anyhow::{Result, bail};

/// A validated profile name that cannot escape the profile directory.
///
/// Rejects:
/// - Empty strings
/// - Path separators (`/`, `\`)
/// - Traversal sequences (`..`)
/// - Internal directories and command aliases (`backups`, `auto`, `-`, dot-prefixed names)
/// - Control characters (ASCII < 0x20 or DEL 0x7F)
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileName(String);

impl ProfileName {
    /// Returns the validated profile name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

fn validate(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Profile name cannot be empty");
    }
    if name.starts_with('.') || name.eq_ignore_ascii_case("backups") || matches!(name, "auto" | "-")
    {
        bail!("Profile name '{name}' is reserved");
    }
    if name.contains("..") {
        bail!("Profile name must not contain traversal sequences '..'");
    }
    for ch in name.chars() {
        if ch == '/' || ch == '\\' {
            bail!("Profile name must not contain path separators ('/' or '\\')");
        }
        if (ch as u32) < 0x20 || ch == '\x7f' {
            bail!("Profile name must not contain control characters");
        }
    }
    Ok(())
}

impl TryFrom<String> for ProfileName {
    type Error = anyhow::Error;

    fn try_from(s: String) -> Result<Self> {
        validate(&s)?;
        Ok(Self(s))
    }
}

impl TryFrom<&str> for ProfileName {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> Result<Self> {
        validate(s)?;
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_simple_name() {
        assert!(ProfileName::try_from("work").is_ok());
    }

    #[test]
    fn valid_name_with_hyphens_and_underscores() {
        assert!(ProfileName::try_from("my-profile_123").is_ok());
    }

    #[test]
    fn valid_name_with_at_sign() {
        assert!(ProfileName::try_from("alice@example.com").is_ok());
    }

    #[test]
    fn valid_name_with_single_dot() {
        // Single dots not at start are fine — only "." alone and ".." are reserved
        assert!(ProfileName::try_from("profile.v2").is_ok());
    }

    #[test]
    fn rejects_empty() {
        assert!(ProfileName::try_from("").is_err());
    }

    #[test]
    fn rejects_forward_slash() {
        assert!(ProfileName::try_from("a/b").is_err());
        assert!(ProfileName::try_from("../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_backslash() {
        assert!(ProfileName::try_from("a\\b").is_err());
    }

    #[test]
    fn rejects_double_dot_alone() {
        assert!(ProfileName::try_from("..").is_err());
    }

    #[test]
    fn rejects_double_dot_embedded() {
        assert!(ProfileName::try_from("a..b").is_err());
    }

    #[test]
    fn rejects_reserved_dot() {
        assert!(ProfileName::try_from(".").is_err());
    }

    #[test]
    fn rejects_internal_names_and_command_aliases() {
        for name in [
            "backups",
            "Backups",
            ".current_profile",
            ".codexctl_profile_staging",
            "auto",
            "-",
        ] {
            assert!(
                ProfileName::try_from(name).is_err(),
                "accepted reserved name {name}"
            );
        }
        assert!(ProfileName::try_from("Auto").is_ok());
    }

    #[test]
    fn rejects_null_byte() {
        assert!(ProfileName::try_from("a\0b").is_err());
    }

    #[test]
    fn rejects_newline() {
        assert!(ProfileName::try_from("a\nb").is_err());
    }

    #[test]
    fn rejects_escape() {
        assert!(ProfileName::try_from("a\x1bb").is_err());
    }

    #[test]
    fn rejects_del() {
        assert!(ProfileName::try_from("a\x7fb").is_err());
    }

    #[test]
    fn as_str_returns_inner() {
        let name = ProfileName::try_from("hello").unwrap();
        assert_eq!(name.as_str(), "hello");
    }

    #[test]
    fn display_matches_inner() {
        let name = ProfileName::try_from("hello").unwrap();
        assert_eq!(name.to_string(), "hello");
    }
}
