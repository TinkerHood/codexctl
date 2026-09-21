use crate::utils::config::Config;
use crate::utils::files::write_bytes_preserve_permissions;
use crate::utils::transaction::DirectoryLock;
use crate::utils::validation::ProfileName;
use anyhow::{Context as _, Result};
use colored::Colorize as _;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::{SigSet, SigmaskHow, Signal, killpg, pthread_sigmask};
#[cfg(unix)]
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
#[cfg(unix)]
use nix::unistd::{Pid, getpgrp, getpid, tcgetpgrp, tcsetpgrp};
use std::fs::File;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{ExitStatus, Stdio};
#[cfg(not(unix))]
use tokio::process::Command;

pub async fn execute(
    config: Config,
    profile: String,
    passphrase: Option<String>,
    command: Vec<String>,
    quiet: bool,
) -> Result<ExitStatus> {
    let profile_name = ProfileName::try_from(profile.as_str())
        .with_context(|| format!("Invalid profile name '{profile}'"))?;
    let profile_dir = config.profile_path_validated(&profile_name)?;
    let codex_dir = config.codex_dir();

    if !profile_dir.exists() {
        anyhow::bail!("Profile '{profile}' not found");
    }

    if command.is_empty() {
        anyhow::bail!("No command specified to run");
    }

    let profile_auth = load_profile_auth(&profile_dir, &profile, passphrase.as_ref()).await?;
    #[cfg(unix)]
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(windows)]
    let mut interrupt = tokio::signal::windows::ctrl_c()?;
    let _auth_lock = DirectoryLock::acquire(codex_dir, ".codexctl_auth.lock")?;
    recover_locked(codex_dir)?;
    let journal = apply_profile_auth(codex_dir, &profile_auth)?;

    #[cfg(test)]
    if let Some(ready) = std::env::var_os("CODEXCTL_TEST_PAUSE_AFTER_SWAP") {
        std::fs::write(ready, b"selected")?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // Execute command
    let cmd = &command[0];
    let args = &command[1..];

    if !quiet {
        println!(
            "{} Running with profile {}: {}",
            "▶".cyan(),
            profile.green(),
            command.join(" ").dimmed()
        );
    }

    // Log to history
    let _ = crate::commands::history::log_command(&config, &profile, &command.join(" ")).await;

    #[cfg(unix)]
    let mut child_command = std::process::Command::new(cmd);
    #[cfg(not(unix))]
    let mut child_command = Command::new(cmd);
    child_command
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit());
    #[cfg(unix)]
    child_command.process_group(0);
    #[cfg(not(unix))]
    child_command.kill_on_drop(true);
    let child = match child_command.spawn() {
        Ok(child) => child,
        Err(error) => {
            restore_original_auth(codex_dir, &journal)?;
            return Err(error).with_context(|| format!("Failed to execute command: {cmd}"));
        }
    };

    #[cfg(unix)]
    let status_result = wait_for_unix_child(child, &mut interrupt, &mut termination).await;
    #[cfg(not(unix))]
    let status_result = wait_for_other_child(child, &mut interrupt).await;

    // If the child could still be running, retain the journal and the selected
    // auth; a later invocation can recover once the command has stopped.
    let status = status_result.with_context(|| {
        format!("Could not prove command {cmd} stopped; auth recovery journal retained")
    })?;

    // Always restore original auth after command execution.
    restore_original_auth(codex_dir, &journal)
        .context("Could not restore original auth after command execution")?;

    if !quiet {
        if status.success() {
            println!(
                "\n{} Command completed, restored original auth",
                "✓".green()
            );
        } else {
            println!(
                "\n{} Command exited with code {:?}",
                "!".yellow(),
                status.code()
            );
        }
    }

    Ok(status)
}

#[cfg(unix)]
fn kill_child_group(group: Pid) -> Result<()> {
    match killpg(group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(error).context("Failed to stop command process group"),
    }
}

#[cfg(unix)]
fn set_foreground(group: Pid) -> Result<()> {
    // A background process normally receives SIGTTOU from tcsetpgrp. Block it
    // only for this synchronous call so restoration can run after handoff.
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut previous = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))?;
    let changed = tcsetpgrp(std::io::stdin(), group);
    let unblocked = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None);
    changed?;
    unblocked?;
    Ok(())
}

