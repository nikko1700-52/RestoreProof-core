//! Comparing two drill reports.
//!
//! One report tells you whether recovery worked today. Two reports tell you
//! whether it is getting worse — which is the question that actually predicts an
//! outage. `restoreproof diff` answers it locally, on two files you already
//! have, with no server and no history database.
//!
//! # What counts as a regression
//!
//! Only things that mean "recovery got worse":
//!
//! * the overall status moved to a worse one;
//! * a check that used to pass no longer passes;
//! * an objective moved from `PASS` to `FAIL`.
//!
//! A check becoming slower is reported but is **not** a regression on its own:
//! machines differ, and a tool that cries wolf about timing gets ignored.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use restoreproof_core::metrics::ObjectiveOutcome;
use restoreproof_core::report::Report;
use restoreproof_core::{CheckStatus, RunStatus};
use serde::Serialize;

/// How one check changed between two drills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckChange {
    /// Same status in both reports.
    Unchanged,
    /// Did not pass before, passes now.
    Fixed,
    /// Passed before, does not pass now.
    Regressed,
    /// Still not passing, but the status changed (`FAILED` became `ERROR`).
    StillFailing,
    /// Present only in the newer report.
    Added,
    /// Present only in the older report.
    Removed,
}

impl CheckChange {
    /// Whether this change means recovery got worse.
    #[must_use]
    pub const fn is_regression(self) -> bool {
        matches!(self, Self::Regressed)
    }

    const fn symbol(self) -> &'static str {
        match self {
            Self::Unchanged => "  ",
            Self::Fixed => "+ ",
            Self::Regressed => "! ",
            Self::StillFailing => "~ ",
            Self::Added => "> ",
            Self::Removed => "< ",
        }
    }
}

/// One check's change.
#[derive(Debug, Clone, Serialize)]
pub struct CheckDiff {
    /// Check id.
    pub id: String,
    /// Display name, from whichever report has it.
    pub name: String,
    /// Status in the older report.
    pub before: Option<CheckStatus>,
    /// Status in the newer report.
    pub after: Option<CheckStatus>,
    /// Classification.
    pub change: CheckChange,
    /// Duration difference in seconds, when the check exists in both.
    pub duration_delta_seconds: Option<f64>,
}

/// How one objective changed.
#[derive(Debug, Clone, Serialize)]
pub struct ObjectiveDiff {
    /// Objective name: `rto` or `rpo`.
    pub objective: &'static str,
    /// Measured value in the older report.
    pub before_seconds: Option<f64>,
    /// Measured value in the newer report.
    pub after_seconds: Option<f64>,
    /// Outcome in the older report.
    pub before_outcome: ObjectiveOutcome,
    /// Outcome in the newer report.
    pub after_outcome: ObjectiveOutcome,
    /// Whether the objective went from met to missed.
    pub regressed: bool,
}

/// The full comparison.
#[derive(Debug, Clone, Serialize)]
pub struct ReportDiff {
    /// Project of the older report.
    pub before_project: String,
    /// Project of the newer report.
    pub after_project: String,
    /// Whether the two reports describe different projects.
    pub different_projects: bool,
    /// Status of the older report.
    pub before_status: RunStatus,
    /// Status of the newer report.
    pub after_status: RunStatus,
    /// Per-check changes, in the order of the newer report.
    pub checks: Vec<CheckDiff>,
    /// Objective changes.
    pub objectives: Vec<ObjectiveDiff>,
    /// Duration difference of the whole drill.
    pub duration_delta_seconds: f64,
    /// Whether anything got worse.
    pub regressed: bool,
}

/// Rank a run status so two can be compared.
const fn rank(status: RunStatus) -> u8 {
    match status {
        RunStatus::Passed => 4,
        RunStatus::Partial => 3,
        RunStatus::Skipped => 2,
        RunStatus::Failed => 1,
        RunStatus::Error => 0,
    }
}

