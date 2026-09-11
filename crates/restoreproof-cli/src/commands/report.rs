//! `restoreproof report`: display a report written earlier.

use std::path::Path;

use restoreproof_core::ExitCode;
use restoreproof_report::terminal::Style;

use crate::cli::{GlobalArgs, OutputFormat};
use crate::output::Output;

/// Display, and optionally verify, an existing report.
pub fn run(global: &GlobalArgs, out: &Output, file: &Path, verify: bool) -> ExitCode {
    let is_markdown = file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));

    if is_markdown {
        if verify {
            out.error(
                "only JSON reports carry an integrity digest. Verify the `.json` file written \
                 alongside this one.",
            );
            return ExitCode::Usage;
        }
        return match std::fs::read_to_string(file) {
            Ok(text) => {
                if out.emit(&text, global.output.as_deref()).is_err() {
                    return ExitCode::Internal;
                }
                ExitCode::Success
            }
            Err(err) => {
                out.error(&format!("cannot read `{}`: {err}", file.display()));
                ExitCode::Usage
            }
        };
    }

    let report = match restoreproof_report::load(file) {
        Ok(report) => report,
        Err(err) => {
            out.error(&err.to_string());
            return ExitCode::Usage;
        }
    };

    if verify {
        return match report.verify_integrity() {
            Ok(true) => {
                out.line(&format!(
                    "\n  Integrity verified: the report matches its recorded {} digest.\n  {}\n",
                    report.integrity.algorithm, report.integrity.value
                ));
                out.line(
                    "  Note: the digest detects accidental modification. It is not a signature.\n",
                );
                ExitCode::Success
            }
            Ok(false) => {
                out.error(&format!(
                    "integrity check FAILED for `{}`.\nThe recorded digest does not match the \
                     document. The report was modified after it was written.",
                    file.display()
                ));
                ExitCode::CheckFailed
            }
            Err(err) => {
                out.error(&err.to_string());
                ExitCode::Internal
            }
        };
    }

    let rendered = match global.format {
        OutputFormat::Terminal => {
            restoreproof_report::terminal::render(&report, Style::new(out.color()))
        }
        OutputFormat::Markdown => restoreproof_report::markdown::render(&report),
        OutputFormat::Junit => restoreproof_report::junit::render(&report),
        OutputFormat::Prometheus => restoreproof_report::prometheus::render(&report),
        OutputFormat::Json => {
            match restoreproof_report::render(&report, restoreproof_report::Format::Json) {
                Ok(text) => text,
                Err(err) => {
                    out.error(&err.to_string());
                    return ExitCode::Internal;
                }
            }
        }
    };

    if out.emit(&rendered, global.output.as_deref()).is_err() {
        return ExitCode::Internal;
    }
    report.run.status.exit_code()
}
