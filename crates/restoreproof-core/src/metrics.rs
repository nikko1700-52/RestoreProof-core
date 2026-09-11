//! Measured RTO and estimated RPO.
//!
//! Definitions used by `RestoreProof`, and kept identical everywhere:
//!
//! * **Measured RTO** — wall-clock duration between the start of the recovery
//!   procedure and the moment every *required* check passed.
//! * **Estimated RPO** — difference between the reference timestamp (by default
//!   the moment the drill started) and the creation timestamp of the backup
//!   that was restored.
//!
//! When the backup source does not expose a creation timestamp, the RPO is
//! reported as `UNKNOWN`. It is never guessed, never defaulted to zero and
//! never silently omitted.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Result of comparing a measured value against its objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObjectiveOutcome {
    /// Measured value is within the objective.
    Pass,
    /// Measured value exceeds the objective.
    Fail,
    /// The measurement or the objective is missing.
    Unknown,
}

/// Round to millisecond precision so that reports are stable and diffable.
fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Recovery Time Objective: measured against `target_seconds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RtoMetric {
    /// Measured recovery time, in seconds. `None` when the drill never reached
    /// a state where all required checks passed.
    pub measured_seconds: Option<f64>,
    /// Objective declared in the configuration.
    pub target_seconds: Option<u64>,
    /// Comparison outcome.
    pub outcome: ObjectiveOutcome,
    /// Human-readable explanation when the outcome is `UNKNOWN`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl RtoMetric {
    /// Compare a measured recovery duration against the configured objective.
    #[must_use]
    pub fn evaluate(measured: Option<Duration>, target_seconds: Option<u64>) -> Self {
        let measured_seconds = measured.map(|d| round_ms(d.as_secs_f64()));
        let (outcome, note) = match (measured_seconds, target_seconds) {
            (Some(measured), Some(target)) => {
                #[allow(clippy::cast_precision_loss)]
                let target_f = target as f64;
                if measured <= target_f {
                    (ObjectiveOutcome::Pass, None)
                } else {
                    (ObjectiveOutcome::Fail, None)
                }
            }
            (Some(_), None) => (
                ObjectiveOutcome::Unknown,
                Some("RTO_UNKNOWN: no target_rto_seconds configured".to_owned()),
            ),
            (None, _) => (
                ObjectiveOutcome::Unknown,
                Some(
                    "RTO_UNKNOWN: the drill never reached a state where all required checks passed"
                        .to_owned(),
                ),
            ),
        };
        Self {
            measured_seconds,
            target_seconds,
            outcome,
            note,
        }
    }
}

/// Recovery Point Objective: estimated from backup metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpoMetric {
    /// Creation timestamp of the restored backup, when known.
    pub backup_timestamp: Option<DateTime<Utc>>,
    /// Instant the estimation is relative to (drill start by default).
    pub reference_timestamp: DateTime<Utc>,
    /// Estimated data loss window, in seconds.
    pub estimated_seconds: Option<i64>,
    /// Objective declared in the configuration.
    pub target_seconds: Option<u64>,
    /// Comparison outcome.
    pub outcome: ObjectiveOutcome,
    /// Human-readable explanation when the outcome is `UNKNOWN` or suspicious.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl RpoMetric {
    /// Estimate the RPO and compare it against the configured objective.
    ///
    /// A backup timestamp in the future relative to the reference is clamped to
    /// zero and flagged, because it usually means the two clocks disagree.
    #[must_use]
    pub fn evaluate(
        backup_timestamp: Option<DateTime<Utc>>,
        reference_timestamp: DateTime<Utc>,
        target_seconds: Option<u64>,
    ) -> Self {
        let Some(backup_timestamp) = backup_timestamp else {
            return Self {
                backup_timestamp: None,
                reference_timestamp,
                estimated_seconds: None,
                target_seconds,
                outcome: ObjectiveOutcome::Unknown,
                note: Some(
                    "RPO_UNKNOWN: the backup source did not expose a creation timestamp".to_owned(),
                ),
            };
        };

        let raw = (reference_timestamp - backup_timestamp).num_seconds();
        let mut note = None;
        let estimated = if raw < 0 {
            note = Some(
                "the backup timestamp is ahead of the reference timestamp; clock skew is likely, \
                 the RPO was clamped to 0"
                    .to_owned(),
            );
            0
        } else {
            raw
        };

        let outcome = match target_seconds {
            Some(target) => {
                let within = u64::try_from(estimated).is_ok_and(|value| value <= target);
                if within {
                    ObjectiveOutcome::Pass
                } else {
                    ObjectiveOutcome::Fail
                }
            }
            None => {
                note.get_or_insert_with(|| {
                    "RPO_UNKNOWN: no target_rpo_seconds configured".to_owned()
                });
                ObjectiveOutcome::Unknown
            }
        };

        Self {
            backup_timestamp: Some(backup_timestamp),
            reference_timestamp,
            estimated_seconds: Some(estimated),
            target_seconds,
            outcome,
            note,
        }
    }

    /// Age of the backup in seconds, i.e. the estimated RPO.
    #[must_use]
    pub const fn backup_age_seconds(&self) -> Option<i64> {
        self.estimated_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 15, hour, minute, 0).unwrap()
    }

    #[test]
    fn rto_within_target_passes() {
        let metric = RtoMetric::evaluate(Some(Duration::from_secs(600)), Some(1800));
        assert_eq!(metric.outcome, ObjectiveOutcome::Pass);
        assert_eq!(metric.measured_seconds, Some(600.0));
    }

    #[test]
    fn rto_exactly_on_target_passes() {
        let metric = RtoMetric::evaluate(Some(Duration::from_secs(1800)), Some(1800));
        assert_eq!(metric.outcome, ObjectiveOutcome::Pass);
    }

    #[test]
    fn rto_above_target_fails() {
        let metric = RtoMetric::evaluate(Some(Duration::from_secs(1801)), Some(1800));
        assert_eq!(metric.outcome, ObjectiveOutcome::Fail);
    }

    #[test]
    fn rto_without_measurement_is_unknown_and_explains_why() {
        let metric = RtoMetric::evaluate(None, Some(1800));
        assert_eq!(metric.outcome, ObjectiveOutcome::Unknown);
        assert!(metric.note.unwrap_or_default().contains("RTO_UNKNOWN"));
    }

    #[test]
    fn rto_without_target_is_unknown() {
        let metric = RtoMetric::evaluate(Some(Duration::from_secs(10)), None);
        assert_eq!(metric.outcome, ObjectiveOutcome::Unknown);
    }

    #[test]
    fn rto_is_rounded_to_milliseconds() {
        let metric = RtoMetric::evaluate(Some(Duration::from_micros(1_234_567)), Some(10));
        assert_eq!(metric.measured_seconds, Some(1.235));
    }

    #[test]
    fn rpo_within_target_passes() {
        let metric = RpoMetric::evaluate(Some(at(8, 0)), at(10, 0), Some(14_400));
        assert_eq!(metric.estimated_seconds, Some(7_200));
        assert_eq!(metric.outcome, ObjectiveOutcome::Pass);
    }

    #[test]
    fn rpo_above_target_fails() {
        let metric = RpoMetric::evaluate(Some(at(0, 0)), at(10, 0), Some(14_400));
        assert_eq!(metric.estimated_seconds, Some(36_000));
        assert_eq!(metric.outcome, ObjectiveOutcome::Fail);
    }

    #[test]
    fn unknown_backup_timestamp_never_invents_a_value() {
        let metric = RpoMetric::evaluate(None, at(10, 0), Some(14_400));
        assert_eq!(metric.estimated_seconds, None);
        assert_eq!(metric.outcome, ObjectiveOutcome::Unknown);
        assert!(metric.note.unwrap_or_default().contains("RPO_UNKNOWN"));
    }

    #[test]
    fn clock_skew_is_clamped_and_flagged() {
        let metric = RpoMetric::evaluate(Some(at(12, 0)), at(10, 0), Some(14_400));
        assert_eq!(metric.estimated_seconds, Some(0));
        assert_eq!(metric.outcome, ObjectiveOutcome::Pass);
        assert!(metric.note.unwrap_or_default().contains("clock skew"));
    }

    #[test]
    fn missing_target_yields_unknown_outcome() {
        let metric = RpoMetric::evaluate(Some(at(8, 0)), at(10, 0), None);
        assert_eq!(metric.estimated_seconds, Some(7_200));
        assert_eq!(metric.outcome, ObjectiveOutcome::Unknown);
    }
}
