//! Backup sources for `RestoreProof`.
//!
//! A backup source answers two questions: *what is in this backup* and *can it
//! be restored here*. Everything else — starting services, running checks,
//! writing reports — happens elsewhere, which is what makes it possible to add
//! a new storage backend without touching the rest of the product.
//!
//! The open-source edition ships three implementations of
//! [`source::BackupSource`]:
//!
//! | Type | Tool required | Notes |
//! |---|---|---|
//! | `local` | none | a directory or file produced by your own dump job |
//! | `restic` | `restic` | read-only: `snapshots` and `restore` |
//! | `borg` | `borg` | experimental |
//!
//! Object stores, hypervisor snapshots and vendor APIs are deliberately out of
//! scope here; they are implemented against the same trait in the commercial
//! edition, as described in `PREMIUM.md`.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod borg;
pub mod error;
pub mod local;
pub mod metadata;
pub mod restic;
pub mod source;

pub use error::{Result, StorageError};
pub use metadata::{BackupMetadata, RestoreOutcome};
pub use source::{BackupSource, ResolvedSecret, StorageLimits, build};
