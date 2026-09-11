//! Errors raised by backup sources.

use std::path::PathBuf;

use restoreproof_core::ExitCode;
use restoreproof_core::process::ProcessError;

/// Something went wrong while inspecting or restoring a backup.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The external tool this source needs is not installed.
    #[error(
        "`{tool}` is required for `{source_type}` backups but is not installed.\n\
         Install it and make sure it is on PATH, or switch to `type: local`."
    )]
    ToolMissing {
        /// Program that is missing.
        tool: &'static str,
        /// Backup type that needs it.
        source_type: &'static str,
    },

    /// The external tool ran but failed.
    #[error("`{tool}` failed ({status}): {details}")]
    ToolFailed {
        /// Program that failed.
        tool: String,
        /// How it ended.
        status: String,
        /// Redacted tail of its standard error.
        details: String,
    },

    /// The external tool produced output that could not be understood.
    #[error("could not interpret the output of `{tool}`: {details}")]
    UnexpectedOutput {
        /// Program whose output was unusable.
        tool: String,
        /// What was expected.
        details: String,
    },

    /// A filesystem operation failed.
    #[error("{operation} failed for `{path}`: {source}")]
    Io {
        /// What was being attempted.
        operation: &'static str,
        /// Path involved.
        path: PathBuf,
        /// Underlying cause.
        source: std::io::Error,
    },

    /// The operation did not finish in time.
    #[error("{0}")]
    Timeout(String),

    /// A configured secret could not be read.
    #[error("cannot read the backup secret: {0}")]
    Secret(String),

    /// The backup itself is unusable.
    #[error("the backup is not usable: {0}")]
    InvalidBackup(String),
}

impl StorageError {
    /// Exit code associated with this failure.
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        match self {
            Self::ToolMissing { .. } => ExitCode::MissingDependency,
            Self::Timeout(_) => ExitCode::Timeout,
            _ => ExitCode::RestoreFailed,
        }
    }

    /// Convert a process error, mapping "not found" to a missing dependency.
    #[must_use]
    pub fn from_process(
        error: ProcessError,
        source_type: &'static str,
        tool: &'static str,
    ) -> Self {
        match error {
            ProcessError::NotFound { .. } => Self::ToolMissing { tool, source_type },
            ProcessError::Timeout { program, seconds } => {
                Self::Timeout(format!("`{program}` did not finish within {seconds}s"))
            }
            ProcessError::Io { program, source } => Self::Io {
                operation: "running",
                path: PathBuf::from(program),
                source,
            },
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, StorageError>;
