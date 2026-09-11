//! Check executors for `RestoreProof`.
//!
//! A check is the unit of proof in a recovery drill. This crate turns a
//! validated [`restoreproof_config::checks::CheckSpec`] into a
//! [`restoreproof_core::report::CheckOutcome`], applying the timeout and retry
//! policy, and redacting everything on the way out.
//!
//! # Failure semantics
//!
//! The distinction matters, because it is what makes a report trustworthy:
//!
//! * `PASSED` — the assertion held.
//! * `FAILED` — the check ran and the assertion did not hold. A service that is
//!   down, a query that returns nothing, a missing dump: those are failures of
//!   the *recovery*, which is exactly what the drill is looking for.
//! * `ERROR` — `RestoreProof` could not determine anything: a tool is missing, a
//!   credential was not provided, the environment is not observable. An errored
//!   required check never counts as a success.
//! * `SKIPPED` — the check was deliberately not executed.
//!
//! # Adding a check type
//!
//! Implement [`executor::CheckExecutor`], add a variant to
//! [`restoreproof_config::checks::CheckKind`] and one arm to the dispatch in
//! [`executor`]. Nothing else changes.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod command;
pub mod container;
pub mod context;
pub mod executor;
pub mod file;
pub mod http;
mod program;
pub mod script;
pub mod sql;

#[cfg(test)]
mod test_support;

pub use context::{CheckContext, EnvironmentProbe, ProbeError, ServiceObservation};
pub use executor::{CheckEvaluation, CheckExecutor, run_check};
