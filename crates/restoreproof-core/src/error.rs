//! Process exit codes and the error type shared across crates.

use std::fmt;

/// Exit codes returned by the `restoreproof` binary.
///
/// These are part of the public contract of the CLI: CI pipelines are expected
/// to branch on them, so they must stay stable across minor releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum ExitCode {
    /// Every required check passed.
    Success = 0,
    /// At least one required check failed.
    CheckFailed = 1,
    /// The configuration is missing, unreadable or invalid.
    InvalidConfig = 2,
    /// A required external dependency (Docker, restic, borg, ...) is missing.
    MissingDependency = 3,
    /// The backup could not be inspected or restored.
    RestoreFailed = 4,
    /// A global or per-step timeout expired.
    Timeout = 5,
    /// An unexpected internal error occurred.
    Internal = 6,
    /// The command line arguments were used incorrectly.
    Usage = 7,
}

impl ExitCode {
    /// Numeric value handed to the operating system.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// Short human readable explanation, used by `restoreproof version` and docs.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Success => "all required checks passed",
            Self::CheckFailed => "at least one required check failed",
            Self::InvalidConfig => "invalid configuration",
            Self::MissingDependency => "a required external dependency is missing",
            Self::RestoreFailed => "the backup could not be restored",
            Self::Timeout => "a timeout expired",
            Self::Internal => "internal error",
            Self::Usage => "incorrect command line usage",
        }
    }

    /// Every exit code, in numeric order. Used by the documentation tests.
    #[must_use]
    pub const fn all() -> [Self; 8] {
        [
            Self::Success,
            Self::CheckFailed,
            Self::InvalidConfig,
            Self::MissingDependency,
            Self::RestoreFailed,
            Self::Timeout,
            Self::Internal,
            Self::Usage,
        ]
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.code(), self.describe())
    }
}

/// Errors raised by the core domain layer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A status string could not be parsed.
    #[error("unknown status `{0}`")]
    UnknownStatus(String),
    /// A report could not be serialized or its integrity hash computed.
    #[error("report serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl CoreError {
    /// Exit code that best describes this error.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        match self {
            Self::UnknownStatus(_) => ExitCode::InvalidConfig,
            Self::Serialization(_) => ExitCode::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_documented_contract() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::CheckFailed.code(), 1);
        assert_eq!(ExitCode::InvalidConfig.code(), 2);
        assert_eq!(ExitCode::MissingDependency.code(), 3);
        assert_eq!(ExitCode::RestoreFailed.code(), 4);
        assert_eq!(ExitCode::Timeout.code(), 5);
        assert_eq!(ExitCode::Internal.code(), 6);
        assert_eq!(ExitCode::Usage.code(), 7);
    }

    #[test]
    fn all_exit_codes_are_unique_and_ordered() {
        let codes: Vec<i32> = ExitCode::all().iter().map(|c| c.code()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(codes, sorted);
    }
}
