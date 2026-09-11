//! `JUnit` XML rendering.
//!
//! Every CI system — Jenkins, GitLab, Azure `DevOps`, `CircleCI`, `GitHub`
//! Actions through a reporter action — knows how to display `JUnit` XML. Emitting it means
//! a recovery drill appears in the same place as the rest of a pipeline's
//! results, with each check as a test case, instead of being a wall of text in a
//! log.
//!
//! # How statuses map
//!
//! | Check | `JUnit` |
//! |---|---|
//! | passed | plain `<testcase>` |
//! | required, failed | `<failure>` |
//! | required, errored or skipped | `<error>` |
//! | **optional**, not passed | `<skipped>`, with a message saying so |
//!
//! Optional checks map to `<skipped>` on purpose. They do not change the exit
//! code, so turning them into `<failure>` would make a pipeline red for
//! something the author deliberately marked as non-blocking. The message states
//! plainly that the check ran and did not pass, so nothing is hidden.
//!
//! The RTO and RPO verdicts are emitted as `<properties>`, not as test cases,
//! because they do not decide the outcome of the drill either.

use std::fmt::Write as _;

use restoreproof_core::CheckStatus;
use restoreproof_core::report::{CheckOutcome, Report};

/// Render a report as `JUnit` XML.
#[must_use]
pub fn render(report: &Report) -> String {
    let summary = &report.summary;
    let optional_not_passed = report
        .checks
        .iter()
        .filter(|check| !check.required && check.status != CheckStatus::Passed)
        .count();
    let failures = report
        .checks
        .iter()
        .filter(|check| check.required && check.status == CheckStatus::Failed)
        .count();
    let errors = report
        .checks
        .iter()
        .filter(|check| {
            check.required && matches!(check.status, CheckStatus::Error | CheckStatus::Skipped)
        })
        .count();

    let mut out = String::with_capacity(4096);
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<testsuites name="restoreproof" tests="{}" failures="{failures}" errors="{errors}" skipped="{optional_not_passed}" time="{:.3}">"#,
        summary.total, report.run.duration_seconds
    );
    let _ = writeln!(
        out,
        r#"  <testsuite name="{}" tests="{}" failures="{failures}" errors="{errors}" skipped="{optional_not_passed}" time="{:.3}" timestamp="{}">"#,
        escape(&report.run.project),
        summary.total,
        report.run.duration_seconds,
        report.run.started_at.format("%Y-%m-%dT%H:%M:%S"),
    );

    render_properties(&mut out, report);

    for check in &report.checks {
        render_case(&mut out, report, check);
    }

    if !report.errors.is_empty() {
        let _ = writeln!(
            out,
            "    <system-err>{}</system-err>",
            escape(&report.errors.join("\n"))
        );
    }

    let _ = writeln!(out, "  </testsuite>");
    let _ = writeln!(out, "</testsuites>");
    out
}

fn render_properties(out: &mut String, report: &Report) {
    let mut properties: Vec<(String, String)> = vec![
        (
            "restoreproof.version".to_owned(),
            report.tool.version.clone(),
        ),
        (
            "restoreproof.platform".to_owned(),
            report.tool.platform.clone(),
        ),
        ("drill.run_id".to_owned(), report.run.id.clone()),
        ("drill.status".to_owned(), report.run.status.to_string()),
        (
            "backup.source_type".to_owned(),
            report.backup.source_type.clone(),
        ),
        ("backup.location".to_owned(), report.backup.location.clone()),
        (
            "config.sha256".to_owned(),
            report.config.config_sha256.clone(),
        ),
        ("report.sha256".to_owned(), report.integrity.value.clone()),
        (
            "rto.outcome".to_owned(),
            format!("{:?}", report.objectives.rto.outcome).to_uppercase(),
        ),
        (
            "rpo.outcome".to_owned(),
            format!("{:?}", report.objectives.rpo.outcome).to_uppercase(),
        ),
    ];

    if let Some(commit) = &report.tool.git_commit {
        properties.push(("restoreproof.commit".to_owned(), commit.clone()));
    }
    if let Some(snapshot) = &report.backup.snapshot_id {
        properties.push(("backup.snapshot".to_owned(), snapshot.clone()));
    }
    if let Some(version) = &report.config.application_version {
        properties.push(("application.version".to_owned(), version.clone()));
    }
    if let Some(measured) = report.objectives.rto.measured_seconds {
        properties.push(("rto.measured_seconds".to_owned(), format!("{measured:.3}")));
    }
    if let Some(target) = report.objectives.rto.target_seconds {
        properties.push(("rto.target_seconds".to_owned(), target.to_string()));
    }
    if let Some(estimated) = report.objectives.rpo.estimated_seconds {
        properties.push(("rpo.estimated_seconds".to_owned(), estimated.to_string()));
    }
    if let Some(target) = report.objectives.rpo.target_seconds {
        properties.push(("rpo.target_seconds".to_owned(), target.to_string()));
    }

    let _ = writeln!(out, "    <properties>");
    for (name, value) in properties {
        let _ = writeln!(
            out,
            r#"      <property name="{}" value="{}"/>"#,
            escape(&name),
            escape(&value)
        );
    }
    let _ = writeln!(out, "    </properties>");
}

