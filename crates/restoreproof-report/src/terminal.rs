//! Terminal rendering.
//!
//! Colour is opt-in: the CLI decides, based on whether the stream is a terminal
//! and on `NO_COLOR`. Nothing here writes to stdout directly, so the same
//! rendering can be captured in tests.

use std::fmt::Write as _;

use restoreproof_core::metrics::ObjectiveOutcome;
use restoreproof_core::report::Report;
use restoreproof_core::{CheckStatus, RunStatus};

use crate::format::{bytes, duration, seconds};

/// ANSI styling, applied only when colour is enabled.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    enabled: bool,
}

impl Style {
    /// Build a style.
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\u{1b}[{code}m{text}\u{1b}[0m")
        } else {
            text.to_owned()
        }
    }

    fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }
}

/// Render a report for a terminal.
#[must_use]
pub fn render(report: &Report, style: Style) -> String {
    let mut out = String::with_capacity(2048);

    let _ = writeln!(
        out,
        "\n{} {}  {}",
        run_symbol(report.run.status, style),
        style.bold(report.run.status.as_str()),
        style.dim(&format!(
            "{} · {} · {}",
            report.run.project,
            report.run.started_at.format("%Y-%m-%d %H:%M:%SZ"),
            duration(report.run.duration_seconds)
        ))
    );
    let _ = writeln!(out);

    for check in &report.checks {
        let _ = writeln!(
            out,
            "  {} {:<28} {}  {}",
            check_symbol(check.status, style),
            truncate_column(&check.name, 28),
            style.dim(&format!("{:>8}", duration(check.duration_seconds))),
            check.message
        );
        if check.status != CheckStatus::Passed {
            for detail in &check.details {
                for line in detail.lines().take(6) {
                    let _ = writeln!(out, "      {}", style.dim(line));
                }
            }
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Checks   {} passed, {} failed, {} error, {} skipped (of {})",
        style.green(&report.summary.passed.to_string()),
        style.red(&report.summary.failed.to_string()),
        style.yellow(&report.summary.error.to_string()),
        report.summary.skipped,
        report.summary.total
    );

    let rto = &report.objectives.rto;
    let _ = writeln!(
        out,
        "  RTO      {} (target {})  {}",
        rto.measured_seconds
            .map_or_else(|| "—".to_owned(), duration),
        rto.target_seconds.map_or_else(
            || "not set".to_owned(),
            |v| seconds(i64::try_from(v).unwrap_or(i64::MAX))
        ),
        objective(rto.outcome, "RTO", style)
    );

    let rpo = &report.objectives.rpo;
    let _ = writeln!(
        out,
        "  RPO      {} (target {})  {}",
        rpo.estimated_seconds
            .map_or_else(|| "—".to_owned(), seconds),
        rpo.target_seconds.map_or_else(
            || "not set".to_owned(),
            |v| seconds(i64::try_from(v).unwrap_or(i64::MAX))
        ),
        objective(rpo.outcome, "RPO", style)
    );

    let _ = writeln!(
        out,
        "  Backup   {} · {} · {}",
        report.backup.source_type,
        report
            .backup
            .snapshot_id
            .as_deref()
            .unwrap_or("unknown snapshot"),
        report
            .backup
            .size_bytes
            .map_or_else(|| "size unknown".to_owned(), bytes)
    );

    if report.recovery.environment_started_at.is_some() && !report.recovery.environment_cleaned_up {
        let _ = writeln!(
            out,
            "  {}",
            style.yellow("The recovery environment was kept running. Remove it when you are done.")
        );
    }

    for warning in &report.warnings {
        let _ = writeln!(out, "  {} {warning}", style.yellow("warning:"));
    }
    for error in &report.errors {
        let _ = writeln!(out, "  {} {error}", style.red("error:"));
    }

    let _ = writeln!(out);
    out
}

fn truncate_column(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn run_symbol(status: RunStatus, style: Style) -> String {
    match status {
        RunStatus::Passed => style.green("PASS"),
        RunStatus::Partial => style.yellow("PART"),
        RunStatus::Failed => style.red("FAIL"),
        RunStatus::Error => style.red("ERR "),
        RunStatus::Skipped => style.dim("SKIP"),
    }
}

fn check_symbol(status: CheckStatus, style: Style) -> String {
    match status {
        CheckStatus::Passed => style.green("ok  "),
        CheckStatus::Failed => style.red("fail"),
        CheckStatus::Error => style.yellow("err "),
        CheckStatus::Skipped => style.dim("skip"),
    }
}

fn objective(outcome: ObjectiveOutcome, prefix: &str, style: Style) -> String {
    match outcome {
        ObjectiveOutcome::Pass => style.green(&format!("{prefix}_PASS")),
        ObjectiveOutcome::Fail => style.red(&format!("{prefix}_FAIL")),
        ObjectiveOutcome::Unknown => style.dim(&format!("{prefix}_UNKNOWN")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_can_be_disabled_entirely() {
        let style = Style::new(false);
        assert_eq!(style.green("ok"), "ok");
        assert!(!style.red("bad").contains('\u{1b}'));
    }

    #[test]
    fn colour_is_applied_when_enabled() {
        let style = Style::new(true);
        assert!(style.green("ok").contains("\u{1b}[32m"));
    }

    #[test]
    fn long_names_are_truncated_without_breaking_characters() {
        assert_eq!(truncate_column("short", 10), "short");
        assert_eq!(truncate_column("ééééééééééé", 5), "éééé…");
    }
}
