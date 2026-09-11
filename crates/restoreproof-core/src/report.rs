//! The recovery-drill report document.
//!
//! The report is the deliverable of `RestoreProof`: it is what an operator
//! attaches to a ticket, sends to an auditor or diffs between two drills. It
//! must therefore be (a) free of secrets, (b) stable enough to diff, and (c)
//! tamper-evident.
//!
//! Integrity is provided by a SHA-256 over the canonical JSON encoding of the
//! document with `integrity.value` set to the empty string. [`Report::seal`]
//! computes it, [`Report::verify_integrity`] recomputes and compares. The hash
//! is **not** a signature: it detects accidental modification and transcription
//! errors, not a determined forger. Signed reports are out of scope for the
//! open-source edition, and this limitation is stated in the documentation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::metrics::{RpoMetric, RtoMetric};
use crate::redact::Redactor;
use crate::status::{CheckStatus, RunStatus};

/// Current schema version of the JSON report.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// Maximum number of bytes kept for a single captured output block.
pub const MAX_DETAIL_BYTES: usize = 8 * 1024;

/// A full recovery-drill report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Version of this document's schema.
    pub schema_version: u32,
    /// Which build produced the report.
    pub tool: ToolInfo,
    /// Identity and outcome of the drill.
    pub run: RunInfo,
    /// Which backup was restored.
    pub backup: BackupInfo,
    /// Timeline of the recovery procedure.
    pub recovery: RecoveryTimeline,
    /// Measured objectives.
    pub objectives: Objectives,
    /// Counts per status.
    pub summary: CheckSummary,
    /// Detailed result of every executed check.
    pub checks: Vec<CheckOutcome>,
    /// Fatal problems encountered during the drill.
    pub errors: Vec<String>,
    /// Non-fatal problems worth reading.
    pub warnings: Vec<String>,
    /// Provenance of the configuration used.
    pub config: ConfigInfo,
    /// Tamper-evidence for the document above.
    pub integrity: Integrity,
}

/// Which build produced the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Crate version of the CLI.
    pub version: String,
    /// Git commit, when the binary was built from a checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    /// `os/arch` of the machine that ran the drill.
    pub platform: String,
}

impl ToolInfo {
    /// Build the tool information block for the running binary.
    #[must_use]
    pub fn detect(version: &str, git_commit: Option<&str>) -> Self {
        Self {
            version: version.to_owned(),
            git_commit: git_commit.map(str::to_owned),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        }
    }
}

/// Identity and outcome of one drill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunInfo {
    /// Unique identifier of this run.
    pub id: String,
    /// Project name from the configuration.
    pub project: String,
    /// Optional project description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// When the drill started (UTC).
    pub started_at: DateTime<Utc>,
    /// When the drill finished (UTC).
    pub finished_at: DateTime<Utc>,
    /// Total wall-clock duration.
    pub duration_seconds: f64,
    /// Aggregated status.
    pub status: RunStatus,
}

/// Which backup was restored, with every credential removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupInfo {
    /// Backup source type (`local`, `restic`, `borg`).
    pub source_type: String,
    /// Location of the repository, redacted.
    pub location: String,
    /// Snapshot or archive identifier that was restored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    /// Creation timestamp of the snapshot, when the source exposes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Size of the restored data, in bytes, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Timeline of the recovery procedure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryTimeline {
    /// When the restore step started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_started_at: Option<DateTime<Utc>>,
    /// When the restore step finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_finished_at: Option<DateTime<Utc>>,
    /// Duration of the restore step.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_duration_seconds: Option<f64>,
    /// When the recovery environment was reported as started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment_started_at: Option<DateTime<Utc>>,
    /// When every required check had passed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<DateTime<Utc>>,
    /// Whether the temporary environment was destroyed.
    pub environment_cleaned_up: bool,
}

/// Measured objectives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objectives {
    /// Measured recovery time.
    pub rto: RtoMetric,
    /// Estimated recovery point.
    pub rpo: RpoMetric,
}

/// Counts per status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSummary {
    /// Total number of checks recorded.
    pub total: usize,
    /// Checks that passed.
    pub passed: usize,
    /// Checks that failed.
    pub failed: usize,
    /// Checks that could not be executed.
    pub error: usize,
    /// Checks that were not executed.
    pub skipped: usize,
    /// Of the totals above, how many were required.
    pub required: usize,
}

impl CheckSummary {
    /// Count a slice of outcomes.
    #[must_use]
    pub fn from_outcomes(outcomes: &[CheckOutcome]) -> Self {
        let mut summary = Self {
            total: outcomes.len(),
            ..Self::default()
        };
        for outcome in outcomes {
            if outcome.required {
                summary.required += 1;
            }
            match outcome.status {
                CheckStatus::Passed => summary.passed += 1,
                CheckStatus::Failed => summary.failed += 1,
                CheckStatus::Error => summary.error += 1,
                CheckStatus::Skipped => summary.skipped += 1,
            }
        }
        summary
    }
}

