//! Recovery drill orchestration.
//!
//! This crate owns the part of `RestoreProof` that actually touches the
//! machine: it restores a backup into a private workspace, starts an isolated
//! Docker Compose environment, waits for it, runs the checks against it,
//! destroys it, and assembles the report.
//!
//! It depends on the other crates and nothing depends on it except the CLI,
//! which is what makes a different front-end — a scheduler, an API, a web
//! console — possible without touching any of the logic here.
//!
//! # Guarantees
//!
//! * The recovery environment is destroyed on every path, including panics and
//!   timeouts, unless `--keep-environment` was passed.
//! * The workspace holding restored data is private (`0700`) and removed with
//!   the same rule.
//! * A report is always produced, including when the restore itself failed.
//! * No secret reaches a log line, a report or the terminal.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod docker;
pub mod environment;
pub mod error;
pub mod plan;
pub mod runner;
pub mod workspace;

pub use docker::{DockerCli, DockerInfo};
pub use environment::RecoveryEnvironment;
pub use error::{Result, RunnerError};
pub use plan::Plan;
pub use runner::{DrillOutcome, RunMode, RunOptions, execute};
pub use workspace::Workspace;
