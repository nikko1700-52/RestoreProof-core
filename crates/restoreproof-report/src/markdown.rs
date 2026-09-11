//! Markdown rendering.
//!
//! The output is written to be pasted into a ticket or sent to a customer: it
//! leads with the verdict, states the objectives, then details every check.

use std::fmt::Write as _;

use restoreproof_core::metrics::ObjectiveOutcome;
use restoreproof_core::report::Report;
use restoreproof_core::{CheckStatus, RunStatus};

use crate::format::{bytes, duration, optional, seconds, table_cell};

/// Render a report as Markdown.
#[must_use]
pub fn render(report: &Report) -> String {
    let mut out = String::with_capacity(4096);

    let _ = writeln!(out, "# Recovery drill — {}", report.run.project);
    let _ = writeln!(out);
    if let Some(description) = &report.run.description {
        let _ = writeln!(out, "_{description}_");
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "## {} {}",
        status_badge(report.run.status),
        report.run.status
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} of {} checks passed in {}.",
        report.summary.passed,
        report.summary.total,
        duration(report.run.duration_seconds)
    );
    let _ = writeln!(out);

    render_objectives(&mut out, report);
    render_run_details(&mut out, report);
    render_checks(&mut out, report);
    render_lists(&mut out, report);
    render_integrity(&mut out, report);

    out
}

fn render_objectives(out: &mut String, report: &Report) {
    let rto = &report.objectives.rto;
    let rpo = &report.objectives.rpo;

    let _ = writeln!(out, "## Objectives");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Objective | Measured | Target | Result |");
    let _ = writeln!(out, "|---|---|---|---|");
    let _ = writeln!(
        out,
        "| RTO (time to recover) | {} | {} | {} |",
        rto.measured_seconds
            .map_or_else(|| "—".to_owned(), duration),
        rto.target_seconds.map_or_else(
            || "not set".to_owned(),
            |value| seconds(i64::try_from(value).unwrap_or(i64::MAX))
        ),
        objective_label("RTO", rto.outcome)
    );
    let _ = writeln!(
        out,
        "| RPO (data loss window) | {} | {} | {} |",
        rpo.estimated_seconds
            .map_or_else(|| "—".to_owned(), seconds),
        rpo.target_seconds.map_or_else(
            || "not set".to_owned(),
            |value| seconds(i64::try_from(value).unwrap_or(i64::MAX))
        ),
        objective_label("RPO", rpo.outcome)
    );
    let _ = writeln!(out);

    for note in [rto.note.as_ref(), rpo.note.as_ref()].into_iter().flatten() {
        let _ = writeln!(out, "> {note}");
        let _ = writeln!(out);
    }
}