#[cfg(unix)]
struct ForegroundTerminal(Option<Pid>);

#[cfg(unix)]
impl ForegroundTerminal {
    fn handoff(group: Pid) -> Result<Self> {
        let original = match tcgetpgrp(std::io::stdin()) {
            Ok(group) => group,
            Err(Errno::ENOTTY) => return Ok(Self(None)),
            Err(error) => return Err(error).context("Failed to inspect terminal foreground group"),
        };
        if original != getpgrp() {
            return Ok(Self(None));
        }
        set_foreground(group).context("Failed to hand terminal to command")?;
        // The child may have attempted a read before the foreground handoff.
        let _ = killpg(group, Signal::SIGCONT);
        Ok(Self(Some(original)))
    }

    fn restore(&self) -> Result<()> {
        if let Some(group) = self.0 {
            set_foreground(group).context("Failed to restore terminal foreground group")?;
        }
        Ok(())
    }
}

#[cfg(unix)]
async fn wait_for_unix_child(
    mut child: std::process::Child,
    interrupt: &mut tokio::signal::unix::Signal,
    termination: &mut tokio::signal::unix::Signal,
) -> Result<ExitStatus> {
    let group = Pid::from_raw(i32::try_from(child.id())?);
    let terminal = match ForegroundTerminal::handoff(group) {
        Ok(terminal) => terminal,
        Err(error) => {
            // A very short command may have exited before the foreground
            // handoff. Its status is enough to prove it cannot use auth.
            if let Some(status) = child.try_wait()? {
                kill_child_group(group)?;
                return Ok(status);
            }
            kill_child_group(group)?;
            let _ = child.wait();
            return Err(error);
        }
    };
    let mut terminal = terminal;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(20));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (status, group_stopped): (Result<ExitStatus>, bool) = loop {
        tokio::select! {
            _ = poll.tick() => {
                let observed = match waitpid(group, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED)) {
                    Ok(status) => status,
                    Err(error) => break (Err(error).context("Failed to inspect command status"), false),
                };
                match observed {
                    WaitStatus::Exited(_, code) => break (Ok(ExitStatus::from_raw(code << 8)), false),
                    WaitStatus::Signaled(_, signal, core) => {
                        let raw = signal as i32 | if core { 0x80 } else { 0 };
                        break (Ok(ExitStatus::from_raw(raw)), false);
                    }
                    WaitStatus::Stopped(_, _) => {
                        if let Err(error) = terminal.restore() {
                            break (Err(error), false);
                        }
                        // The npm launcher and this process share a shell job.
                        // Stop that whole job so the shell can regain control.
                        let stopped = if terminal.0.is_some() {
                            killpg(getpgrp(), Signal::SIGSTOP)
                        } else {
                            nix::sys::signal::kill(getpid(), Signal::SIGSTOP)
                        };
                        if let Err(error) = stopped {
                            break (Err(error).context("Failed to stop wrapper job with command"), false);
                        }
                        terminal = match ForegroundTerminal::handoff(group) {
                            Ok(terminal) => terminal,
                            Err(error) => break (Err(error), false),
                        };
                        if let Err(error) = killpg(group, Signal::SIGCONT) {
                            break (Err(error).context("Failed to resume command process group"), false);
                        }
                    }
                    WaitStatus::Continued(_) | WaitStatus::StillAlive => {}
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    WaitStatus::PtraceEvent(_, _, _) | WaitStatus::PtraceSyscall(_) => {
                        // No tracer is expected for a command we launched. Stop
                        // its group and retain recovery state if one appears.
                        break (Err(anyhow::anyhow!("Unexpected traced command status")), false);
                    }
                }
            },
            _ = interrupt.recv() => {
                break match kill_child_group(group) {
                    Ok(()) => (child.wait().context("Failed to wait for command"), true),
                    Err(error) => (Err(error), false),
                };
            },
            _ = termination.recv() => {
                break match kill_child_group(group) {
                    Ok(()) => (child.wait().context("Failed to wait for command"), true),
                    Err(error) => (Err(error), false),
                };
            },
        }
    };
    // A wrapper can exit while descendants remain. Stop the whole group before
    // putting the original credentials back into the shared Codex home.
    let group_result = if group_stopped {
        Ok(())
    } else {
        kill_child_group(group)
    };
    let terminal_result = terminal.restore();
    group_result?;
    terminal_result?;
    status
}

