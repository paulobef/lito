//! Lito Exec - runs a [`CommandSpec`] the policy has already cleared.
//!
//! This crate does no policy checks of its own: [`run`] assumes its caller has
//! already put the spec through `lito_policy::CommandPolicy::judge` (and, for a
//! `NeedsApproval` verdict, obtained approval). It is only concerned with
//! running the process safely:
//!
//! - `stdin` is `/dev/null` - a command can never block waiting on input.
//! - the environment is **rebuilt, not inherited** - only `PATH`, `HOME`,
//!   `USER`, `LANG`, and `TERM=dumb` are passed through, so a parent-process
//!   API key can't leak into a subprocess (privacy-first).
//! - output is capped per stream at [`MAX_OUTPUT_BYTES`]; the rest is drained
//!   but dropped, and `truncated` is set.
//! - the command is killed after [`COMMAND_TIMEOUT`].
//! - a non-zero exit code is an `Ok(CommandOutcome)`, not an `Err` - it's a
//!   result, not a failure. Only a spawn failure or a timeout is an `Err`.

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lito_policy::CommandSpec;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Wall-clock limit for a single command. On expiry the process is killed and
/// [`run`] returns an `Err`.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-stream cap on captured output. Beyond this the stream keeps being
/// drained (so the child doesn't block on a full pipe) but the bytes are
/// discarded and `truncated` is set.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// The result of running a command that actually started.
#[derive(Debug, Clone)]
pub struct CommandOutcome {
    /// `None` if the process was terminated by a signal rather than exiting.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// True if either stream was capped at [`MAX_OUTPUT_BYTES`].
    pub truncated: bool,
    pub duration: Duration,
}

impl CommandOutcome {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Run `spec` with the default [`COMMAND_TIMEOUT`].
pub async fn run(spec: &CommandSpec) -> Result<CommandOutcome> {
    run_with_timeout(spec, COMMAND_TIMEOUT).await
}

/// Run `spec` with an explicit timeout. Exposed mainly so tests don't have to
/// wait 120s to exercise the timeout path.
pub async fn run_with_timeout(spec: &CommandSpec, timeout: Duration) -> Result<CommandOutcome> {
    if spec.argv.is_empty() {
        anyhow::bail!("cannot run an empty command");
    }

    let mut cmd = build_command(spec);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", spec.program()))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let out_task = tokio::spawn(read_capped(stdout));
    let err_task = tokio::spawn(read_capped(stderr));

    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.context("failed to wait on the command")?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            // The reader tasks end once the pipes close after the kill.
            let _ = out_task.await;
            let _ = err_task.await;
            anyhow::bail!(
                "command timed out after {}s and was killed",
                timeout.as_secs()
            );
        }
    };

    let (stdout, out_truncated) = out_task.await.context("stdout reader panicked")??;
    let (stderr, err_truncated) = err_task.await.context("stderr reader panicked")??;

    Ok(CommandOutcome {
        exit_code: status.code(),
        stdout,
        stderr,
        truncated: out_truncated || err_truncated,
        duration: started.elapsed(),
    })
}

/// Build the `tokio::process::Command` for a spec, including the rebuilt
/// environment and the `shell: true` wrapping.
fn build_command(spec: &CommandSpec) -> Command {
    let mut cmd = if spec.shell {
        let mut c = Command::new("/bin/sh");
        c.arg("-c").arg(spec.argv.join(" "));
        c
    } else {
        let mut c = Command::new(&spec.argv[0]);
        c.args(&spec.argv[1..]);
        c
    };

    cmd.current_dir(&spec.cwd);
    cmd.env_clear();
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    cmd.env("PATH", path);
    if let Some(home) = dirs_home() {
        cmd.env("HOME", home);
    }
    if let Ok(user) = std::env::var("USER") {
        cmd.env("USER", user);
    }
    cmd.env(
        "LANG",
        std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into()),
    );
    cmd.env("TERM", "dumb");
    cmd.kill_on_drop(true);
    cmd
}

/// `$HOME` without pulling `dirs` into this crate just for one lookup.
fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Read a stream, keeping at most [`MAX_OUTPUT_BYTES`] but continuing to drain
/// past that so the child never blocks on a full pipe. Returns the captured
/// text (lossy UTF-8) and whether anything was dropped.
async fn read_capped<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<(String, bool)> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if buf.len() < MAX_OUTPUT_BYTES {
            let room = MAX_OUTPUT_BYTES - buf.len();
            let take = n.min(room);
            buf.extend_from_slice(&chunk[..take]);
            if take < n {
                truncated = true;
            }
        } else {
            truncated = true;
        }
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec(argv: &[&str], shell: bool) -> CommandSpec {
        CommandSpec::new(
            argv.iter().map(|s| s.to_string()).collect(),
            shell,
            std::env::temp_dir(),
        )
    }

    #[tokio::test]
    async fn captures_stdout_and_zero_exit() {
        let out = run(&spec(&["echo", "hello world"], false)).await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(out.stdout.trim(), "hello world");
        assert!(out.stderr.is_empty());
        assert!(!out.truncated);
        assert!(out.success());
    }

    #[tokio::test]
    async fn nonzero_exit_is_ok_not_err() {
        let out = run(&spec(&["sh", "-c", "exit 3"], false)).await.unwrap();
        assert_eq!(out.exit_code, Some(3));
        assert!(!out.success());
    }

    #[tokio::test]
    async fn argv_mode_does_not_interpret_shell_metacharacters() {
        // In argv mode `$(...)` and `>` are literal arguments to `echo`.
        let out = run(&spec(
            &["echo", "$(whoami)", ">", "/tmp/should-not-exist-xyz"],
            false,
        ))
        .await
        .unwrap();
        assert!(out.stdout.contains("$(whoami)"));
        assert!(!std::path::Path::new("/tmp/should-not-exist-xyz").exists());
    }

    #[tokio::test]
    async fn shell_mode_does_interpret_a_pipe() {
        let out = run(&spec(&["printf 'a\\nb\\nc\\n' | wc -l"], true))
            .await
            .unwrap();
        assert_eq!(out.stdout.trim(), "3");
    }

    #[tokio::test]
    async fn environment_is_rebuilt_not_inherited() {
        std::env::set_var("LITO_EXEC_SECRET", "leaked");
        let out = run(&spec(&["sh", "-c", "echo \"[$LITO_EXEC_SECRET]\""], false))
            .await
            .unwrap();
        std::env::remove_var("LITO_EXEC_SECRET");
        assert_eq!(out.stdout.trim(), "[]");
    }

    #[tokio::test]
    async fn stdin_is_null_so_a_reader_gets_eof_immediately() {
        let out = run(&spec(&["cat"], false)).await.unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.is_empty());
    }

    #[tokio::test]
    async fn output_is_capped_and_flagged() {
        let out = run(&spec(&["sh", "-c", "yes x | head -c 200000"], false))
            .await
            .unwrap();
        assert!(out.truncated);
        assert!(out.stdout.len() <= MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn timeout_kills_and_errors() {
        let err = run_with_timeout(&spec(&["sleep", "10"], false), Duration::from_millis(150))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }
}
