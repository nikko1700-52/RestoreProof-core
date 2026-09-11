//! `restoreproof check`: run only the checks, against a running environment.

use std::path::PathBuf;

use restoreproof_core::ExitCode;
use restoreproof_runner::RunMode;

use crate::cli::GlobalArgs;
use crate::output::Output;

/// Run the checks without restoring or starting anything.
pub async fn run(global: &GlobalArgs, out: &Output, restore_dir: Option<PathBuf>) -> ExitCode {
    out.line(
        "\n  Running checks only. Nothing is restored, started or destroyed.\n  \
         The environment must already be running under the Compose project name from your \
         configuration.\n",
    );
    crate::commands::run::run(global, out, RunMode::ChecksOnly, restore_dir).await
}