fn render_run_details(out: &mut String, report: &Report) {
    let _ = writeln!(out, "## Run");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | |");
    let _ = writeln!(out, "|---|---|");
    let _ = writeln!(out, "| Run id | `{}` |", report.run.id);
    let _ = writeln!(
        out,
        "| Started (UTC) | {} |",
        report.run.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    let _ = writeln!(
        out,
        "| Finished (UTC) | {} |",
        report.run.finished_at.format("%Y-%m-%d %H:%M:%S")
    );
    let _ = writeln!(
        out,
        "| RestoreProof | {} ({}) |",
        report.tool.version, report.tool.platform
    );
    if let Some(commit) = &report.tool.git_commit {
        let _ = writeln!(out, "| Build | `{commit}` |");
    }
    let _ = writeln!(out, "| Backup source | {} |", report.backup.source_type);
    let _ = writeln!(
        out,
        "| Backup location | `{}` |",
        table_cell(&report.backup.location)
    );
    let _ = writeln!(
        out,
        "| Snapshot | {} |",
        report
            .backup
            .snapshot_id
            .as_deref()
            .map_or_else(|| "—".to_owned(), |id| format!("`{id}`"))
    );
    let _ = writeln!(
        out,
        "| Backup taken (UTC) | {} |",
        report.backup.created_at.map_or_else(
            || "UNKNOWN".to_owned(),
            |ts| ts.format("%Y-%m-%d %H:%M:%S").to_string()
        )
    );
    let _ = writeln!(
        out,
        "| Backup size | {} |",
        optional(report.backup.size_bytes.map(bytes))
    );
    let _ = writeln!(
        out,
        "| Restore duration | {} |",
        optional(report.recovery.restore_duration_seconds.map(duration))
    );
    let _ = writeln!(
        out,
        "| Environment destroyed | {} |",
        match (
            report.recovery.environment_started_at,
            report.recovery.environment_cleaned_up,
        ) {
            (None, _) => "no environment was started",
            (Some(_), true) => "yes",
            (Some(_), false) => "**no** — clean it up manually",
        }
    );
    let _ = writeln!(
        out,
        "| Configuration | `{}` |",
        table_cell(&report.config.path)
    );
    if let Some(version) = &report.config.application_version {
        let _ = writeln!(out, "| Application version | `{version}` |");
    }
    let _ = writeln!(out);
}

fn render_checks(out: &mut String, report: &Report) {
    let _ = writeln!(out, "## Checks");
    let _ = writeln!(out);
    let _ = writeln!(out, "| | Check | Type | Required | Duration | Result |");
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for check in &report.checks {
        let _ = writeln!(
            out,
            "| {} | {} | `{}` | {} | {} | {} |",
            check_badge(check.status),
            table_cell(&check.name),
            check.kind,
            if check.required { "yes" } else { "no" },
            duration(check.duration_seconds),
            table_cell(&check.message)
        );
    }
    let _ = writeln!(out);

    let interesting: Vec<_> = report
        .checks
        .iter()
        .filter(|check| check.status != CheckStatus::Passed && !check.details.is_empty())
        .collect();
    if !interesting.is_empty() {
        let _ = writeln!(out, "### Details of non-passing checks");
        let _ = writeln!(out);
        for check in interesting {
            let _ = writeln!(
                out,
                "<details><summary><code>{}</code> — {}</summary>",
                check.id,
                table_cell(&check.message)
            );
            let _ = writeln!(out);
            let _ = writeln!(out, "```text");
            for detail in &check.details {
                let _ = writeln!(out, "{detail}");
            }
            let _ = writeln!(out, "```");
            let _ = writeln!(out);
            let _ = writeln!(out, "</details>");
            let _ = writeln!(out);
        }
    }
}

fn render_lists(out: &mut String, report: &Report) {
    if !report.errors.is_empty() {
        let _ = writeln!(out, "## Errors");
        let _ = writeln!(out);
        for error in &report.errors {
            let _ = writeln!(out, "- {error}");
        }
        let _ = writeln!(out);
    }
    if !report.warnings.is_empty() {
        let _ = writeln!(out, "## Warnings");
        let _ = writeln!(out);
        for warning in &report.warnings {
            let _ = writeln!(out, "- {warning}");
        }
        let _ = writeln!(out);
    }
}

fn render_integrity(out: &mut String, report: &Report) {
    let _ = writeln!(out, "## Integrity");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "- Report digest (`{}`): `{}`",
        report.integrity.algorithm, report.integrity.value
    );
    let _ = writeln!(
        out,
        "- Configuration digest: `{}`",
        report.config.config_sha256
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "> The digest detects accidental modification of the JSON report. It is not a signature: \
         it proves nothing against someone who can also recompute it. Signed reports are part of \
         the commercial edition."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "<sub>Generated by RestoreProof Core {} — verify with `restoreproof report <file> --verify`.</sub>",
        report.tool.version
    );
}

const fn status_badge(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Passed => "✅",
        RunStatus::Partial => "⚠️",
        RunStatus::Failed => "❌",
        RunStatus::Error => "🛑",
        RunStatus::Skipped => "⏭️",
    }
}

const fn check_badge(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Passed => "✅",
        CheckStatus::Failed => "❌",
        CheckStatus::Error => "🛑",
        CheckStatus::Skipped => "⏭️",
    }
}

fn objective_label(prefix: &str, outcome: ObjectiveOutcome) -> String {
    match outcome {
        ObjectiveOutcome::Pass => format!("✅ {prefix}_PASS"),
        ObjectiveOutcome::Fail => format!("❌ {prefix}_FAIL"),
        ObjectiveOutcome::Unknown => format!("❔ {prefix}_UNKNOWN"),
    }
}
