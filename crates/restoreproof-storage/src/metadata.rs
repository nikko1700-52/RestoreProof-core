//! What a backup source reports about a backup.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Facts about the backup that will be restored.
///
/// Every field except `source_type` and `location` is optional: a source that
/// cannot determine a value says so, and the report shows `UNKNOWN`. Nothing
/// here is ever guessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// `local`, `restic` or `borg`.
    pub source_type: String,
    /// Repository or directory, with credentials already removed.
    pub location: String,
    /// Snapshot or archive identifier.
    pub snapshot_id: Option<String>,
    /// When the snapshot was created.
    pub created_at: Option<DateTime<Utc>>,
    /// Size of the snapshot, when the source can report it.
    pub size_bytes: Option<u64>,
    /// Caveats worth surfacing, for instance an approximated timestamp.
    pub notes: Vec<String>,
}

/// Outcome of a restore.
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    /// Directory the backup was restored into.
    pub destination: PathBuf,
    /// Number of bytes present in the destination afterwards.
    pub bytes_restored: Option<u64>,
    /// How long the restore took.
    pub duration: Duration,
    /// Redacted, truncated log lines worth keeping in the report.
    pub log: Vec<String>,
}
