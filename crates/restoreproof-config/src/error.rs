//! Configuration errors, written to be actionable rather than terse.

use std::path::{Path, PathBuf};

use restoreproof_core::ExitCode;

/// A single semantic problem found in a configuration document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Dotted path of the offending field, e.g. `recovery.startup_timeout_seconds`.
    pub field: String,
    /// What is wrong, and what to do about it.
    pub message: String,
}

impl ValidationIssue {
    /// Build an issue for `field`.
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// Everything that can go wrong while loading a scenario.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A file could not be read.
    #[error("cannot read `{path}`: {source}")]
    Io {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying cause.
        source: std::io::Error,
    },

    /// A YAML document could not be parsed, or contains unknown fields.
    #[error("invalid YAML in `{path}`: {message}")]
    Yaml {
        /// File that failed to parse.
        path: PathBuf,
        /// Parser message, including the offending line when available.
        message: String,
    },

    /// The `version:` field is not supported by this build.
    #[error("unsupported configuration version {found} in `{path}` (this build supports {supported})")]
    UnsupportedVersion {
        /// File carrying the version.
        path: PathBuf,
        /// Version declared in the document.
        found: u32,
        /// Version supported by this build.
        supported: u32,
    },

    /// One or more semantic rules were violated.
    #[error("{} configuration problem(s) found:\n{}", issues.len(), format_issues(issues))]
    Invalid {
        /// Every problem found, so the user can fix them in one pass.
        issues: Vec<ValidationIssue>,
    },

    /// A referenced file or directory does not exist.
    #[error("{field}: `{path}` does not exist (resolved from the configuration directory)")]
    MissingPath {
        /// Field that referenced the path.
        field: String,
        /// Path as resolved.
        path: PathBuf,
    },

    /// A referenced path escapes the project directory.
    ///
    /// This is the anti path-traversal rule: a configuration file cannot point
    /// the tool at `/etc/shadow` or at a symlink leading outside the project,
    /// unless the operator explicitly allowlisted that location.
    #[error(
        "{field}: `{path}` resolves outside the project directory `{root}`.\n\
         Paths are confined to the project by default. If this location is intentional, add it to \
         `security.allow_external_paths`.{extra}"
    )]
    PathEscapesProject {
        /// Field that referenced the path.
        field: String,
        /// Canonical path that was refused.
        path: PathBuf,
        /// Canonical project root.
        root: PathBuf,
        /// Additional context (for instance: allowlisting is not permitted here).
        extra: String,
    },

    /// A document did not match the expected schema.
    #[error("{context}: {message}")]
    Schema {
        /// Where the problem was found, e.g. `backup` or `checks[2] (app-health)`.
        context: String,
        /// What was expected.
        message: String,
    },

    /// A configuration would create an unsafe recovery environment.
    #[error("refusing to continue for safety: {reason}\nsee docs/security.md")]
    Unsafe {
        /// Why the configuration was refused.
        reason: String,
    },
}

fn format_issues(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| format!("  - {issue}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl ConfigError {
    /// Exit code associated with this error.
    ///
    /// Every configuration problem maps to [`ExitCode::InvalidConfig`] so that
    /// scripts can distinguish "your configuration is wrong" from "the recovery
    /// failed".
    #[must_use]
    pub const fn exit_code(&self) -> ExitCode {
        ExitCode::InvalidConfig
    }

    /// Helper for wrapping an I/O error with the path that caused it.
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, ConfigError>;
