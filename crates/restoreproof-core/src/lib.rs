//! Core domain model shared by every `RestoreProof` crate.
//!
//! It holds the vocabulary of a recovery drill (statuses, exit codes,
//! objectives, the report document) so that the CLI, the runner and any future
//! front-end agree on the same semantics, together with the security primitives
//! every other crate is required to use.
//!
//! # Security notes
//!
//! * [`secret::Secret`] is the only supported way to carry a credential in
//!   memory. It has no [`std::fmt::Display`] implementation, its `Debug`
//!   implementation prints a placeholder, and it is zeroized on drop.
//! * [`redact::Redactor`] is applied to **every** string that reaches a report,
//!   a log line or the terminal.
//! * [`process::CommandSpec`] is the only way an external program is started:
//!   no shell, no inherited environment, no inherited stdin, bounded time and
//!   bounded output.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod error;
pub mod metrics;
pub mod process;
pub mod redact;
pub mod report;
pub mod secret;
pub mod status;

pub use error::{CoreError, ExitCode};
pub use metrics::{ObjectiveOutcome, RpoMetric, RtoMetric};
pub use process::{CommandOutput, CommandSpec, ProcessError};
pub use redact::{REDACTED, Redactor, truncate};
pub use report::Report;
pub use secret::Secret;
pub use status::{CheckStatus, RunStatus};