/// Compare two reports, oldest first.
#[must_use]
pub fn compare(before: &Report, after: &Report) -> ReportDiff {
    let before_checks: BTreeMap<&str, &restoreproof_core::report::CheckOutcome> = before
        .checks
        .iter()
        .map(|check| (check.id.as_str(), check))
        .collect();
    let after_checks: BTreeMap<&str, &restoreproof_core::report::CheckOutcome> = after
        .checks
        .iter()
        .map(|check| (check.id.as_str(), check))
        .collect();

    let mut checks: Vec<CheckDiff> = Vec::new();

    for check in &after.checks {
        let previous = before_checks.get(check.id.as_str());
        let change = match previous {
            None => CheckChange::Added,
            Some(previous) if previous.status == check.status => CheckChange::Unchanged,
            Some(previous) => match (previous.status.is_passed(), check.status.is_passed()) {
                (true, true) => CheckChange::Unchanged,
                (false, true) => CheckChange::Fixed,
                (true, false) => CheckChange::Regressed,
                // Both non-passing, but the status changed: FAILED became
                // ERROR, say. Still broken, and worth showing.
                (false, false) => CheckChange::StillFailing,
            },
        };
        checks.push(CheckDiff {
            id: check.id.clone(),
            name: check.name.clone(),
            before: previous.map(|previous| previous.status),
            after: Some(check.status),
            change,
            duration_delta_seconds: previous
                .map(|previous| round_ms(check.duration_seconds - previous.duration_seconds)),
        });
    }

    for check in &before.checks {
        if !after_checks.contains_key(check.id.as_str()) {
            checks.push(CheckDiff {
                id: check.id.clone(),
                name: check.name.clone(),
                before: Some(check.status),
                after: None,
                change: CheckChange::Removed,
                duration_delta_seconds: None,
            });
        }
    }

    let objectives = vec![
        ObjectiveDiff {
            objective: "rto",
            before_seconds: before.objectives.rto.measured_seconds,
            after_seconds: after.objectives.rto.measured_seconds,
            before_outcome: before.objectives.rto.outcome,
            after_outcome: after.objectives.rto.outcome,
            regressed: before.objectives.rto.outcome == ObjectiveOutcome::Pass
                && after.objectives.rto.outcome == ObjectiveOutcome::Fail,
        },
        ObjectiveDiff {
            objective: "rpo",
            #[allow(clippy::cast_precision_loss)]
            before_seconds: before.objectives.rpo.estimated_seconds.map(|v| v as f64),
            #[allow(clippy::cast_precision_loss)]
            after_seconds: after.objectives.rpo.estimated_seconds.map(|v| v as f64),
            before_outcome: before.objectives.rpo.outcome,
            after_outcome: after.objectives.rpo.outcome,
            regressed: before.objectives.rpo.outcome == ObjectiveOutcome::Pass
                && after.objectives.rpo.outcome == ObjectiveOutcome::Fail,
        },
    ];

    let regressed = rank(after.run.status) < rank(before.run.status)
        || checks.iter().any(|check| check.change.is_regression())
        || objectives.iter().any(|objective| objective.regressed);

    ReportDiff {
        before_project: before.run.project.clone(),
        after_project: after.run.project.clone(),
        different_projects: before.run.project != after.run.project,
        before_status: before.run.status,
        after_status: after.run.status,
        checks,
        objectives,
        duration_delta_seconds: round_ms(after.run.duration_seconds - before.run.duration_seconds),
        regressed,
    }
}

/// Render a comparison for a terminal.
#[must_use]
pub fn render(diff: &ReportDiff, before: &Report, after: &Report) -> String {
    let mut out = String::with_capacity(2048);

    let _ = writeln!(
        out,
        "\n  {} → {}",
        before.run.started_at.format("%Y-%m-%d %H:%M:%SZ"),
        after.run.started_at.format("%Y-%m-%d %H:%M:%SZ")
    );

    if diff.different_projects {
        let _ = writeln!(
            out,
            "\n  WARNING: these reports are for different projects (`{}` and `{}`).",
            diff.before_project, diff.after_project
        );
    }

    let _ = writeln!(
        out,
        "\n  Status   {} → {}{}",
        diff.before_status,
        diff.after_status,
        if diff.before_status == diff.after_status {
            ""
        } else if rank(diff.after_status) < rank(diff.before_status) {
            "   WORSE"
        } else {
            "   better"
        }
    );
    let _ = writeln!(out, "  Duration {:+.1}s", diff.duration_delta_seconds);

    let notable: Vec<&CheckDiff> = diff
        .checks
        .iter()
        .filter(|check| check.change != CheckChange::Unchanged)
        .collect();

    let _ = writeln!(out, "\n  Checks");
    if notable.is_empty() {
        let _ = writeln!(out, "    no change ({} checks)", diff.checks.len());
    } else {
        for check in notable {
            let _ = writeln!(
                out,
                "    {}{:<28} {} → {}",
                check.change.symbol(),
                truncate(&check.name, 28),
                check
                    .before
                    .map_or_else(|| "absent".to_owned(), |status| status.to_string()),
                check
                    .after
                    .map_or_else(|| "absent".to_owned(), |status| status.to_string()),
            );
        }
    }

    let slower: Vec<&CheckDiff> = diff
        .checks
        .iter()
        .filter(|check| {
            check
                .duration_delta_seconds
                .is_some_and(|delta| delta > 1.0)
        })
        .collect();
    if !slower.is_empty() {
        let _ = writeln!(out, "\n  Slower");
        for check in slower {
            let _ = writeln!(
                out,
                "      {:<28} {:+.1}s",
                truncate(&check.name, 28),
                check.duration_delta_seconds.unwrap_or_default()
            );
        }
    }

    let _ = writeln!(out, "\n  Objectives");
    for objective in &diff.objectives {
        let _ = writeln!(
            out,
            "    {:<5} {} → {}{}",
            objective.objective.to_uppercase(),
            format_seconds(objective.before_seconds),
            format_seconds(objective.after_seconds),
            if objective.regressed { "   WORSE" } else { "" }
        );
    }

    let _ = writeln!(
        out,
        "\n  {}\n",
        if diff.regressed {
            "Recovery got worse between these two drills."
        } else {
            "No regression between these two drills."
        }
    );

    out
}

