//! Prometheus text-format rendering.
//!
//! Written for the `node_exporter` textfile collector: point `--format
//! prometheus --output /var/lib/node_exporter/textfile/restoreproof.prom` at a
//! scheduled drill and recovery becomes something you can alert on, with no
//! server, no account and no data leaving the machine.
//!
//! ```promql
//! # A drill proved the recovery does not work.
//! restoreproof_drill_success == 0
//!
//! # No drill has run for more than a day: silence is not success.
//! time() - restoreproof_drill_completed_timestamp_seconds > 86400
//!
//! # Recovery is getting slower, or the objective is no longer met.
//! restoreproof_rto_seconds > restoreproof_rto_target_seconds
//! ```
//!
//! The second query matters as much as the first. A monitoring integration that
//! only reports failures cannot tell you that the drill stopped running.
//!
//! # Unknown values are omitted, never zeroed
//!
//! When the RPO cannot be determined, no `restoreproof_rpo_seconds` sample is
//! emitted. A missing series is visibly missing in Prometheus; a `0` would read
//! as "no data loss", which would be a lie.

use std::fmt::Write as _;

use restoreproof_core::metrics::ObjectiveOutcome;
use restoreproof_core::report::Report;
use restoreproof_core::{CheckStatus, RunStatus};

/// One metric family with its samples.
struct Family {
    name: &'static str,
    help: &'static str,
    samples: Vec<(Vec<(&'static str, String)>, String)>,
}

impl Family {
    fn new(name: &'static str, help: &'static str) -> Self {
        Self {
            name,
            help,
            samples: Vec::new(),
        }
    }

    // A builder that accepts any displayable value. Taking it by value is the
    // point: every call site passes either a small `Copy` number or a freshly
    // built `String`, and borrowing would only add noise.
    #[allow(clippy::needless_pass_by_value)]
    fn sample(mut self, labels: Vec<(&'static str, String)>, value: impl ToString) -> Self {
        self.samples.push((labels, value.to_string()));
        self
    }

    fn render(&self, out: &mut String) {
        if self.samples.is_empty() {
            return;
        }
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} gauge", self.name);
        for (labels, value) in &self.samples {
            let rendered: Vec<String> = labels
                .iter()
                .map(|(key, value)| format!("{key}=\"{}\"", escape(value)))
                .collect();
            let _ = writeln!(out, "{}{{{}}} {value}", self.name, rendered.join(","));
        }
    }
}

/// Render a report in the Prometheus text exposition format.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render(report: &Report) -> String {
    let project = report.run.project.clone();
    let base = || vec![("project", project.clone())];

    let mut families = Vec::new();

    families.push(
        Family::new(
            "restoreproof_drill_info",
            "Metadata about the last recovery drill. Always 1.",
        )
        .sample(
            vec![
                ("project", project.clone()),
                ("run_id", report.run.id.clone()),
                ("status", report.run.status.to_string()),
                ("version", report.tool.version.clone()),
                ("platform", report.tool.platform.clone()),
                ("backup_source", report.backup.source_type.clone()),
                (
                    "snapshot",
                    report.backup.snapshot_id.clone().unwrap_or_default(),
                ),
            ],
            1,
        ),
    );

    families.push(
        Family::new(
            "restoreproof_drill_success",
            "1 if every required check passed, 0 otherwise.",
        )
        .sample(
            base(),
            i32::from(matches!(
                report.run.status,
                RunStatus::Passed | RunStatus::Partial
            )),
        ),
    );

    families.push(
        Family::new(
            "restoreproof_drill_completed_timestamp_seconds",
            "Unix timestamp at which the last drill finished. Alert on staleness.",
        )
        .sample(base(), report.run.finished_at.timestamp()),
    );

    families.push(
        Family::new(
            "restoreproof_drill_duration_seconds",
            "Wall-clock duration of the last drill.",
        )
        .sample(base(), format!("{:.3}", report.run.duration_seconds)),
    );

    if let Some(duration) = report.recovery.restore_duration_seconds {
        families.push(
            Family::new(
                "restoreproof_restore_duration_seconds",
                "Time spent restoring the backup, excluding service startup and checks.",
            )
            .sample(base(), format!("{duration:.3}")),
        );
    }

    families.push(
        Family::new(
            "restoreproof_checks_total",
            "Number of checks recorded by the last drill.",
        )
        .sample(base(), report.summary.total),
    );

    let mut by_status = Family::new(
        "restoreproof_checks",
        "Number of checks in the last drill, by outcome.",
    );
    for (status, count) in [
        ("passed", report.summary.passed),
        ("failed", report.summary.failed),
        ("error", report.summary.error),
        ("skipped", report.summary.skipped),
    ] {
        by_status = by_status.sample(
            vec![("project", project.clone()), ("status", status.to_owned())],
            count,
        );
    }
    families.push(by_status);

    let mut check_status = Family::new(
        "restoreproof_check_status",
        "1 if the named check passed, 0 otherwise.",
    );
    let mut check_duration = Family::new(
        "restoreproof_check_duration_seconds",
        "Duration of the named check.",
    );
    for check in &report.checks {
        let labels = vec![
            ("project", project.clone()),
            ("check", check.id.clone()),
            ("type", check.kind.clone()),
            ("required", check.required.to_string()),
        ];
        check_status = check_status.sample(
            labels.clone(),
            i32::from(check.status == CheckStatus::Passed),
        );
        check_duration = check_duration.sample(labels, format!("{:.3}", check.duration_seconds));
    }
    families.push(check_status);
    families.push(check_duration);

    // Objectives. A value that could not be determined is omitted, never zeroed.
    if let Some(measured) = report.objectives.rto.measured_seconds {
        families.push(
            Family::new(
                "restoreproof_rto_seconds",
                "Measured recovery time: start of the drill until every required check passed.",
            )
            .sample(base(), format!("{measured:.3}")),
        );
    }
    if let Some(target) = report.objectives.rto.target_seconds {
        families.push(
            Family::new(
                "restoreproof_rto_target_seconds",
                "Configured Recovery Time Objective.",
            )
            .sample(base(), target),
        );
    }
    if let Some(met) = objective_value(report.objectives.rto.outcome) {
        families.push(
            Family::new(
                "restoreproof_rto_met",
                "1 if the measured recovery time is within the objective, 0 otherwise.",
            )
            .sample(base(), met),
        );
    }

    if let Some(estimated) = report.objectives.rpo.estimated_seconds {
        families.push(
            Family::new(
                "restoreproof_rpo_seconds",
                "Estimated data loss window: age of the restored backup.",
            )
            .sample(base(), estimated),
        );
    }
    if let Some(target) = report.objectives.rpo.target_seconds {
        families.push(
            Family::new(
                "restoreproof_rpo_target_seconds",
                "Configured Recovery Point Objective.",
            )
            .sample(base(), target),
        );
    }
    if let Some(met) = objective_value(report.objectives.rpo.outcome) {
        families.push(
            Family::new(
                "restoreproof_rpo_met",
                "1 if the estimated data loss window is within the objective, 0 otherwise.",
            )
            .sample(base(), met),
        );
    }

    if let Some(size) = report.backup.size_bytes {
        families.push(
            Family::new(
                "restoreproof_backup_size_bytes",
                "Size of the restored backup.",
            )
            .sample(base(), size),
        );
    }
    if let Some(created) = report.backup.created_at {
        families.push(
            Family::new(
                "restoreproof_backup_timestamp_seconds",
                "Unix timestamp at which the restored backup was taken.",
            )
            .sample(base(), created.timestamp()),
        );
    }

    let mut out = String::with_capacity(2048);
    for family in &families {
        family.render(&mut out);
    }
    out
}

/// `PASS` and `FAIL` become 1 and 0; `UNKNOWN` emits no sample at all.
const fn objective_value(outcome: ObjectiveOutcome) -> Option<i32> {
    match outcome {
        ObjectiveOutcome::Pass => Some(1),
        ObjectiveOutcome::Fail => Some(0),
        ObjectiveOutcome::Unknown => None,
    }
}

/// Escape a label value per the Prometheus text format.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_report;