fn render_case(out: &mut String, report: &Report, check: &CheckOutcome) {
    let _ = writeln!(
        out,
        r#"    <testcase name="{}" classname="{}.{}" time="{:.3}">"#,
        escape(check.name.as_str()),
        escape(&report.run.project),
        escape(&check.kind),
        check.duration_seconds
    );

    let details = escape(&check.details.join("\n"));
    let message = escape(&check.message);

    match (check.required, check.status) {
        (_, CheckStatus::Passed) => {}
        (true, CheckStatus::Failed) => {
            let _ = writeln!(
                out,
                r#"      <failure message="{message}" type="CheckFailed">{details}</failure>"#
            );
        }
        (true, CheckStatus::Error | CheckStatus::Skipped) => {
            let _ = writeln!(
                out,
                r#"      <error message="{message}" type="CheckNotProven">{details}</error>"#
            );
        }
        (false, _) => {
            let _ = writeln!(
                out,
                r#"      <skipped message="optional check did not pass ({}): {message}"/>"#,
                check.status
            );
        }
    }

    let _ = writeln!(out, "    </testcase>");
}

/// Escape the five XML predefined entities.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters other than tab, newline and carriage return
            // are not valid in XML 1.0 at all.
            c if c.is_control() && c != '\t' && c != '\n' && c != '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::sample_report;

    #[test]
    fn the_document_is_well_formed_enough_for_a_ci_parser() {
        let xml = render(&sample_report());
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<testsuites "));
        assert!(xml.trim_end().ends_with("</testsuites>"));
        assert_eq!(xml.matches("<testcase ").count(), 2);
        assert_eq!(xml.matches("</testcase>").count(), 2);
    }

    #[test]
    fn a_failing_required_check_becomes_a_failure() {
        let xml = render(&sample_report());
        assert!(xml.contains(r#"type="CheckFailed""#));
        assert!(xml.contains("expected at least 1 row(s), got 0"));
        assert!(xml.contains(r#"failures="1""#));
    }

    #[test]
    fn an_optional_check_never_turns_a_pipeline_red() {
        let mut report = sample_report();
        if let Some(check) = report.checks.get_mut(1) {
            check.required = false;
        }
        let xml = render(&report);
        assert!(xml.contains("optional check did not pass"));
        assert!(xml.contains(r#"failures="0""#));
        assert!(xml.contains(r#"errors="0""#));
    }

    #[test]
    fn an_unprovable_required_check_becomes_an_error() {
        let mut report = sample_report();
        if let Some(check) = report.checks.get_mut(1) {
            check.status = restoreproof_core::CheckStatus::Error;
        }
        let xml = render(&report);
        assert!(xml.contains(r#"type="CheckNotProven""#));
        assert!(xml.contains(r#"errors="1""#));
    }

    #[test]
    fn objectives_are_properties_not_test_cases() {
        let xml = render(&sample_report());
        assert!(xml.contains(r#"name="rto.outcome""#));
        assert!(xml.contains(r#"name="rpo.outcome""#));
        assert!(!xml.contains(r#"name="RTO""#));
    }

    #[test]
    fn xml_metacharacters_cannot_break_the_document() {
        let mut report = sample_report();
        if let Some(check) = report.checks.get_mut(0) {
            check.name = r#"<script>alert("x" & 'y')</script>"#.to_owned();
        }
        // The message is only rendered for a non-passing check, so put the
        // hostile text on the failing one.
        if let Some(check) = report.checks.get_mut(1) {
            check.message = "a < b && c > d".to_owned();
            check.details = vec![r#"</failure><injected/>"#.to_owned()];
        }
        let xml = render(&report);

        assert!(!xml.contains("<script>"));
        assert!(xml.contains("&lt;script&gt;"));
        assert!(xml.contains("&amp;&amp;"));
        assert!(!xml.contains("<injected/>"));
        // Exactly one closing tag per failure element: nothing escaped the
        // element it was written into.
        assert_eq!(xml.matches("</failure>").count(), 1);
    }

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(escape("a\u{0}b\u{7}c"), "a b c");
        assert_eq!(escape("keep\tthis\nand\rthis"), "keep\tthis\nand\rthis");
    }
}
