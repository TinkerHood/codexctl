//! Integration tests for structured JSON command output.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn status_json_reports_auth_mode_capabilities() {
    let fixture = CliFixture::new();
    fixture.install_fake_codex();
    fixture.write_live_auth(r#"{"api_key":"sk-test"}"#);
    fs::write(fixture.config_dir().join(".current_profile"), "api-work").unwrap();

    let output = fixture
        .command()
        .args(["status", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(payload["auth_mode"], "api_key");
    assert_eq!(payload["plan_claims_available"], false);
    assert_eq!(payload["api_realtime_available"], true);
    assert_eq!(payload["current_profile"], "api-work");
    assert_eq!(payload["codex_installed"], true);
}

#[test]
fn usage_all_json_includes_mixed_profile_modes() {
    let fixture = CliFixture::new();
    write_profile(
        &fixture.config_dir().join("api"),
        &serde_json::json!({"api_key":"sk-api"}),
        Some(serde_json::json!({
            "name": "api",
            "created_at": Utc::now(),
            "updated_at": Utc::now(),
            "email": serde_json::Value::Null,
            "description": serde_json::Value::Null,
            "auth_mode": "api_key",
            "version": env!("CARGO_PKG_VERSION"),
            "encrypted": false
        })),
    );
    write_profile(
        &fixture.config_dir().join("team"),
        &make_chatgpt_auth("team", 30, false),
        None,
    );

    let output = fixture
        .command()
        .args(["usage", "--all", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).unwrap();
    let profiles = payload
        .as_array()
        .expect("usage --all should return an array");
    assert_eq!(profiles.len(), 2);
    assert!(profiles.iter().any(|p| p["auth_mode"] == "api_key"));
    assert!(profiles.iter().any(|p| p["auth_mode"] == "chatgpt"));
}

#[test]
fn verify_json_distinguishes_locked_profiles() {
    let fixture = CliFixture::new();
    fixture.write_live_auth(r#"{"api_key":"sk-valid"}"#);

    fixture
        .command()
        .args(["save", "api-valid", "--force"])
        .assert()
        .success();

    fixture.write_live_auth(r#"{"api_key":"sk-locked"}"#);
    fixture
        .command()
        .args(["save", "api-locked", "--force", "--passphrase", "secret"])
        .assert()
        .success();

    let output = fixture
        .command()
        .args(["verify", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let payload: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(payload["valid"], 1);
    assert_eq!(payload["locked"], 1);
    assert_eq!(payload["invalid"], 0);

    let profiles = payload["profiles"].as_array().unwrap();
    assert!(profiles.iter().any(|p| p["status"] == "valid"));
    assert!(profiles.iter().any(|p| p["status"] == "locked"));
}

struct CliFixture {
    _temp: TempDir,
    home_dir: PathBuf,
    config_dir: PathBuf,
    bin_dir: PathBuf,
    path: String,
}

impl CliFixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let home_dir = temp.path().join("home");
        let config_dir = temp.path().join("profiles");
        let bin_dir = temp.path().join("bin");
        fs::create_dir_all(home_dir.join(".codex")).unwrap();
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        let existing_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{existing_path}", bin_dir.display());

        Self {
            _temp: temp,
            home_dir,
            config_dir,
            bin_dir,
            path,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::cargo_bin("codexctl").unwrap();
        cmd.env("HOME", &self.home_dir)
            .env("CODEXCTL_DIR", &self.config_dir)
            .env_remove("CODEXCTL_PASSPHRASE")
            .env_remove("CODEXCTL_QUIET")
            .env("PATH", &self.path);
        cmd
    }

    fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    fn write_live_auth(&self, auth: &str) {
        fs::write(self.home_dir.join(".codex").join("auth.json"), auth).unwrap();
    }

    fn install_fake_codex(&self) {
        let codex_path = self.bin_dir.join("codex");
        fs::write(&codex_path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&codex_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&codex_path, perms).unwrap();
        }
    }
}

#[test]
fn save_without_live_auth_preserves_existing_profile() {
    let fixture = CliFixture::new();
    let profile = fixture.config_dir().join("work");
    write_profile(&profile, &serde_json::json!({"api_key":"saved"}), None);
    let original = fs::read(profile.join("auth.json")).unwrap();
    fixture
        .command()
        .args(["save", "work", "--force"])
        .assert()
        .failure();
    assert_eq!(fs::read(profile.join("auth.json")).unwrap(), original);
}

#[test]
fn save_replaces_complete_profile_and_removes_stale_files() {
    let fixture = CliFixture::new();
    fixture.write_live_auth(r#"{"api_key":"replacement"}"#);
    let profile = fixture.config_dir().join("work");
    write_profile(&profile, &serde_json::json!({"api_key":"old"}), None);
    fs::write(profile.join("stale"), "old").unwrap();
    fixture
        .command()
        .args(["--quiet", "save", "work", "--force"])
        .assert()
        .success();
    assert_eq!(
        fs::read(profile.join("auth.json")).unwrap(),
        br#"{"api_key":"replacement"}"#
    );
    assert!(!profile.join("stale").exists());
    assert!(profile.join("profile.json").is_file());
}

#[test]
fn backup_rejects_traversal_and_existing_destinations() {
    let fixture = CliFixture::new();
    fixture.write_live_auth(r#"{"api_key":"original"}"#);
    fixture
        .command()
        .args(["backup", "--name", "../escape"])
        .assert()
        .failure();
    assert!(!fixture.config_dir().join("escape").exists());
    fixture
        .command()
        .args(["backup", "--name", "snapshot"])
        .assert()
        .success();
    fixture.write_live_auth(r#"{"api_key":"replacement"}"#);
    fixture
        .command()
        .args(["backup", "--name", "snapshot"])
        .assert()
        .failure();
    assert_eq!(
        fs::read(fixture.config_dir().join("backups/snapshot/auth.json")).unwrap(),
        br#"{"api_key":"original"}"#
    );
}

#[test]
fn profile_names_cannot_overwrite_backup_storage() {
    let fixture = CliFixture::new();
    fixture.write_live_auth(r#"{"api_key":"original"}"#);
    fixture
        .command()
        .args(["save", "backups", "--force"])
        .assert()
        .failure();
    assert!(fixture.config_dir().join("backups").is_dir());
    assert!(!fixture.config_dir().join("backups/auth.json").exists());
}

#[cfg(unix)]
#[test]
fn run_propagates_child_failure_and_restores_auth() {
    let fixture = CliFixture::new();
    write_profile(
        &fixture.config_dir().join("work"),
        &serde_json::json!({"api_key":"saved"}),
        None,
    );
    fixture.write_live_auth(r#"{"api_key":"original"}"#);
    fixture
        .command()
        .args([
            "--quiet",
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "exit 23",
        ])
        .assert()
        .code(23);
    assert_eq!(
        fs::read(fixture.home_dir.join(".codex/auth.json")).unwrap(),
        br#"{"api_key":"original"}"#
    );
}

#[test]
fn run_restores_auth_when_command_cannot_start() {
    let fixture = CliFixture::new();
    write_profile(
        &fixture.config_dir().join("work"),
        &serde_json::json!({"api_key":"saved"}),
        None,
    );
    fixture.write_live_auth(r#"{"api_key":"original"}"#);
    fixture
        .command()
        .args([
            "--quiet",
            "run",
            "--profile",
            "work",
            "--",
            "codexctl-nonexistent-test-command",
        ])
        .assert()
        .failure();
    assert_eq!(
        fs::read(fixture.home_dir.join(".codex/auth.json")).unwrap(),
        br#"{"api_key":"original"}"#
    );
}

#[cfg(unix)]
#[test]
fn run_reports_restore_failure_even_in_quiet_mode() {
    let fixture = CliFixture::new();
    write_profile(
        &fixture.config_dir().join("work"),
        &serde_json::json!({"api_key":"saved"}),
        None,
    );
    fixture.write_live_auth(r#"{"api_key":"original"}"#);
    let result = fixture
        .command()
        .args([
            "--quiet",
            "run",
            "--profile",
            "work",
            "--",
            "sh",
            "-c",
            "rm -- \"$HOME/.codex/auth.json\" && mkdir -- \"$HOME/.codex/auth.json\"",
        ])
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8_lossy(&result).contains("Could not restore original auth"));
}

fn write_profile(profile_dir: &Path, auth_json: &Value, meta_json: Option<Value>) {
    fs::create_dir_all(profile_dir).unwrap();
    fs::write(
        profile_dir.join("auth.json"),
        serde_json::to_vec_pretty(auth_json).unwrap(),
    )
    .unwrap();

    if let Some(meta) = meta_json {
        fs::write(
            profile_dir.join("profile.json"),
            serde_json::to_vec_pretty(&meta).unwrap(),
        )
        .unwrap();
    }
}

fn make_chatgpt_auth(plan: &str, days_until_expiry: i64, with_api_key: bool) -> Value {
    let now = Utc::now();
    let payload = serde_json::json!({
        "email": "user@example.com",
        "https://api.openai.com/auth": {
            "chatgpt_plan_type": plan,
            "chatgpt_subscription_active_start": now.to_rfc3339(),
            "chatgpt_subscription_active_until": (now + Duration::days(days_until_expiry)).to_rfc3339(),
            "chatgpt_account_id": "acct_test",
            "organizations": []
        }
    });
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
    let payload = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
    let token = format!("{header}.{payload}.sig");

    if with_api_key {
        serde_json::json!({
            "tokens": { "id_token": token },
            "api_key": "sk-test"
        })
    } else {
        serde_json::json!({
            "tokens": { "id_token": token }
        })
    }
}
