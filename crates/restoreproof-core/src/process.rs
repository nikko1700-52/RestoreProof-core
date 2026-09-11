//! Sandboxed execution of external programs.
//!
//! Every external tool `RestoreProof` runs — `docker`, `restic`, `borg`, a
//! user's check script — goes through this module. The policy it enforces is
//! the same in all cases:
//!
//! * **No shell.** A program and its arguments are passed as an `argv` array.
//!   There is no code path that builds a command line string, so shell
//!   metacharacters in a configuration value are inert.
//! * **No inherited environment.** The child starts from an empty environment
//!   to which only an explicit, audited set of variables is added. A credential
//!   that happens to sit in the operator's shell never leaks into a check.
//! * **No inherited stdin.** Standard input is `/dev/null`, so a tool that
//!   decides to prompt for a passphrase fails fast instead of hanging forever.
//! * **Bounded time.** Every execution has a timeout and the child is killed
//!   when it expires.
//! * **Bounded output.** Output is captured up to a ceiling; the process is
//!   still drained so it cannot block on a full pipe, but the extra bytes are
//!   discarded rather than buffered.
//! * **Killed on drop.** If the runner unwinds, children do not survive it.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;

/// Fallback `PATH` used when the parent process has none.
const FALLBACK_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Errors raised while running an external program.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The program is not installed or not on `PATH`.
    #[error("`{program}` was not found. Install it, or make it available on PATH.")]
    NotFound {
        /// Program that could not be executed.
        program: String,
    },
    /// The program did not finish within its timeout and was killed.
    #[error("`{program}` did not finish within {seconds}s and was terminated")]
    Timeout {
        /// Program that timed out.
        program: String,
        /// Configured timeout.
        seconds: u64,
    },
    /// The program could not be started or waited on.
    #[error("failed to run `{program}`: {source}")]
    Io {
        /// Program that could not be executed.
        program: String,
        /// Underlying cause.
        source: std::io::Error,
    },
}

/// What to run, and under which limits.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    env: BTreeMap<OsString, OsString>,
    workdir: Option<PathBuf>,
    timeout: Duration,
    max_output_bytes: usize,
}

/// Default ceiling for captured output.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

impl CommandSpec {
    /// Start building a command.
    ///
    /// The environment starts out with only `PATH`, `HOME`, `LANG` and `TZ`,
    /// which are required for most tools to behave predictably and carry no
    /// secrets.
    #[must_use]
    pub fn new(program: impl Into<OsString>) -> Self {
        let mut env = BTreeMap::new();
        let path = std::env::var_os("PATH").unwrap_or_else(|| OsString::from(FALLBACK_PATH));
        env.insert(OsString::from("PATH"), path);
        if let Some(home) = std::env::var_os("HOME") {
            env.insert(OsString::from("HOME"), home);
        }
        env.insert(OsString::from("LANG"), OsString::from("C.UTF-8"));
        env.insert(OsString::from("TZ"), OsString::from("UTC"));

        Self {
            program: program.into(),
            args: Vec::new(),
            env,
            workdir: None,
            timeout: Duration::from_secs(60),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Append one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Append several arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set one environment variable for the child.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Replace the whole environment with exactly these variables.
    ///
    /// Used by check executors that must not even expose `PATH` inheritance.
    #[must_use]
    pub fn env_exact(mut self, env: BTreeMap<OsString, OsString>) -> Self {
        self.env = env;
        self
    }

    /// Run the child in this directory.
    #[must_use]
    pub fn workdir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.workdir = Some(dir.into());
        self
    }

    /// Kill the child after this duration.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Capture at most this many bytes from each stream.
    #[must_use]
    pub const fn max_output_bytes(mut self, max: usize) -> Self {
        self.max_output_bytes = max;
        self
    }

    /// Program name, for logs and errors.
    #[must_use]
    pub fn program_name(&self) -> String {
        Path::new(&self.program)
            .file_name()
            .unwrap_or_else(|| OsStr::new("?"))
            .to_string_lossy()
            .into_owned()
    }

    /// Arguments as lossy strings, for plans and logs.
    ///
    /// Never call this on a command that carries a secret in its arguments —
    /// `RestoreProof` passes secrets through the environment or a file instead,
    /// precisely so that this is safe.
    #[must_use]
    pub fn display_args(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// Run the program to completion.
    ///
    /// # Errors
    ///
    /// See [`ProcessError`]. A non-zero exit status is **not** an error: it is
    /// returned in [`CommandOutput::status`] so the caller decides what it means.
    pub async fn run(&self) -> Result<CommandOutput, ProcessError> {
        let program_name = self.program_name();
        let started = Instant::now();

        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Start from an empty environment, then add only what was declared.
        command.env_clear();
        for (key, value) in &self.env {
            command.env(key, value);
        }
        if let Some(dir) = &self.workdir {
            command.current_dir(dir);
        }

        let mut child = command.spawn().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProcessError::NotFound {
                    program: program_name.clone(),
                }
            } else {
                ProcessError::Io {
                    program: program_name.clone(),
                    source,
                }
            }
        })?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let cap = self.max_output_bytes;

        let collect = async {
            let (out, err) = tokio::join!(read_capped(stdout, cap), read_capped(stderr, cap));
            let status = child.wait().await;
            (out, err, status)
        };

        match tokio::time::timeout(self.timeout, collect).await {
            Ok(((stdout, stdout_truncated), (stderr, stderr_truncated), status)) => {
                let status = status.map_err(|source| ProcessError::Io {
                    program: program_name.clone(),
                    source,
                })?;
                Ok(CommandOutput {
                    program: program_name,
                    status: status.code(),
                    stdout,
                    stderr,
                    truncated: stdout_truncated || stderr_truncated,
                    duration: started.elapsed(),
                })
            }
            Err(_) => Err(ProcessError::Timeout {
                program: program_name,
                seconds: self.timeout.as_secs(),
            }),
        }
    }
}