/// Result of a single check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckOutcome {
    /// Stable identifier from `checks.yaml`.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description from the configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Check type (`http`, `command`, `sql`, `file`, `container`, `script`).
    pub kind: String,
    /// Whether the drill fails when this check does not pass.
    pub required: bool,
    /// Outcome.
    pub status: CheckStatus,
    /// When the check started.
    pub started_at: DateTime<Utc>,
    /// How long the check took.
    pub duration_seconds: f64,
    /// One-line explanation of the outcome.
    pub message: String,
    /// Captured output, redacted and truncated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// Number of attempts performed (retries included).
    pub attempts: u32,
}

/// Provenance of the configuration used for the drill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigInfo {
    /// Path of the configuration file, as provided by the user.
    pub path: String,
    /// `version:` field of the configuration document.
    pub version: u32,
    /// Application version under test, when the user supplied one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_version: Option<String>,
    /// SHA-256 of the configuration file, so a report can be tied to an exact input.
    pub config_sha256: String,
}

/// Tamper-evidence for the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Integrity {
    /// Always `sha256` in schema version 1.
    pub algorithm: String,
    /// Hex-encoded digest, empty until [`Report::seal`] is called.
    pub value: String,
}

impl Default for Integrity {
    fn default() -> Self {
        Self {
            algorithm: "sha256".to_owned(),
            value: String::new(),
        }
    }
}

impl Report {
    /// Compute the canonical digest of this document.
    ///
    /// The digest covers the JSON encoding of the report with `integrity.value`
    /// emptied, so that it is reproducible from the published document.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the document cannot be encoded.
    pub fn digest(&self) -> Result<String, CoreError> {
        use sha2::{Digest, Sha256};

        let mut canonical = self.clone();
        canonical.integrity.value = String::new();
        let bytes = serde_json::to_vec(&canonical)?;
        Ok(hex_encode(&Sha256::digest(bytes)))
    }

    /// Fill in `integrity.value`.
    ///
    /// Call this **after** redaction, so the digest covers the published text.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the document cannot be encoded.
    pub fn seal(&mut self) -> Result<(), CoreError> {
        self.integrity.algorithm = "sha256".to_owned();
        self.integrity.value = self.digest()?;
        Ok(())
    }

    /// Recompute the digest and compare it with the recorded one.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the document cannot be encoded.
    pub fn verify_integrity(&self) -> Result<bool, CoreError> {
        Ok(!self.integrity.value.is_empty() && self.digest()? == self.integrity.value)
    }

    /// Apply redaction and truncation to every free-form string in the report.
    ///
    /// This is the last line of defence: individual components already redact
    /// their own output, but a report is never written without going through
    /// this method.
    pub fn sanitize(&mut self, redactor: &Redactor) {
        self.backup.location = redactor.redact(&self.backup.location);
        for error in &mut self.errors {
            *error = redactor.redact_truncated(error, MAX_DETAIL_BYTES);
        }
        for warning in &mut self.warnings {
            *warning = redactor.redact_truncated(warning, MAX_DETAIL_BYTES);
        }
        for check in &mut self.checks {
            check.message = redactor.redact_truncated(&check.message, MAX_DETAIL_BYTES);
            for detail in &mut check.details {
                *detail = redactor.redact_truncated(detail, MAX_DETAIL_BYTES);
            }
        }
        if let Some(note) = &self.objectives.rto.note {
            self.objectives.rto.note = Some(redactor.redact(note));
        }
        if let Some(note) = &self.objectives.rpo.note {
            self.objectives.rpo.note = Some(redactor.redact(note));
        }
    }

    /// Redact, recount the summary and seal in one call.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the document cannot be encoded.
    pub fn finalize(&mut self, redactor: &Redactor) -> Result<(), CoreError> {
        self.sanitize(redactor);
        self.summary = CheckSummary::from_outcomes(&self.checks);
        self.seal()
    }
}

/// Hex-encode a byte slice, lower case.
#[must_use]
pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut acc, byte| {
            // Writing to a String is infallible.
            let _ = write!(acc, "{byte:02x}");
            acc
        })
}