#[cfg(not(unix))]
async fn wait_for_other_child(
    mut child: tokio::process::Child,
    interrupt: &mut tokio::signal::windows::CtrlC,
) -> Result<ExitStatus> {
    tokio::select! {
        status = child.wait() => status.context("Failed to wait for command"),
        _ = interrupt.recv() => {
            child.kill().await.context("Failed to stop command")?;
            child.wait().await.context("Failed to wait for command")
        }
    }
}

async fn load_profile_auth(
    profile_dir: &Path,
    profile_name: &str,
    passphrase: Option<&String>,
) -> Result<Vec<u8>> {
    let profile_auth_path = profile_dir.join("auth.json");
    if !profile_auth_path.exists() {
        anyhow::bail!("Profile '{profile_name}' does not contain auth.json");
    }

    let profile_auth = tokio::fs::read(&profile_auth_path)
        .await
        .with_context(|| format!("Failed to read {}", profile_auth_path.display()))?;

    if crate::utils::crypto::is_encrypted(&profile_auth) {
        return crate::utils::crypto::decrypt(&profile_auth, passphrase)
            .with_context(|| format!("Failed to decrypt profile '{profile_name}'"));
    }

    Ok(profile_auth)
}

const JOURNAL_NAME: &str = ".codexctl_run_auth_recovery";

struct AuthJournal {
    path: std::path::PathBuf,
    original: Option<Vec<u8>>,
    selected: Vec<u8>,
}

/// Repair an interrupted run unless an active run still owns the auth lock.
pub fn recover_interrupted_run(codex_dir: &Path) -> Result<()> {
    if !codex_dir.exists() {
        return Ok(());
    }
    let Some(_lock) = DirectoryLock::try_acquire(codex_dir, ".codexctl_auth.lock")? else {
        return Ok(());
    };
    recover_locked(codex_dir)
}

fn recover_locked(codex_dir: &Path) -> Result<()> {
    let journal = codex_dir.join(JOURNAL_NAME);
    if !journal.exists() {
        return cleanup_pending(codex_dir);
    }
    let selected = std::fs::read(journal.join("selected"))
        .context("Interrupted run journal is incomplete; auth left untouched")?;
    let original = if journal.join("absent").exists() {
        None
    } else {
        Some(
            std::fs::read(journal.join("original"))
                .context("Interrupted run journal is incomplete; auth left untouched")?,
        )
    };
    let current = read_optional_auth(&codex_dir.join("auth.json"))?;
    if current == original {
        // The run died before the swap or after restoring it.
    } else if current.as_deref() == Some(selected.as_slice()) {
        restore_bytes(codex_dir, original.as_deref())?;
    } else {
        anyhow::bail!(
            "Interrupted run auth at {} changed outside codexctl; recovery copy retained at {}",
            codex_dir.join("auth.json").display(),
            journal.display()
        );
    }
    std::fs::remove_dir_all(&journal).with_context(|| {
        format!(
            "Failed to clear auth recovery journal at {}",
            journal.display()
        )
    })?;
    sync_dir(codex_dir)?;
    cleanup_pending(codex_dir)
}

fn cleanup_pending(codex_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(codex_dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".codexctl_run_pending_")
            && entry.file_type()?.is_dir()
        {
            std::fs::remove_dir_all(entry.path()).with_context(|| {
                format!(
                    "Failed to remove interrupted auth staging at {}",
                    entry.path().display()
                )
            })?;
        }
    }
    Ok(())
}

/// Recover while the caller already holds `.codexctl_auth.lock`.
pub(crate) fn recover_interrupted_run_locked(codex_dir: &Path) -> Result<()> {
    recover_locked(codex_dir)
}