/// Result of running an external program.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Program that produced this output.
    pub program: String,
    /// Exit code, or `None` when the process was terminated by a signal.
    pub status: Option<i32>,
    /// Captured standard output, lossily decoded and capped.
    pub stdout: String,
    /// Captured standard error, lossily decoded and capped.
    pub stderr: String,
    /// Whether output was discarded because it exceeded the cap.
    pub truncated: bool,
    /// Wall-clock duration.
    pub duration: Duration,
}

impl CommandOutput {
    /// Whether the program exited with status 0.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.status == Some(0)
    }

    /// Short description of how the program ended.
    #[must_use]
    pub fn describe_status(&self) -> String {
        match self.status {
            Some(code) => format!("exit code {code}"),
            None => "terminated by a signal".to_owned(),
        }
    }

    /// Last lines of standard error, for error messages.
    #[must_use]
    pub fn stderr_tail(&self, lines: usize) -> String {
        let collected: Vec<&str> = self.stderr.lines().rev().take(lines).collect();
        collected.into_iter().rev().collect::<Vec<_>>().join("\n")
    }
}

/// Read a stream to EOF, keeping at most `cap` bytes.
///
/// The stream is drained even past the cap so the child never blocks writing to
/// a full pipe, but the surplus is discarded instead of being buffered.
async fn read_capped<R>(reader: Option<R>, cap: usize) -> (String, bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(mut reader) = reader else {
        return (String::new(), false);
    };
    let mut kept: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;

    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let chunk = buffer.get(..read).unwrap_or_default();
                let remaining = cap.saturating_sub(kept.len());
                if remaining == 0 {
                    truncated = true;
                } else if chunk.len() > remaining {
                    kept.extend_from_slice(chunk.get(..remaining).unwrap_or_default());
                    truncated = true;
                } else {
                    kept.extend_from_slice(chunk);
                }
            }
        }
    }

    (String::from_utf8_lossy(&kept).into_owned(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_a_program_and_captures_output() {
        let output = CommandSpec::new("/bin/echo")
            .arg("hello")
            .run()
            .await
            .unwrap();
        assert!(output.is_success());
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn a_missing_program_is_reported_clearly() {
        let err = CommandSpec::new("definitely-not-a-real-program-xyz")
            .run()
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::NotFound { .. }));
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn shell_metacharacters_are_inert() {
        // If this were handed to a shell, it would create /tmp/pwned.
        let output = CommandSpec::new("/bin/echo")
            .arg("; touch /tmp/restoreproof-pwned")
            .run()
            .await
            .unwrap();
        assert!(output.stdout.contains("; touch"));
        assert!(!Path::new("/tmp/restoreproof-pwned").exists());
    }

    #[tokio::test]
    async fn the_parent_environment_is_not_inherited() {
        // SAFETY-equivalent note: set_var is process-global; this test uses a
        // name no other test reads.
        let output = CommandSpec::new("/usr/bin/env").run().await.unwrap();
        assert!(!output.stdout.contains("CARGO_PKG_NAME"));
        assert!(output.stdout.contains("PATH="));
    }

    #[tokio::test]
    async fn declared_environment_reaches_the_child() {
        let output = CommandSpec::new("/usr/bin/env")
            .env("RESTOREPROOF_TEST_VAR", "visible")
            .run()
            .await
            .unwrap();
        assert!(output.stdout.contains("RESTOREPROOF_TEST_VAR=visible"));
    }

    #[tokio::test]
    async fn stdin_is_closed_so_prompts_cannot_hang() {
        let output = CommandSpec::new("/bin/cat")
            .timeout(Duration::from_secs(5))
            .run()
            .await
            .unwrap();
        assert!(output.is_success());
        assert!(output.stdout.is_empty());
    }

    #[tokio::test]
    async fn a_hanging_program_is_killed() {
        let err = CommandSpec::new("/bin/sleep")
            .arg("30")
            .timeout(Duration::from_millis(200))
            .run()
            .await
            .unwrap_err();
        assert!(matches!(err, ProcessError::Timeout { .. }));
    }

    #[tokio::test]
    async fn output_is_capped() {
        let output = CommandSpec::new("/bin/sh")
            .args(["-c", "head -c 100000 /dev/zero | tr '\\0' 'a'"])
            .max_output_bytes(1024)
            .timeout(Duration::from_secs(10))
            .run()
            .await
            .unwrap();
        assert!(output.truncated);
        assert!(output.stdout.len() <= 1024);
    }

    #[tokio::test]
    async fn non_zero_exit_codes_are_data_not_errors() {
        let output = CommandSpec::new("/bin/sh")
            .args(["-c", "exit 3"])
            .run()
            .await
            .unwrap();
        assert_eq!(output.status, Some(3));
        assert!(!output.is_success());
        assert_eq!(output.describe_status(), "exit code 3");
    }

    #[tokio::test]
    async fn workdir_is_honoured() {
        let output = CommandSpec::new("/bin/pwd")
            .workdir("/tmp")
            .run()
            .await
            .unwrap();
        assert!(output.stdout.trim().ends_with("tmp"));
    }

    #[tokio::test]
    async fn stderr_tail_returns_the_last_lines() {
        let output = CommandSpec::new("/bin/sh")
            .args(["-c", "printf 'a\\nb\\nc\\n' >&2"])
            .run()
            .await
            .unwrap();
        assert_eq!(output.stderr_tail(2), "b\nc");
    }
}
