//! Check and run statuses, and the rule that turns the former into the latter.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ExitCode};

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    /// The assertion held.
    Passed,
    /// The check ran and the assertion did not hold.
    Failed,
    /// The check could not be executed (missing tool, unreachable endpoint...).
    Error,
    /// The check was not executed (disabled, or a prerequisite was absent).
    Skipped,
}

impl CheckStatus {
    /// Whether this status proves the assertion.
    #[must_use]
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }

    /// Lower-case identifier used in terminal output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "PASSED",
            Self::Failed => "FAILED",
            Self::Error => "ERROR",
            Self::Skipped => "SKIPPED",
        }
    }
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CheckStatus {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PASSED" | "PASS" | "OK" => Ok(Self::Passed),
            "FAILED" | "FAIL" => Ok(Self::Failed),
            "ERROR" => Ok(Self::Error),
            "SKIPPED" | "SKIP" => Ok(Self::Skipped),
            other => Err(CoreError::UnknownStatus(other.to_owned())),
        }
    }
}

/// Overall outcome of a recovery drill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    /// Every check passed.
    Passed,
    /// All required checks passed, but at least one optional check did not.
    Partial,
    /// At least one required check failed.
    Failed,
    /// At least one required check could not be executed or was skipped.
    Error,
    /// Nothing was executed (for instance `--dry-run`).
    Skipped,
}

impl RunStatus {
    /// Lower-case identifier used in terminal output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "PASSED",
            Self::Partial => "PARTIAL",
            Self::Failed => "FAILED",
            Self::Error => "ERROR",
            Self::Skipped => "SKIPPED",
        }
    }

    /// Exit code the CLI returns for this run status.
    ///
    /// A required check that could not be executed maps to
    /// [`ExitCode::CheckFailed`] and **not** to a success code: an unprovable
    /// recovery is never reported as a proven one.
    #[must_use]
    pub const fn exit_code(self) -> ExitCode {
        match self {
            Self::Passed | Self::Partial | Self::Skipped => ExitCode::Success,
            Self::Failed | Self::Error => ExitCode::CheckFailed,
        }
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunStatus {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "PASSED" | "PASS" => Ok(Self::Passed),
            "PARTIAL" => Ok(Self::Partial),
            "FAILED" | "FAIL" => Ok(Self::Failed),
            "ERROR" => Ok(Self::Error),
            "SKIPPED" | "SKIP" => Ok(Self::Skipped),
            other => Err(CoreError::UnknownStatus(other.to_owned())),
        }
    }
}

/// Aggregate per-check statuses into the status of the whole drill.
///
/// The rule is deliberately strict, because the product claim is "your
/// application can actually be restored":
///
/// * any **required** check that failed  -> [`RunStatus::Failed`]
/// * any **required** check that errored or was skipped -> [`RunStatus::Error`]
/// * all required checks passed, some optional one did not -> [`RunStatus::Partial`]
/// * everything passed -> [`RunStatus::Passed`]
/// * nothing ran -> [`RunStatus::Skipped`]
pub fn aggregate<I>(outcomes: I) -> RunStatus
where
    I: IntoIterator<Item = (bool, CheckStatus)>,
{
    let mut any = false;
    let mut required_failed = false;
    let mut required_unproven = false;
    let mut optional_not_passed = false;

    for (required, status) in outcomes {
        any = true;
        match (required, status) {
            (true, CheckStatus::Failed) => required_failed = true,
            (true, CheckStatus::Error | CheckStatus::Skipped) => required_unproven = true,
            (true, CheckStatus::Passed) => {}
            (false, CheckStatus::Passed) => {}
            (false, _) => optional_not_passed = true,
        }
    }

    if !any {
        return RunStatus::Skipped;
    }
    if required_failed {
        return RunStatus::Failed;
    }
    if required_unproven {
        return RunStatus::Error;
    }
    if optional_not_passed {
        return RunStatus::Partial;
    }
    RunStatus::Passed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_spellings() {
        assert_eq!("pass".parse::<CheckStatus>().unwrap(), CheckStatus::Passed);
        assert_eq!(
            " FAILED ".parse::<CheckStatus>().unwrap(),
            CheckStatus::Failed
        );
        assert_eq!("skip".parse::<CheckStatus>().unwrap(), CheckStatus::Skipped);
        assert!("nonsense".parse::<CheckStatus>().is_err());
    }

    #[test]
    fn round_trips_through_display() {
        for status in [
            CheckStatus::Passed,
            CheckStatus::Failed,
            CheckStatus::Error,
            CheckStatus::Skipped,
        ] {
            assert_eq!(status.to_string().parse::<CheckStatus>().unwrap(), status);
        }
        for status in [
            RunStatus::Passed,
            RunStatus::Partial,
            RunStatus::Failed,
            RunStatus::Error,
            RunStatus::Skipped,
        ] {
            assert_eq!(status.to_string().parse::<RunStatus>().unwrap(), status);
        }
    }

    #[test]
    fn all_passing_is_passed() {
        let status = aggregate([(true, CheckStatus::Passed), (false, CheckStatus::Passed)]);
        assert_eq!(status, RunStatus::Passed);
        assert_eq!(status.exit_code(), ExitCode::Success);
    }

    #[test]
    fn failing_required_check_fails_the_run() {
        let status = aggregate([(true, CheckStatus::Failed), (false, CheckStatus::Passed)]);
        assert_eq!(status, RunStatus::Failed);
        assert_eq!(status.exit_code(), ExitCode::CheckFailed);
    }

    #[test]
    fn skipped_required_check_is_not_a_success() {
        let status = aggregate([(true, CheckStatus::Skipped)]);
        assert_eq!(status, RunStatus::Error);
        assert_eq!(status.exit_code(), ExitCode::CheckFailed);
    }

    #[test]
    fn errored_required_check_is_not_a_success() {
        let status = aggregate([(true, CheckStatus::Error)]);
        assert_eq!(status, RunStatus::Error);
        assert_eq!(status.exit_code(), ExitCode::CheckFailed);
    }

    #[test]
    fn failing_optional_check_is_partial_and_still_exits_zero() {
        let status = aggregate([(true, CheckStatus::Passed), (false, CheckStatus::Failed)]);
        assert_eq!(status, RunStatus::Partial);
        assert_eq!(status.exit_code(), ExitCode::Success);
    }

    #[test]
    fn a_failed_required_check_outranks_an_errored_one() {
        let status = aggregate([(true, CheckStatus::Error), (true, CheckStatus::Failed)]);
        assert_eq!(status, RunStatus::Failed);
    }

    #[test]
    fn nothing_executed_is_skipped() {
        assert_eq!(aggregate([]), RunStatus::Skipped);
    }
}
