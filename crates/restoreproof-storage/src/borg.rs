//! `BorgBackup` repositories.
//!
//! Support is intentionally minimal and marked experimental: `borg list` to
//! find the archive and `borg extract` to restore it. As with restic, the
//! repository is only ever read.
//!
//! Two caveats are surfaced rather than hidden:
//!
//! * `borg list --json` reports archive timestamps in **local time without an
//!   offset**. They are interpreted in the local timezone of the machine
//!   running the drill, and the resulting metadata carries a note saying so.
//! * `borg extract` writes relative to the current directory, so the child is
//!   started with its working directory set to the restore target.

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use restoreproof_config::RepositoryLocation;
use restoreproof_core::CommandSpec;
use serde::Deserialize;

use crate::error::{Result, StorageError};
use crate::metadata::{BackupMetadata, RestoreOutcome};
use crate::source::{BackupSource, ResolvedSecret, StorageLimits, directory_size};

const TOOL: &str = "borg";
const SOURCE_TYPE: &str = "borg";

#[derive(Debug, Deserialize)]
struct BorgArchiveList {
    #[serde(default)]
    archives: Vec<BorgArchive>,
}

#[derive(Debug, Deserialize)]
struct BorgArchive {
    name: String,
    #[serde(default)]
    id: Option<String>,
    /// Local time without an offset, as produced by borg.
    #[serde(default)]
    time: Option<String>,
}

/// A `BorgBackup` repository.
#[derive(Debug, Clone)]
pub struct BorgSource {
    repository: RepositoryLocation,
    secret: ResolvedSecret,
    archive: String,
    limits: StorageLimits,
}

impl BorgSource {
    /// Build a source from validated configuration.
    #[must_use]
    pub const fn new(
        repository: RepositoryLocation,
        secret: ResolvedSecret,
        archive: String,
        limits: StorageLimits,
    ) -> Self {
        Self {
            repository,
            secret,
            archive,
            limits,
        }
    }

    fn command(&self, timeout: std::time::Duration) -> CommandSpec {
        let mut spec = CommandSpec::new(TOOL)
            .timeout(timeout)
            .max_output_bytes(self.limits.max_output_bytes)
            // Never accept a repository that moved or whose key changed without
            // the operator noticing, and never run an interactive prompt.
            .env("BORG_RELOCATED_REPO_ACCESS_IS_OK", "no")
            .env("BORG_CHECK_I_KNOW_WHAT_I_AM_DOING", "NO")
            .env("BORG_DELETE_I_KNOW_WHAT_I_AM_DOING", "NO");

        match &self.secret {
            ResolvedSecret::File(path) => {
                spec = spec.env("BORG_PASSPHRASE_FD", "-").env("BORG_PASSCOMMAND", String::new());
                // borg has no passphrase-file variable; read it and pass the
                // value, which stays out of argv.
                if let Ok(value) = std::fs::read_to_string(path) {
                    spec = spec.env("BORG_PASSPHRASE", value.trim_end_matches(['\n', '\r']));
                }
            }
            ResolvedSecret::Value(secret) => {
                spec = spec.env("BORG_PASSPHRASE", secret.expose_secret());
            }
            ResolvedSecret::None => {
                if let Some(value) = std::env::var_os("BORG_PASSPHRASE") {
                    spec = spec.env("BORG_PASSPHRASE", value);
                }
                // An unencrypted repository would otherwise prompt; with stdin
                // closed that is a hang, so acknowledge it explicitly.
                spec = spec.env("BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK", "yes");
            }
        }
        spec
    }

    async fn resolve_archive(&self) -> Result<BorgArchive> {
        let output = self
            .command(self.limits.inspect_timeout)
            .arg("list")
            .arg("--json")
            .arg(self.repository.as_argument())
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

        let list: BorgArchiveList = serde_json::from_str(output.stdout.trim()).map_err(|err| {
            StorageError::UnexpectedOutput {
                tool: TOOL.to_owned(),
                details: format!("expected the JSON output of `borg list --json`: {err}"),
            }
        })?;

        if self.archive == "latest" {
            list.archives.into_iter().next_back().ok_or_else(|| {
                StorageError::InvalidBackup("the repository contains no archive".to_owned())
            })
        } else {
            list.archives
                .into_iter()
                .find(|archive| archive.name == self.archive)
                .ok_or_else(|| {
                    StorageError::InvalidBackup(format!(
                        "the repository contains no archive named `{}`",
                        self.archive
                    ))
                })
        }
    }
}

/// Interpret a borg timestamp (local time, no offset) as UTC.
fn parse_borg_time(raw: &str) -> Option<DateTime<Utc>> {
    let naive = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|local| local.with_timezone(&Utc))
}

#[async_trait]
impl BackupSource for BorgSource {
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
        let archive = self.resolve_archive().await?;
        let created_at = archive.time.as_deref().and_then(parse_borg_time);
        let mut notes = vec![
            "BorgBackup support is experimental in this release".to_owned(),
        ];
        if created_at.is_some() {
            notes.push(
                "borg reports archive timestamps without a UTC offset; the timestamp was \
                 interpreted in the local timezone of this machine"
                    .to_owned(),
            );
        }

        Ok(BackupMetadata {
            source_type: SOURCE_TYPE.to_owned(),
            location: self.location(),
            snapshot_id: Some(archive.id.unwrap_or(archive.name)),
            created_at,
            size_bytes: None,
            notes,
        })
    }

    async fn restore(&self, destination: &Path) -> Result<RestoreOutcome> {
        let archive = self.resolve_archive().await?;
        let started = Instant::now();

        let output = self
            .command(self.limits.restore_timeout)
            .arg("extract")
            .arg(format!("{}::{}", self.repository.as_argument(), archive.name))
            // borg extract writes relative to the working directory.
            .workdir(destination)
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

        Ok(RestoreOutcome {
            destination: destination.to_path_buf(),
            bytes_restored: directory_size(destination).ok(),
            duration: started.elapsed(),
            log: vec![format!("extracted archive `{}`", archive.name)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn archive_json_is_parsed() {
        let json = r#"{"archives":[{"name":"nightly-2026-01-14","id":"aa","time":"2026-01-14T02:00:00.000000"},{"name":"nightly-2026-01-15","id":"bb","time":"2026-01-15T02:00:00.000000"}]}"#;
        let list: BorgArchiveList = serde_json::from_str(json).unwrap();
        assert_eq!(list.archives.len(), 2);
        assert_eq!(list.archives[1].name, "nightly-2026-01-15");
    }

    #[test]
    fn borg_timestamps_are_parsed() {
        assert!(parse_borg_time("2026-01-15T02:00:00.000000").is_some());
        assert!(parse_borg_time("2026-01-15T02:00:00").is_some());
        assert!(parse_borg_time("not a timestamp").is_none());
    }

    #[test]
    fn the_passphrase_never_reaches_the_command_line() {
        let source = BorgSource::new(
            RepositoryLocation::Local(PathBuf::from("/srv/borg")),
            ResolvedSecret::Value(restoreproof_core::Secret::new("passphrase-secret")),
            "latest".to_owned(),
            StorageLimits::default(),
        );
        let rendered = source
            .command(std::time::Duration::from_secs(5))
            .display_args()
            .join(" ");
        assert!(!rendered.contains("passphrase-secret"));
    }
}
