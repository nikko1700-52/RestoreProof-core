//! Report rendering for `RestoreProof`.
//!
//! Three renderings of the same document:
//!
//! * **JSON** — the canonical form. It is what the integrity digest covers and
//!   the only form `restoreproof report --verify` can check.
//! * **Markdown** — written to be pasted into a ticket or handed to an auditor.
//! * **Terminal** — a compact summary for the person running the drill.
//!
//! Reports are written with restrictive permissions (`0600` files in a `0700`
//! directory on Unix): a drill report describes the contents of a production
//! backup, and is not something to leave world-readable.
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
pub mod format;
pub mod markdown;
pub mod terminal;
pub mod writer;

#[cfg(test)]
mod test_support;

pub use error::{ReportError, Result};
pub use writer::{Format, load, render, write_all};