/// SHA-256 of an arbitrary byte slice, hex encoded.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex_encode(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::ObjectiveOutcome;
    use chrono::TimeZone;

    fn sample_report() -> Report {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();
        Report {
            schema_version: REPORT_SCHEMA_VERSION,
            tool: ToolInfo::detect("0.1.0", Some("abcdef1")),
            run: RunInfo {
                id: "run-1".to_owned(),
                project: "example-app".to_owned(),
                description: None,
                started_at: t0,
                finished_at: t0,
                duration_seconds: 12.5,
                status: RunStatus::Passed,
            },
            backup: BackupInfo {
                source_type: "restic".to_owned(),
                location: "s3://bucket/repo".to_owned(),
                snapshot_id: Some("deadbeef".to_owned()),
                created_at: Some(t0),
                size_bytes: Some(1024),
            },
            recovery: RecoveryTimeline {
                restore_started_at: Some(t0),
                restore_finished_at: Some(t0),
                restore_duration_seconds: Some(1.0),
                environment_started_at: Some(t0),
                ready_at: Some(t0),
                environment_cleaned_up: true,
            },
            objectives: Objectives {
                rto: RtoMetric::evaluate(Some(std::time::Duration::from_secs(12)), Some(1800)),
                rpo: RpoMetric::evaluate(Some(t0), t0, Some(14_400)),
            },
            summary: CheckSummary::default(),
            checks: vec![CheckOutcome {
                id: "app-health".to_owned(),
                name: "health endpoint".to_owned(),
                description: None,
                kind: "http".to_owned(),
                required: true,
                status: CheckStatus::Passed,
                started_at: t0,
                duration_seconds: 0.1,
                message: "HTTP 200".to_owned(),
                details: vec![],
                attempts: 1,
            }],
            errors: vec![],
            warnings: vec![],
            config: ConfigInfo {
                path: "restoreproof.yaml".to_owned(),
                version: 1,
                application_version: None,
                config_sha256: sha256_hex(b"version: 1"),
            },
            integrity: Integrity::default(),
        }
    }

    #[test]
    fn sealing_then_verifying_succeeds() {
        let mut report = sample_report();
        report.seal().unwrap();
        assert_eq!(report.integrity.value.len(), 64);
        assert!(report.verify_integrity().unwrap());
    }

    #[test]
    fn an_unsealed_report_does_not_verify() {
        let report = sample_report();
        assert!(!report.verify_integrity().unwrap());
    }

    #[test]
    fn tampering_is_detected() {
        let mut report = sample_report();
        report.seal().unwrap();
        report.run.status = RunStatus::Failed;
        assert!(!report.verify_integrity().unwrap());
    }

    #[test]
    fn the_digest_is_deterministic() {
        let mut a = sample_report();
        let mut b = sample_report();
        a.seal().unwrap();
        b.seal().unwrap();
        assert_eq!(a.integrity.value, b.integrity.value);
    }

    #[test]
    fn finalize_redacts_before_hashing() {
        let mut report = sample_report();
        report.backup.location = "restic:sftp://user:TopSecretPass@host/repo".to_owned();
        report.checks.first_mut().unwrap().message = "failed with TopSecretPass".to_owned();

        let mut redactor = Redactor::new();
        redactor.add_secret("TopSecretPass");
        report.finalize(&redactor).unwrap();

        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("TopSecretPass"), "secret leaked into report");
        assert!(report.verify_integrity().unwrap());
    }

    #[test]
    fn finalize_recounts_the_summary() {
        let mut report = sample_report();
        report.finalize(&Redactor::new()).unwrap();
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.summary.passed, 1);
        assert_eq!(report.summary.required, 1);
    }

    #[test]
    fn report_round_trips_through_json() {
        let mut report = sample_report();
        report.finalize(&Redactor::new()).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let parsed: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, report);
        assert!(parsed.verify_integrity().unwrap());
    }

    #[test]
    fn summary_counts_every_status() {
        let t0 = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();
        let make = |status, required| CheckOutcome {
            id: "c".to_owned(),
            name: "c".to_owned(),
            description: None,
            kind: "http".to_owned(),
            required,
            status,
            started_at: t0,
            duration_seconds: 0.0,
            message: String::new(),
            details: vec![],
            attempts: 1,
        };
        let outcomes = vec![
            make(CheckStatus::Passed, true),
            make(CheckStatus::Failed, true),
            make(CheckStatus::Error, false),
            make(CheckStatus::Skipped, false),
        ];
        let summary = CheckSummary::from_outcomes(&outcomes);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.error, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.required, 2);
    }

    #[test]
    fn sha256_hex_matches_the_reference_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn objective_outcomes_survive_serialization() {
        let metric = RtoMetric::evaluate(Some(std::time::Duration::from_secs(1)), Some(10));
        let json = serde_json::to_string(&metric).unwrap();
        assert!(json.contains("\"PASS\""));
        let parsed: RtoMetric = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.outcome, ObjectiveOutcome::Pass);
    }
}