fn format_seconds(value: Option<f64>) -> String {
    value.map_or_else(|| "UNKNOWN".to_owned(), |value| format!("{value:.1}s"))
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn round_ms(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_report;

    #[test]
    fn an_identical_report_shows_no_regression() {
        let report = sample_report();
        let diff = compare(&report, &report);
        assert!(!diff.regressed);
        assert!(
            diff.checks
                .iter()
                .all(|check| check.change == CheckChange::Unchanged)
        );
    }

    #[test]
    fn a_check_that_stops_passing_is_a_regression() {
        let before = sample_report();
        let mut after = sample_report();
        if let Some(check) = after.checks.get_mut(0) {
            check.status = CheckStatus::Failed;
        }
        let diff = compare(&before, &after);
        assert!(diff.regressed);
        assert_eq!(diff.checks[0].change, CheckChange::Regressed);
    }

    #[test]
    fn a_check_that_starts_passing_is_not_a_regression() {
        let before = sample_report();
        let mut after = sample_report();
        if let Some(check) = after.checks.get_mut(1) {
            check.status = CheckStatus::Passed;
        }
        let diff = compare(&before, &after);
        assert!(!diff.regressed);
        assert_eq!(diff.checks[1].change, CheckChange::Fixed);
    }

    #[test]
    fn a_worse_overall_status_is_a_regression() {
        let mut before = sample_report();
        before.run.status = RunStatus::Passed;
        let mut after = sample_report();
        after.run.status = RunStatus::Failed;
        assert!(compare(&before, &after).regressed);
    }

    #[test]
    fn added_and_removed_checks_are_reported_but_are_not_regressions() {
        let before = sample_report();
        let mut after = sample_report();
        after.checks.truncate(1);
        if let Some(check) = after.checks.get_mut(0) {
            check.id = "renamed".to_owned();
        }
        let diff = compare(&before, &after);
        assert!(
            diff.checks
                .iter()
                .any(|check| check.change == CheckChange::Added)
        );
        assert!(
            diff.checks
                .iter()
                .any(|check| check.change == CheckChange::Removed)
        );
        assert!(!diff.regressed);
    }

    #[test]
    fn a_slower_check_is_reported_but_is_not_a_regression() {
        let before = sample_report();
        let mut after = sample_report();
        if let Some(check) = after.checks.get_mut(0) {
            check.duration_seconds += 30.0;
        }
        let diff = compare(&before, &after);
        assert!(!diff.regressed);
        assert_eq!(diff.checks[0].duration_delta_seconds, Some(30.0));

        let text = render(&diff, &before, &after);
        assert!(text.contains("Slower"));
    }

    #[test]
    fn an_objective_going_from_met_to_missed_is_a_regression() {
        let mut before = sample_report();
        before.objectives.rpo.outcome = ObjectiveOutcome::Pass;
        let mut after = sample_report();
        after.objectives.rpo.outcome = ObjectiveOutcome::Fail;
        let diff = compare(&before, &after);
        assert!(diff.regressed);
    }

    #[test]
    fn comparing_different_projects_is_flagged() {
        let before = sample_report();
        let mut after = sample_report();
        after.run.project = "another-app".to_owned();
        let diff = compare(&before, &after);
        assert!(diff.different_projects);
        assert!(render(&diff, &before, &after).contains("different projects"));
    }
}
