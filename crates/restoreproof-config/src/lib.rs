//! Configuration loading, validation and path confinement for `RestoreProof`.
//!
//! The entry point is [`Scenario::load`]. Nothing else in the workspace reads a
//! configuration file, and nothing downstream receives an unvalidated path, an
//! unchecked SQL statement or an unaudited Compose file.
//!
//! # Security model of this crate
//!
//! | Attack | Where it is stopped |
//! |---|---|
//! | Path traversal, symlink escape | [`paths::PathPolicy`] |
//! | Argument injection into `restic`/`borg`/`docker` | [`argsafe`] |
//! | Writing or file-reading SQL in a "check" | [`sql_guard`] |
//! | Request forgery through an HTTP check | [`http_guard`] |
//! | Container escape through the Compose file | [`compose_audit`] |
//! | Environment hijacking (`PATH`, `LD_PRELOAD`, `DOCKER_HOST`) | [`argsafe::validate_env_entry`] |
//! | Silent typos that make a drill pass | `deny_unknown_fields` on every struct |
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod argsafe;
pub mod checks;
pub mod compose_audit;
pub mod error;
pub mod http_guard;
pub mod model;
pub mod paths;
pub mod scenario;
pub mod sql_guard;
mod tagged;

pub use error::{ConfigError, Result, ValidationIssue};
pub use model::{RawConfig, ReportFormat, SUPPORTED_VERSION, SecuritySpec};
pub use scenario::{RepositoryLocation, ResolvedBackup, Scenario, SecretSource};