fn read_optional_auth(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn apply_profile_auth(codex_dir: &Path, profile_auth: &[u8]) -> Result<AuthJournal> {
    std::fs::create_dir_all(codex_dir)
        .with_context(|| format!("Failed to create codex directory: {}", codex_dir.display()))?;

    let auth_path = codex_dir.join("auth.json");
    let original_auth = read_optional_auth(&auth_path)?;
    let pending = tempfile::Builder::new()
        .prefix(".codexctl_run_pending_")
        .tempdir_in(codex_dir)?;
    if let Some(original) = &original_auth {
        std::fs::write(pending.path().join("original"), original)?;
        File::open(pending.path().join("original"))?.sync_all()?;
    } else {
        std::fs::write(pending.path().join("absent"), [])?;
        File::open(pending.path().join("absent"))?.sync_all()?;
    }
    std::fs::write(pending.path().join("selected"), profile_auth)?;
    File::open(pending.path().join("selected"))?.sync_all()?;
    sync_dir(pending.path())?;
    let journal_path = codex_dir.join(JOURNAL_NAME);
    std::fs::rename(pending.path(), &journal_path)
        .context("Failed to publish auth recovery journal")?;
    sync_dir(codex_dir)?;

    let install = write_bytes_preserve_permissions(&auth_path, profile_auth)
        .with_context(|| format!("Failed to apply profile auth to {}", auth_path.display()))
        .and_then(|()| sync_dir(codex_dir));
    if let Err(error) = install {
        recover_locked(codex_dir).context("Could not recover failed auth swap")?;
        return Err(error);
    }
    #[cfg(test)]
    if std::env::var_os("CODEXCTL_TEST_ABORT_AFTER_AUTH_SWAP").is_some() {
        std::process::abort();
    }

    Ok(AuthJournal {
        path: journal_path,
        original: original_auth,
        selected: profile_auth.to_vec(),
    })
}

fn restore_bytes(codex_dir: &Path, original_auth: Option<&[u8]>) -> Result<()> {
    let auth_path = codex_dir.join("auth.json");
    match original_auth {
        Some(content) => write_bytes_preserve_permissions(&auth_path, content)
            .with_context(|| format!("Failed to restore {}", auth_path.display()))?,
        None => match std::fs::remove_file(&auth_path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to remove {}", auth_path.display()));
            }
        },
    }
    sync_dir(codex_dir)?;
    Ok(())
}

