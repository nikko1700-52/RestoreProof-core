//! Report errors.

use std::path::PathBuf;

/// Something went wrong while rendering, writing or reading a report.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// The report could not be serialized.
    #[error("cannot encode the report: {0}")]
    Encode(#[source] serde_json::Error),

    /// A report file could not be parsed.
    #[error("`{path}` is not a valid RestoreProof JSON report: {source}")]
    Decode {
        /// File that could not be parsed.
        path: PathBuf,
        /// Underlying cause.
        source: serde_json::Error,
    },

    /// A file or directory could not be written or read.
    #[error("cannot access `{path}`: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying cause.
        source: std::io::Error,
    },
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, ReportError>;
