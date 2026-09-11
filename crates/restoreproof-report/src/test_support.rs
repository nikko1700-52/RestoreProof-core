//! A representative report used by the tests of this crate.

use chrono::{TimeZone, Utc};
use restoreproof_core::metrics::{RpoMetric, RtoMetric};
use restoreproof_core::report::{
    BackupInfo, CheckOutcome, CheckSummary, ConfigInfo, Integrity, Objectives,
    REPORT_SCHEMA_VERSION, RecoveryTimeline, Report, RunInfo, ToolInfo, sha256_hex,
};
use restoreproof_core::{CheckStatus, Redactor, RunStatus};

pub(crate) fn sample_report() -> Report {
    let started = Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap();
    let finished = Utc.with_ymd_and_hms(2026, 1, 15, 10, 2, 3).unwrap();
    let backup_time = Utc.with_ymd_and_hms(2026, 1, 15, 8, 0, 0).unwrap();

    let checks = vec![
        CheckOutcome {
            id: "app-health".to_owned(),
            name: "Application health endpoint".to_owned(),
            description: Some("The restored app answers on /health".to_owned()),
            kind: "http".to_owned(),
            required: true,
            status: CheckStatus::Passed,
            started_at: started,
            duration_seconds: 0.412,
            message: "HTTP 200 from http://127.0.0.1:18080/health".to_owned(),
            details: vec!["HTTP 200 from http://127.0.0.1:18080/health".to_owned()],
            attempts: 3,
        },
        CheckOutcome {
            id: "orders-present".to_owned(),
            name: "Orders table is populated".to_owned(),
            description: None,
            kind: "sql".to_owned(),
            required: true,
            status: CheckStatus::Failed,
            started_at: started,
            duration_seconds: 1.2,
            message: "expected at least 1 row(s), got 0".to_owned(),
            details: vec!["rows: 0".to_owned()],
            attempts: 1,
        },
    ];

    let mut report = Report {
        schema_version: REPORT_SCHEMA_VERSION,
        tool: ToolInfo::detect("0.1.0", Some("abcdef1")),
        run: RunInfo {
            id: "0d1a5f7c-2b3e-4a1d-9f88-112233445566".to_owned(),
            project: "example-app".to_owned(),
            description: Some("Nightly recovery drill".to_owned()),
            started_at: started,
            finished_at: finished,
            duration_seconds: 123.0,
            status: RunStatus::Failed,
        },
        backup: BackupInfo {
            source_type: "restic".to_owned(),
            location: "/srv/backups/repo".to_owned(),
            snapshot_id: Some("a1b2c3d4".to_owned()),
            created_at: Some(backup_time),
            size_bytes: Some(5 * 1024 * 1024),
        },
        recovery: RecoveryTimeline {
            restore_started_at: Some(started),
            restore_finished_at: Some(started),
            restore_duration_seconds: Some(31.5),
            environment_started_at: Some(started),
            ready_at: None,
            environment_cleaned_up: true,
        },
        objectives: Objectives {
            rto: RtoMetric::evaluate(None, Some(1800)),
            rpo: RpoMetric::evaluate(Some(backup_time), started, Some(14_400)),
        },
        summary: CheckSummary::default(),
        checks,
        errors: Vec::new(),
        warnings: vec!["metrics.target_rto_seconds is set to 30 minutes".to_owned()],
        config: ConfigInfo {
            path: "examples/postgres-local/restoreproof.yaml".to_owned(),
            version: 1,
            application_version: Some("2026.1.3".to_owned()),
            config_sha256: sha256_hex(b"version: 1"),
        },
        integrity: Integrity::default(),
    };
    report
        .finalize(&Redactor::new())
        .unwrap_or_else(|err| unreachable!("sample report must serialize: {err}"));
    report
}