fn restore_original_auth(codex_dir: &Path, journal: &AuthJournal) -> Result<()> {
    let current = read_optional_auth(&codex_dir.join("auth.json"))?;
    if current != journal.original && current.as_deref() != Some(journal.selected.as_slice()) {
        anyhow::bail!(
            "Auth changed while the command ran; current auth left untouched and recovery copy retained at {}",
            journal.path.display()
        );
    }
    if current != journal.original {
        restore_bytes(codex_dir, journal.original.as_deref())?;
    }
    std::fs::remove_dir_all(&journal.path)?;
    sync_dir(codex_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn npm_launcher_fixture(dir: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::symlink;

        let launcher = dir.join("npm/codexctl.js");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/npm/codexctl.js"),
            &launcher,
        )
        .unwrap();
        let platform_arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => panic!("Unexpected architecture: {other}"),
        };
        let platform_os = if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        let binary = dir.join(format!(
            "npm/node_modules/@codexctl/{platform_os}-{platform_arch}/bin/codexctl"
        ));
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        symlink(std::env::current_exe().unwrap(), binary).unwrap();
        launcher
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn early_sigint_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_EARLY_SIGINT_DIR") else {
            return;
        };
        let path = Path::new(&path);
        let config = Config::for_test(path.join("profiles"), path.join("codex-home")).unwrap();
        let status = execute(
            config.clone(),
            "selected".into(),
            None,
            vec!["sh".into(), "-c".into(), "sleep 30".into()],
            true,
        )
        .await
        .unwrap();
        assert!(!status.success());
        assert_eq!(
            std::fs::read(config.codex_dir().join("auth.json")).unwrap(),
            b"original"
        );
        assert!(!config.codex_dir().join(JOURNAL_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn sigint_between_auth_swap_and_spawn_restores_original() {
        let dir = TempDir::new().unwrap();
        let codex_dir = dir.path().join("codex-home");
        std::fs::create_dir(&codex_dir).unwrap();
        std::fs::write(codex_dir.join("auth.json"), b"original").unwrap();
        let profile = dir.path().join("profiles/selected");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("auth.json"), b"selected").unwrap();
        let ready = dir.path().join("after-swap");
        let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "commands::run::tests::early_sigint_helper"])
            .env("CODEXCTL_TEST_EARLY_SIGINT_DIR", dir.path())
            .env("CODEXCTL_TEST_PAUSE_AFTER_SWAP", &ready)
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready.exists(), "helper did not reach the auth swap");
        assert_eq!(
            std::fs::read(codex_dir.join("auth.json")).unwrap(),
            b"selected"
        );
        nix::sys::signal::kill(
            Pid::from_raw(i32::try_from(helper.id()).unwrap()),
            Signal::SIGINT,
        )
        .unwrap();
        assert!(helper.wait().unwrap().success());
        assert_eq!(
            std::fs::read(codex_dir.join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[tokio::test]
    async fn test_load_profile_auth_decrypts_encrypted_profile() {
        let dir = TempDir::new().unwrap();
        let plaintext = br#"{"api_key":"sk-test"}"#.to_vec();
        let encrypted =
            crate::utils::crypto::encrypt(&plaintext, Some(&"secret".to_string())).unwrap();
        tokio::fs::write(dir.path().join("auth.json"), encrypted)
            .await
            .unwrap();

        let auth = load_profile_auth(dir.path(), "encrypted", Some(&"secret".to_string()))
            .await
            .unwrap();
        assert_eq!(auth, plaintext);
    }

    #[test]
    fn test_restore_original_auth_removes_temp_auth_when_none() {
        let dir = TempDir::new().unwrap();
        let journal = apply_profile_auth(dir.path(), b"temp").unwrap();
        restore_original_auth(dir.path(), &journal).unwrap();
        assert!(!dir.path().join("auth.json").exists());
    }

    #[test]
    fn test_apply_profile_auth_preserves_original_auth() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();

        let journal = apply_profile_auth(dir.path(), b"replacement").unwrap();
        assert_eq!(journal.original, Some(b"original".to_vec()));
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"replacement"
        );
        restore_original_auth(dir.path(), &journal).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn crash_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_RUN_DIR") else {
            return;
        };
        let _lock = DirectoryLock::acquire(Path::new(&path), ".codexctl_auth.lock").unwrap();
        apply_profile_auth(Path::new(&path), b"selected").unwrap();
        if let Some(ready) = std::env::var_os("CODEXCTL_TEST_RUN_READY") {
            std::fs::write(ready, b"ready").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    }

    #[test]
    fn aborted_run_restores_original_on_next_invocation() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "commands::run::tests::crash_helper"])
            .env("CODEXCTL_TEST_RUN_DIR", dir.path())
            .env("CODEXCTL_TEST_ABORT_AFTER_AUTH_SWAP", "1")
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"selected"
        );
        recover_interrupted_run(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
        assert!(!dir.path().join(JOURNAL_NAME).exists());
    }

    #[test]
    fn active_run_prevents_concurrent_recovery() {
        let dir = TempDir::new().unwrap();
        let _lock = DirectoryLock::acquire(dir.path(), ".codexctl_auth.lock").unwrap();
        let journal = apply_profile_auth(dir.path(), b"selected").unwrap();
        recover_interrupted_run(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"selected"
        );
        restore_original_auth(dir.path(), &journal).unwrap();
    }

    #[test]
    fn killed_run_rejects_concurrent_writer_then_recovers() {
        let dir = TempDir::new().unwrap();
        let ready = dir.path().join("ready");
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "commands::run::tests::crash_helper"])
            .env("CODEXCTL_TEST_RUN_DIR", dir.path())
            .env("CODEXCTL_TEST_RUN_READY", &ready)
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if ready.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(ready.exists(), "child did not install selected auth");
        assert!(DirectoryLock::acquire(dir.path(), ".codexctl_auth.lock").is_err());
        recover_interrupted_run(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"selected"
        );
        child.kill().unwrap();
        child.wait().unwrap();
        recover_interrupted_run(dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[test]
    fn changed_auth_is_not_overwritten() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let journal = apply_profile_auth(dir.path(), b"selected").unwrap();
        std::fs::write(dir.path().join("auth.json"), b"other login").unwrap();
        assert!(restore_original_auth(dir.path(), &journal).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"other login"
        );
        assert!(dir.path().join(JOURNAL_NAME).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_shell_descendant_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_SIGTERM_DIR") else {
            return;
        };
        let path = Path::new(&path);
        let _lock = DirectoryLock::acquire(path, ".codexctl_auth.lock").unwrap();
        let journal = apply_profile_auth(path, b"selected").unwrap();
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg("sh \"$1/monitor.sh\" \"$1\" & echo $! > \"$1/descendant.pid\"; wait")
            .arg("sh")
            .arg(path)
            .process_group(0);
        let child = command.spawn().unwrap();
        wait_for_unix_child(child, &mut interrupt, &mut termination)
            .await
            .unwrap();
        restore_original_auth(path, &journal).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_stops_shell_descendant_before_auth_restore() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        std::fs::write(
            dir.path().join("monitor.sh"),
            b"while :; do\n  if grep -q original \"$1/auth.json\"; then touch \"$1/violation\"; fi\n  sleep 0.05\ndone\n",
        )
        .unwrap();
        let mut helper = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::run::tests::sigterm_shell_descendant_helper",
            ])
            .env("CODEXCTL_TEST_SIGTERM_DIR", dir.path())
            .spawn()
            .unwrap();
        let descendant = dir.path().join("descendant.pid");
        for _ in 0..100 {
            if descendant.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !descendant.exists() {
            let _ = helper.kill();
            let _ = helper.wait();
            panic!("shell descendant did not start");
        }
        nix::sys::signal::kill(
            Pid::from_raw(i32::try_from(helper.id()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        assert!(helper.wait().unwrap().success());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
        assert!(!dir.path().join("violation").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn sigterm_to_node_launcher_waits_for_auth_cleanup() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        std::fs::write(
            dir.path().join("monitor.sh"),
            b"while :; do\n  if grep -q original \"$1/auth.json\"; then touch \"$1/violation\"; fi\n  sleep 0.05\ndone\n",
        )
        .unwrap();
        let launcher = npm_launcher_fixture(dir.path());
        let mut node = std::process::Command::new("node")
            .arg(launcher)
            .args([
                "--exact",
                "commands::run::tests::sigterm_shell_descendant_helper",
            ])
            .env("CODEXCTL_TEST_SIGTERM_DIR", dir.path())
            .spawn()
            .unwrap();
        let descendant = dir.path().join("descendant.pid");
        for _ in 0..100 {
            if descendant.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !descendant.exists() {
            let _ = node.kill();
            let _ = node.wait();
            panic!("launcher did not start the Rust command");
        }
        nix::sys::signal::kill(
            Pid::from_raw(i32::try_from(node.id()).unwrap()),
            Signal::SIGTERM,
        )
        .unwrap();
        assert!(node.wait().unwrap().success());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
        assert!(!dir.path().join(JOURNAL_NAME).exists());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!dir.path().join("violation").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fast_exit_pty_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_FAST_PTY_DIR") else {
            return;
        };
        let path = Path::new(&path);
        let _lock = DirectoryLock::acquire(path, ".codexctl_auth.lock").unwrap();
        let journal = apply_profile_auth(path, b"selected").unwrap();
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut command = std::process::Command::new("true");
        command.process_group(0);
        assert_eq!(tcgetpgrp(std::io::stdin()).unwrap(), getpgrp());
        let child = command.spawn().unwrap();
        let status = wait_for_unix_child(child, &mut interrupt, &mut termination)
            .await
            .unwrap();
        assert!(status.success());
        assert_eq!(tcgetpgrp(std::io::stdin()).unwrap(), getpgrp());
        restore_original_auth(path, &journal).unwrap();
        std::fs::write(path.join("fast-exit-ok"), b"ok").unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn fast_exit_command_restores_foreground_pty_and_auth() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let status = std::process::Command::new("script")
            .args(["-q", "/dev/null"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "commands::run::tests::fast_exit_pty_helper"])
            .env("CODEXCTL_TEST_FAST_PTY_DIR", dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(dir.path().join("fast-exit-ok").exists());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stopped_pty_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_STOP_PTY_DIR") else {
            return;
        };
        let path = Path::new(&path);
        let _lock = DirectoryLock::acquire(path, ".codexctl_auth.lock").unwrap();
        let journal = apply_profile_auth(path, b"selected").unwrap();
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg("kill -STOP $$; echo resumed > \"$1/resumed\"")
            .arg("sh")
            .arg(path)
            .process_group(0);
        let child = command.spawn().unwrap();
        assert!(
            wait_for_unix_child(child, &mut interrupt, &mut termination)
                .await
                .unwrap()
                .success()
        );
        assert_eq!(tcgetpgrp(std::io::stdin()).unwrap(), getpgrp());
        restore_original_auth(path, &journal).unwrap();
        std::fs::write(path.join("finished"), b"ok").unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ctrl_z_pty_helper() {
        let Some(path) = std::env::var_os("CODEXCTL_TEST_CTRL_Z_DIR") else {
            return;
        };
        let path = Path::new(&path);
        let _lock = DirectoryLock::acquire(path, ".codexctl_auth.lock").unwrap();
        let journal = apply_profile_auth(path, b"selected").unwrap();
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
        let mut termination =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg("echo ready > \"$1/ready\"; while [ ! -e \"$1/continue\" ]; do sleep 0.02; done; echo resumed > \"$1/resumed\"")
            .arg("sh")
            .arg(path)
            .process_group(0);
        let child = command.spawn().unwrap();
        assert!(
            wait_for_unix_child(child, &mut interrupt, &mut termination)
                .await
                .unwrap()
                .success()
        );
        assert_eq!(tcgetpgrp(std::io::stdin()).unwrap(), getpgrp());
        restore_original_auth(path, &journal).unwrap();
        std::fs::write(path.join("finished"), b"ok").unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stopped_child_returns_terminal_to_shell_and_fg_resumes_it() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let status = std::process::Command::new("script")
            .args(["-q", "/dev/null", "bash", "--noprofile", "--norc", "-ic"])
            .arg("\"$CODEXCTL_TEST_STOP_PTY_EXE\" --exact commands::run::tests::stopped_pty_helper; fg")
            .env("CODEXCTL_TEST_STOP_PTY_EXE", std::env::current_exe().unwrap())
            .env("CODEXCTL_TEST_STOP_PTY_DIR", dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(dir.path().join("resumed").exists());
        assert!(dir.path().join("finished").exists());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ctrl_z_then_fg_resumes_foreground_command() {
        use std::io::Write as _;

        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("auth.json"), b"original").unwrap();
        let launcher = npm_launcher_fixture(dir.path());
        let mut script = std::process::Command::new("script")
            .args(["-q", "/dev/null", "bash", "--noprofile", "--norc", "-ic"])
            .arg("node \"$CODEXCTL_TEST_NPM_LAUNCHER\" --exact commands::run::tests::ctrl_z_pty_helper; fg && touch \"$CODEXCTL_TEST_CTRL_Z_DIR/fg-done\"")
            .env("CODEXCTL_TEST_NPM_LAUNCHER", launcher)
            .env("CODEXCTL_TEST_CTRL_Z_DIR", dir.path())
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if dir.path().join("ready").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(dir.path().join("ready").exists());
        script.stdin.as_mut().unwrap().write_all(&[0x1a]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::fs::write(dir.path().join("continue"), b"go").unwrap();
        assert!(script.wait().unwrap().success());
        assert!(dir.path().join("fg-done").exists());
        assert!(dir.path().join("finished").exists());
        assert_eq!(
            std::fs::read(dir.path().join("auth.json")).unwrap(),
            b"original"
        );
    }
}
