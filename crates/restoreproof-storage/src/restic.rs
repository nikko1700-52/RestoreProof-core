//! restic repositories.
//!
//! The repository is only ever read: `snapshots` and `restore`. `RestoreProof`
//! never calls `forget`, `prune` or anything else that could alter a backup it
//! is supposed to verify.
//!
//! The password never appears on the command line, where it would be visible in
//! the host's process list. It is passed through `RESTIC_PASSWORD_FILE` when the
//! configuration points at a file (the plaintext then never enters this
//! process), or through `RESTIC_PASSWORD` when it comes from an environment
//! variable.

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use restoreproof_config::RepositoryLocation;
use restoreproof_core::CommandSpec;
use serde::Deserialize;

use crate::error::{Result, StorageError};
use crate::metadata::{BackupMetadata, RestoreOutcome};
use crate::source::{BackupSource, ResolvedSecret, StorageLimits, directory_size};

const TOOL: &str = "restic";
const SOURCE_TYPE: &str = "restic";

/// One entry of `restic snapshots --json`.
#[derive(Debug, Deserialize)]
struct ResticSnapshot {
    id: String,
    #[serde(default)]
    short_id: Option<String>,
    time: DateTime<Utc>,
}

/// A restic repository.
#[derive(Debug, Clone)]
pub struct ResticSource {
    repository: RepositoryLocation,
    secret: ResolvedSecret,
    snapshot: String,
    include_path: Option<String>,
    limits: StorageLimits,
}

impl ResticSource {
    /// Build a source from validated configuration.
    #[must_use]
    pub const fn new(
        repository: RepositoryLocation,
        secret: ResolvedSecret,
        snapshot: String,
        include_path: Option<String>,
        limits: StorageLimits,
    ) -> Self {
        Self {
            repository,
            secret,
            snapshot,
            include_path,
            limits,
        }
    }

    /// Base command with the repository and credentials wired in.
    fn command(&self, timeout: std::time::Duration) -> CommandSpec {
        let mut spec = CommandSpec::new(TOOL)
            .arg("--repo")
            .arg(self.repository.as_argument())
            .timeout(timeout)
            .max_output_bytes(self.limits.max_output_bytes);

        match &self.secret {
            ResolvedSecret::File(path) => {
                spec = spec.env("RESTIC_PASSWORD_FILE", path.as_os_str());
            }
            ResolvedSecret::Value(secret) => {
                spec = spec.env("RESTIC_PASSWORD", secret.expose_secret());
            }
            ResolvedSecret::None => {
                // No password configured: forward the two standard variables if
                // the operator exported them. `RESTIC_PASSWORD_COMMAND` is
                // deliberately *not* forwarded, because it would make a drill
                // execute a command that RestoreProof never audited.
                for key in ["RESTIC_PASSWORD", "RESTIC_PASSWORD_FILE"] {
                    if let Some(value) = std::env::var_os(key) {
                        spec = spec.env(key, value);
                    }
                }
            }
        }
        spec
    }

    /// Resolve the configured selector to a concrete snapshot.
    async fn resolve_snapshot(&self) -> Result<ResticSnapshot> {
        let mut spec = self.command(self.limits.inspect_timeout).arg("snapshots").arg("--json");
        if self.snapshot == "latest" {
            spec = spec.args(["--latest", "1"]);
        } else {
            spec = spec.arg(&self.snapshot);
        }

        let output = spec
            .run()
            .await
            .map_err(|err| StorageError::from_process(err, SOURCE_TYPE, TOOL))?;

        if !output.is_success() {
            return Err(StorageError::ToolFailed {
                tool: TOOL.to_owned(),
                status: output.describe_status(),
                details: output.stderr_tail(10),
            });
        }

        let snapshots: Vec<ResticSnapshot> =
            serde_json::from_str(output.stdout.trim()).map_err(|err| StorageError::UnexpectedOutput {
                tool: TOOL.to_owned(),
                details: format!("expected a JSON array of snapshots: {err}"),
            })?;

        snapshots.into_iter().next_back().ok_or_else(|| {
            StorageError::InvalidBackup(format!(
                "the repository contains no snapshot matching `{}`",
                self.snapshot
            ))
        })
    }
}

