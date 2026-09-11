//! `restoreproof diff`: compare two reports.

use std::path::Path;

use restoreproof_core::ExitCode;

use crate::cli::{GlobalArgs, OutputFormat};
use crate::output::Output;

/// Compare two JSON reports and report whether recovery got worse.
///
/// Exit code 1 when something regressed, so this is usable as a CI gate on its
/// own: keep the last known-good report in the repository and diff against it.
pub fn run(global: &GlobalArgs, out: &Output, before: &Path, after: &Path) -> ExitCode {
    let (Some(before_report), Some(after_report)) = (load(out, before), load(out, after)) else {
        return ExitCode::Usage;
    };

    let diff = restoreproof_report::compare(&before_report, &after_report);

    let rendered = match global.format {
        OutputFormat::Json => match serde_json::to_string_pretty(&diff) {
            Ok(json) => format!("{json}\n"),
            Err(err) => {
                out.error(&format!("cannot render the comparison: {err}"));
                return ExitCode::Internal;
            }
        },
        _ => restoreproof_report::diff::render(&diff, &before_report, &after_report),
    };

    if out.emit(&rendered, global.output.as_deref()).is_err() {
        return ExitCode::Internal;
    }

    if diff.regressed {
        ExitCode::CheckFailed
    } else {
        ExitCode::Success
    }
}

fn load(out: &Output, path: &Path) -> Option<restoreproof_core::report::Report> {
    match restoreproof_report::load(path) {
        Ok(report) => Some(report),
        Err(err) => {
            out.error(&err.to_string());
            None
        }
    }
}