    fn value_of(text: &str, metric: &str) -> Option<String> {
        text.lines()
            .find(|line| line.starts_with(metric) && line.contains('{'))
            .and_then(|line| line.rsplit(' ').next().map(str::to_owned))
    }

    #[test]
    fn every_family_declares_help_and_type() {
        let text = render(&sample_report());
        let names: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("# HELP "))
            .collect();
        assert!(!names.is_empty());
        assert_eq!(
            names.len(),
            text.lines()
                .filter(|line| line.starts_with("# TYPE "))
                .count()
        );
    }

    #[test]
    fn a_failed_drill_reports_success_zero() {
        let text = render(&sample_report());
        assert_eq!(
            value_of(&text, "restoreproof_drill_success"),
            Some("0".to_owned())
        );
    }

    #[test]
    fn a_passing_drill_reports_success_one() {
        let mut report = sample_report();
        report.run.status = RunStatus::Passed;
        let text = render(&report);
        assert_eq!(
            value_of(&text, "restoreproof_drill_success"),
            Some("1".to_owned())
        );
    }

    #[test]
    fn a_partial_drill_still_counts_as_success() {
        let mut report = sample_report();
        report.run.status = RunStatus::Partial;
        let text = render(&report);
        assert_eq!(
            value_of(&text, "restoreproof_drill_success"),
            Some("1".to_owned())
        );
    }

    #[test]
    fn an_unknown_objective_emits_no_sample_rather_than_zero() {
        let report = sample_report();
        let text = render(&report);
        // The sample report has no measured RTO, so the objective is UNKNOWN.
        assert!(!text.contains("restoreproof_rto_seconds"));
        assert!(!text.contains("restoreproof_rto_met"));
        // The RPO is known, so it is present.
        assert!(text.contains("restoreproof_rpo_seconds"));
    }

    #[test]
    fn a_staleness_timestamp_is_always_present() {
        let text = render(&sample_report());
        assert!(text.contains("restoreproof_drill_completed_timestamp_seconds"));
    }

    #[test]
    fn per_check_series_carry_the_check_identity() {
        let text = render(&sample_report());
        assert!(text.contains(r#"check="app-health""#));
        assert!(text.contains(r#"type="sql""#));
        assert!(text.contains(r#"required="true""#));
    }

    #[test]
    fn label_values_are_escaped() {
        let mut report = sample_report();
        report.run.project = "weird\"name\\with\nbreaks".to_owned();
        let text = render(&report);
        assert!(text.contains(r#"project="weird\"name\\with\nbreaks""#));
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            assert_eq!(
                line.matches('{').count(),
                1,
                "an unescaped label broke a sample line: {line}"
            );
        }
    }

    #[test]
    fn the_output_ends_with_a_newline() {
        let text = render(&sample_report());
        assert!(
            text.ends_with('\n'),
            "the textfile collector requires a trailing newline"
        );
    }
}