#[async_trait]
impl BackupSource for ResticSource {
    fn source_type(&self) -> &'static str {
        SOURCE_TYPE
    }

    fn location(&self) -> String {
        self.repository.as_argument()
    }

    fn required_tool(&self) -> Option<&'static str> {
        Some(TOOL)
    }

    async fn inspect(&self) -> Result<BackupMetadata> {
        let snapshot = self.resolve_snapshot().await?;
        Ok(BackupMetadata {
            source_type: SOURCE_TYPE.to_owned(),
            location: self.location(),
            snapshot_id: Some(snapshot.short_id.unwrap_or(snapshot.id)),
            created_at: Some(snapshot.time),
            size_bytes: None,
            notes: vec![
                "restic does not report a restored size before the restore; the size in this \
                 report is measured on the restored tree"
                    .to_owned(),
            ],
        })
    }

    async fn restore(&self, destination: &Path) -> Result<RestoreOutcome> {
        // Resolve the selector first so that the snapshot named in the report is
        // exactly the one that was restored.
        let snapshot = self.resolve_snapshot().await?;
        let started = Instant::now();

        let mut spec = self
            .command(self.limits.restore_timeout)
            .arg("restore")
            .arg(&snapshot.id)
            .arg("--target")
            .arg(destination.as_os_str());
        if let Some(include) = &self.include_path {
            spec = spec.arg("--include").arg(include);
        }

        let output = spec
            .run()
            .await
            .map_err(|err| StorageError::from_process(err, SOURCE_TYPE, TOOL))?;

        if !output.is_success() {
            return Err(StorageError::ToolFailed {
                tool: TOOL.to_owned(),
                status: output.describe_status(),
                details: output.stderr_tail(10),
            });
        }

        let bytes = directory_size(destination).ok();
        let mut log: Vec<String> = output.stdout.lines().rev().take(5).map(str::to_owned).collect();
        log.reverse();

        Ok(RestoreOutcome {
            destination: destination.to_path_buf(),
            bytes_restored: bytes,
            duration: started.elapsed(),
            log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn source(secret: ResolvedSecret) -> ResticSource {
        ResticSource::new(
            RepositoryLocation::Local(PathBuf::from("/srv/repo")),
            secret,
            "latest".to_owned(),
            None,
            StorageLimits::default(),
        )
    }

    #[test]
    fn the_password_is_never_placed_on_the_command_line() {
        let secret = ResolvedSecret::Value(restoreproof_core::Secret::new("hunter2-secret"));
        let spec = source(secret).command(std::time::Duration::from_secs(5));
        let rendered = spec.display_args().join(" ");
        assert!(!rendered.contains("hunter2"), "secret leaked into argv: {rendered}");
    }

    #[test]
    fn the_repository_is_passed_as_a_separate_argument() {
        let spec = source(ResolvedSecret::None).command(std::time::Duration::from_secs(5));
        assert_eq!(spec.display_args(), vec!["--repo", "/srv/repo"]);
    }

    #[test]
    fn snapshot_json_is_parsed() {
        let json = r#"[{"time":"2026-01-15T08:00:00.123456Z","id":"aabbccddeeff","short_id":"aabbccdd"}]"#;
        let snapshots: Vec<ResticSnapshot> = serde_json::from_str(json).unwrap();
        let snapshot = snapshots.into_iter().next().unwrap();
        assert_eq!(snapshot.short_id.as_deref(), Some("aabbccdd"));
        assert_eq!(snapshot.time.to_rfc3339(), "2026-01-15T08:00:00.123456+00:00");
    }

    #[tokio::test]
    async fn a_missing_restic_binary_maps_to_a_dependency_error() {
        // `restic` is not installed in the test environment; if it ever is,
        // the call still fails because the repository does not exist.
        let source = ResticSource::new(
            RepositoryLocation::Local(PathBuf::from("/nonexistent/repo")),
            ResolvedSecret::None,
            "latest".to_owned(),
            None,
            StorageLimits::default(),
        );
        let err = source.inspect().await.unwrap_err();
        assert!(
            matches!(
                err,
                StorageError::ToolMissing { .. } | StorageError::ToolFailed { .. }
            ),
            "unexpected error: {err}"
        );
    }
}
