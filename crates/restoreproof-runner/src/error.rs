//! Runner errors, each mapped to a documented exit code.

use restoreproof_core::ExitCode;

/// Something went wrong while running a drill.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// Docker, Docker Compose or a backup tool is missing or unusable.
    #[error("{0}")]
    MissingDependency(String),

    /// The backup could not be inspected or restored.
    #[error(transparent)]
    Storage(#[from] restoreproof_storage::StorageError),

    /// The recovery environment could not be started or observed.
    #[error("{0}")]
    Environment(String),

    /// A time limit expired.
    #[error("{0}")]
    Timeout(String),

    /// A report could not be written.
    #[error(transparent)]
    Report(#[from] restoreproof_report::ReportError),

    /// A filesystem operation failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: String,
        /// Underlying cause.
        source: std::io::Error,
    },

    /// An unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}

impl RunnerError {
    /// Exit code for this failure.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::MissingDependency(_) => ExitCode::MissingDependency,
            Self::Storage(err) => err.exit_code(),
            Self::Environment(_) => ExitCode::RestoreFailed,
            Self::Timeout(_) => ExitCode::Timeout,
            Self::Report(_) | Self::Io { .. } | Self::Internal(_) => ExitCode::Internal,
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, RunnerError>;
