//! Orchestration of a recovery drill.
//!
//! The sequence is deliberately linear and always ends the same way:
//!
//! ```text
//! resolve secrets → check dependencies → create a private workspace
//!   → inspect the backup → restore it → start the environment
//!   → wait for readiness → run the checks → measure RTO/RPO
//!   → destroy the environment → write the report
//! ```
//!
//! Two invariants hold on every path, including panics and timeouts:
//!
//! * the environment and the workspace are destroyed (see
//!   [`crate::environment`] and [`crate::workspace`]);
//! * a report is produced. A drill that fails to restore is still a result, and
//!   an operator needs the document that says so.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use restoreproof_checks::context::CheckContext;
use restoreproof_checks::run_check;
use restoreproof_config::Scenario;
use restoreproof_config::checks::CheckKind;
use restoreproof_core::metrics::{RpoMetric, RtoMetric};
use restoreproof_core::report::{
    BackupInfo, CheckOutcome, CheckSummary, ConfigInfo, Integrity, Objectives,
    REPORT_SCHEMA_VERSION, RecoveryTimeline, Report, RunInfo, ToolInfo,
};
use restoreproof_core::{ExitCode, Redactor, RunStatus, Secret, status};
use restoreproof_storage::{BackupMetadata, StorageLimits};
use serde::Deserialize;

use crate::docker::DockerCli;
use crate::environment::{RecoveryEnvironment, project_directory, unique_project_name};
use crate::error::{Result, RunnerError};
use crate::workspace::Workspace;

/// What the runner should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Restore the backup, start the environment, run the checks.
    Full,
    /// Only run the checks, against an environment the operator already started.
    ChecksOnly,
}

/// Knobs the CLI exposes.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// What to do.
    pub mode: RunMode,
    /// Keep the recovery environment and the workspace after the drill.
    pub keep_environment: bool,
    /// Override `recovery.cleanup`.
    pub cleanup_override: Option<bool>,
    /// Override `recovery.total_timeout_seconds`.
    pub timeout_override: Option<u64>,
    /// Produce a report describing the plan without executing anything.
    pub dry_run: bool,
    /// Directory holding already-restored data, for [`RunMode::ChecksOnly`].
    pub restore_dir: Option<PathBuf>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            mode: RunMode::Full,
            keep_environment: false,
            cleanup_override: None,
            timeout_override: None,
            dry_run: false,
            restore_dir: None,
        }
    }
}

/// Result of a drill.
#[derive(Debug, Clone)]
pub struct DrillOutcome {
    /// The report, already redacted and sealed.
    pub report: Report,
    /// Files the report was written to.
    pub report_files: Vec<PathBuf>,
    /// Exit code the CLI should return.
    pub exit_code: ExitCode,
    /// Compose project left running, when the environment was kept.
    pub kept_environment: Option<String>,
    /// Workspace left on disk, when it was kept.
    pub kept_workspace: Option<PathBuf>,
}

/// Optional file overriding the timestamps used for the RPO computation.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RpoReference {
    #[serde(default)]
    backup_timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    reference_timestamp: Option<DateTime<Utc>>,
}

/// Mutable state accumulated as the drill progresses.
#[derive(Default)]
struct Progress {
    metadata: Option<BackupMetadata>,
    restore_started_at: Option<DateTime<Utc>>,
    restore_finished_at: Option<DateTime<Utc>>,
    environment_started_at: Option<DateTime<Utc>>,
    checks: Vec<CheckOutcome>,
    restored_bytes: Option<u64>,
    cleaned_up: bool,
    kept_environment: Option<String>,
    warnings: Vec<String>,
    errors: Vec<String>,
}

/// Run a drill and produce a report, whatever happens.
///
/// # Errors
///
/// Only a failure to *write* the report is returned as an error; every other
/// failure is recorded in the report itself and reflected in the exit code.
pub async fn execute(
    scenario: &Scenario,
    options: &RunOptions,
    tool: &ToolInfo,
) -> Result<DrillOutcome> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let started_at = Utc::now();
    let clock = Instant::now();

    let mut redactor = Redactor::new();
    let mut progress = Progress {
        warnings: scenario.warnings.clone(),
        ..Progress::default()
    };

    let total_timeout = Duration::from_secs(
        options
            .timeout_override
            .unwrap_or(scenario.raw.recovery.total_timeout_seconds)
            .max(1),
    );

    let result = if options.dry_run {
        progress
            .warnings
            .push("--dry-run: nothing was restored, started or checked".to_owned());
        Ok(())
    } else {
        match tokio::time::timeout(
            total_timeout,
            drill(scenario, options, &run_id, &mut redactor, &mut progress),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(RunnerError::Timeout(format!(
                "the drill exceeded its total budget of {}s and was stopped; the recovery \
                 environment has been destroyed",
                total_timeout.as_secs()
            ))),
        }
    };

    if let Err(err) = &result {
        progress.errors.push(err.to_string());
    }

    let finished_at = Utc::now();
    let mut report = assemble(
        scenario,
        options,
        tool,
        &run_id,
        started_at,
        finished_at,
        clock,
        &progress,
    );
    report
        .finalize(&redactor)
        .map_err(|err| RunnerError::Internal(err.to_string()))?;

    let formats: Vec<restoreproof_report::Format> = scenario
        .formats()
        .iter()
        .map(|format| match format {
            restoreproof_config::ReportFormat::Json => restoreproof_report::Format::Json,
            restoreproof_config::ReportFormat::Markdown => restoreproof_report::Format::Markdown,
        })
        .collect();
    let report_files = restoreproof_report::write_all(&report, &scenario.report_dir, &formats)?;

    let exit_code = match &result {
        Ok(()) => report.run.status.exit_code(),
        Err(err) => err.exit_code(),
    };

    Ok(DrillOutcome {
        report,
        report_files,
        exit_code,
        kept_environment: progress.kept_environment.clone(),
        kept_workspace: options
            .keep_environment
            .then(|| std::env::temp_dir().join(format!("restoreproof-{run_id}"))),
    })
}

/// The drill itself. Every early return still leaves a usable [`Progress`].
async fn drill(
    scenario: &Scenario,
    options: &RunOptions,
    run_id: &str,
    redactor: &mut Redactor,
    progress: &mut Progress,
) -> Result<()> {
    let secrets = resolve_check_secrets(scenario, redactor, &mut progress.warnings);

    let mut workspace = Workspace::create(run_id, options.keep_environment)?;
    let restore_dir = match (&options.restore_dir, options.mode) {
        (Some(path), RunMode::ChecksOnly) => path.clone(),
        _ => workspace.restore_dir().to_path_buf(),
    };

    let cleanup = options
        .cleanup_override
        .unwrap_or(scenario.raw.recovery.cleanup)
        && !options.keep_environment;

    let compose_environment = compose_environment(scenario, workspace.root(), &restore_dir);
    let mut environment = match options.mode {
        RunMode::Full => {
            DockerCli::ensure_available().await?;

            // --- Inspect and restore ---------------------------------------
            let source = restoreproof_storage::build(
                &scenario.backup,
                StorageLimits {
                    inspect_timeout: Duration::from_secs(120),
                    restore_timeout: Duration::from_secs(
                        scenario.raw.recovery.total_timeout_seconds,
                    ),
                    max_output_bytes: scenario.raw.security.max_command_output_bytes,
                },
                redactor,
            )?;

            let metadata = source.inspect().await?;
            progress.warnings.extend(metadata.notes.iter().cloned());
            progress.metadata = Some(metadata);

            progress.restore_started_at = Some(Utc::now());
            let outcome = source.restore(&restore_dir).await?;
            progress.restore_finished_at = Some(Utc::now());
            progress.restored_bytes = outcome.bytes_restored;

            // --- Start the environment --------------------------------------
            let project = unique_project_name(scenario.compose_project_base(), run_id);
            progress.environment_started_at = Some(Utc::now());
            let environment = RecoveryEnvironment::start(
                scenario.compose_file.clone(),
                project_directory(&scenario.compose_file),
                project,
                compose_environment,
                scenario.raw.security.max_command_output_bytes,
                cleanup,
                Duration::from_secs(scenario.raw.recovery.startup_timeout_seconds),
            )
            .await?;

            environment
                .wait_until_ready(
                    &scenario.raw.recovery.wait_for,
                    Duration::from_secs(scenario.raw.recovery.startup_timeout_seconds),
                )
                .await?;

            environment
        }
        RunMode::ChecksOnly => RecoveryEnvironment::attach(
            scenario.compose_file.clone(),
            project_directory(&scenario.compose_file),
            scenario.compose_project_base().to_owned(),
            compose_environment,
            scenario.raw.security.max_command_output_bytes,
        ),
    };

    // --- Checks ---------------------------------------------------------
    let context = CheckContext {
        project_root: scenario.project_root().to_path_buf(),
        restore_dir,
        compose_project: environment.project_name().to_owned(),
        security: scenario.raw.security.clone(),
        redactor: redactor.clone(),
        secrets,
        probe: Some(std::sync::Arc::new(environment.probe())),
    };

    for spec in &scenario.checks.checks {
        let outcome = run_check(spec, &scenario.checks.defaults, &context).await;
        tracing::info!(
            check = %outcome.id,
            status = %outcome.status,
            duration = outcome.duration_seconds,
            "check finished"
        );
        progress.checks.push(outcome);
    }

    // --- Teardown --------------------------------------------------------
    if options.keep_environment {
        progress.kept_environment = Some(environment.project_name().to_owned());
        progress.warnings.push(format!(
            "--keep-environment: the recovery environment `{}` and the workspace `{}` were left in \
             place. They hold restored data; remove them with `docker compose -p {} down -v`.",
            environment.project_name(),
            workspace.root().display(),
            environment.project_name()
        ));
        workspace.keep(true);
    } else if let Err(err) = environment.teardown().await {
        progress.warnings.push(err.to_string());
    } else {
        progress.cleaned_up = matches!(options.mode, RunMode::Full) && cleanup;
    }

    Ok(())
}

/// Variables passed to `docker compose`, including the injected paths.
fn compose_environment(
    scenario: &Scenario,
    workspace_root: &std::path::Path,
    restore_dir: &std::path::Path,
) -> BTreeMap<String, String> {
    let mut environment = scenario.raw.recovery.environment.clone();
    environment.insert(
        "RESTOREPROOF_RESTORE_DIR".to_owned(),
        restore_dir.display().to_string(),
    );
    environment.insert(
        "RESTOREPROOF_WORKDIR".to_owned(),
        workspace_root.display().to_string(),
    );
    environment
}

/// Read the environment variables the checks declared, once, up front.
///
/// Reading them here means a missing credential is a single clear warning
/// instead of a surprise in the middle of a drill, and every resolved value is
/// registered for redaction before anything can print it.
fn resolve_check_secrets(
    scenario: &Scenario,
    redactor: &mut Redactor,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Secret> {
    let mut wanted: Vec<String> = Vec::new();
    for spec in scenario.enabled_checks() {
        match &spec.kind {
            CheckKind::Sql(sql) => wanted.push(sql.dsn_env.clone()),
            CheckKind::Http(http) => wanted.extend(http.headers_from_env.values().cloned()),
            _ => {}
        }
    }
    wanted.sort_unstable();
    wanted.dedup();

    let mut secrets = BTreeMap::new();
    for variable in wanted {
        match std::env::var(&variable) {
            Ok(value) if !value.is_empty() => {
                redactor.add_secret(&value);
                secrets.insert(variable, Secret::new(value));
            }
            _ => warnings.push(format!(
                "the environment variable `{variable}` is not set; the checks that need it will \
                 report ERROR"
            )),
        }
    }
    secrets
}

/// Build the report from whatever the drill managed to do.
#[allow(clippy::too_many_arguments)]
fn assemble(
    scenario: &Scenario,
    options: &RunOptions,
    tool: &ToolInfo,
    run_id: &str,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    clock: Instant,
    progress: &Progress,
) -> Report {
    let ready_at = ready_timestamp(&progress.checks);
    let run_status = if options.dry_run {
        RunStatus::Skipped
    } else if progress.checks.is_empty() {
        RunStatus::Error
    } else {
        status::aggregate(
            progress
                .checks
                .iter()
                .map(|check| (check.required, check.status)),
        )
    };

    let measured_rto = ready_at.map(|ready| {
        (ready - started_at)
            .to_std()
            .unwrap_or_else(|_| Duration::from_secs(0))
    });

    let reference = load_rpo_reference(scenario);
    let backup_timestamp = reference
        .backup_timestamp
        .or_else(|| progress.metadata.as_ref().and_then(|meta| meta.created_at));
    let reference_timestamp = reference.reference_timestamp.unwrap_or(started_at);

    let restore_duration = match (progress.restore_started_at, progress.restore_finished_at) {
        (Some(start), Some(end)) => (end - start)
            .to_std()
            .ok()
            .map(|duration| round_ms(duration.as_secs_f64())),
        _ => None,
    };

    Report {
        schema_version: REPORT_SCHEMA_VERSION,
        tool: tool.clone(),
        run: RunInfo {
            id: run_id.to_owned(),
            project: scenario.raw.project.name.clone(),
            description: scenario.raw.project.description.clone(),
            started_at,
            finished_at,
            duration_seconds: round_ms(clock.elapsed().as_secs_f64()),
            status: run_status,
        },
        backup: progress.metadata.as_ref().map_or_else(
            || BackupInfo {
                source_type: scenario.backup.kind().to_owned(),
                location: scenario.backup.location(),
                snapshot_id: None,
                created_at: None,
                size_bytes: None,
            },
            |meta| BackupInfo {
                source_type: meta.source_type.clone(),
                location: meta.location.clone(),
                snapshot_id: meta.snapshot_id.clone(),
                created_at: meta.created_at,
                size_bytes: meta.size_bytes.or(progress.restored_bytes),
            },
        ),
        recovery: RecoveryTimeline {
            restore_started_at: progress.restore_started_at,
            restore_finished_at: progress.restore_finished_at,
            restore_duration_seconds: restore_duration,
            environment_started_at: progress.environment_started_at,
            ready_at,
            environment_cleaned_up: progress.cleaned_up,
        },
        objectives: Objectives {
            rto: RtoMetric::evaluate(measured_rto, scenario.raw.metrics.target_rto_seconds),
            rpo: RpoMetric::evaluate(
                backup_timestamp,
                reference_timestamp,
                scenario.raw.metrics.target_rpo_seconds,
            ),
        },
        summary: CheckSummary::from_outcomes(&progress.checks),
        checks: progress.checks.clone(),
        errors: progress.errors.clone(),
        warnings: progress.warnings.clone(),
        config: ConfigInfo {
            path: scenario.config_path.display().to_string(),
            version: scenario.raw.version,
            application_version: scenario.raw.project.application_version.clone(),
            config_sha256: scenario.config_sha256.clone(),
        },
        integrity: Integrity::default(),
    }
}

/// Instant at which the last required check passed, if they all did.
fn ready_timestamp(checks: &[CheckOutcome]) -> Option<DateTime<Utc>> {
    let required: Vec<&CheckOutcome> = checks.iter().filter(|check| check.required).collect();
    if required.is_empty() || !required.iter().all(|check| check.status.is_passed()) {
        return None;
    }
    required
        .iter()
        .filter_map(|check| {
            chrono::Duration::try_milliseconds((check.duration_seconds * 1000.0).round() as i64)
                .map(|duration| check.started_at + duration)
        })
        .max()
}

fn load_rpo_reference(scenario: &Scenario) -> RpoReference {
    let Some(path) = &scenario.rpo_reference_file else {
        return RpoReference::default();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "the RPO reference file could not be parsed; falling back to backup metadata"
            );
            RpoReference::default()
        }),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "cannot read the RPO reference file");
            RpoReference::default()
        }
    }
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use restoreproof_core::CheckStatus;

    fn outcome(id: &str, required: bool, status: CheckStatus, offset_seconds: i64) -> CheckOutcome {
        CheckOutcome {
            id: id.to_owned(),
            name: id.to_owned(),
            description: None,
            kind: "http".to_owned(),
            required,
            status,
            started_at: Utc::now() + chrono::Duration::seconds(offset_seconds),
            duration_seconds: 1.0,
            message: String::new(),
            details: Vec::new(),
            attempts: 1,
        }
    }

    #[test]
    fn the_ready_instant_is_the_end_of_the_last_required_check() {
        let checks = vec![
            outcome("a", true, CheckStatus::Passed, 0),
            outcome("b", true, CheckStatus::Passed, 10),
            outcome("c", false, CheckStatus::Failed, 20),
        ];
        let ready = ready_timestamp(&checks).unwrap();
        let expected = checks[1].started_at + chrono::Duration::seconds(1);
        assert_eq!(ready, expected);
    }

    #[test]
    fn there_is_no_ready_instant_when_a_required_check_failed() {
        let checks = vec![
            outcome("a", true, CheckStatus::Passed, 0),
            outcome("b", true, CheckStatus::Failed, 10),
        ];
        assert!(ready_timestamp(&checks).is_none());
    }

    #[test]
    fn there_is_no_ready_instant_without_required_checks() {
        let checks = vec![outcome("a", false, CheckStatus::Passed, 0)];
        assert!(ready_timestamp(&checks).is_none());
    }

    #[test]
    fn the_rpo_reference_file_rejects_unknown_fields() {
        let parsed: std::result::Result<RpoReference, _> =
            serde_json::from_str(r#"{"backup_timestamp":"2026-01-15T08:00:00Z"}"#);
        assert!(parsed.is_ok());
        let bad: std::result::Result<RpoReference, _> =
            serde_json::from_str(r#"{"typo_timestamp":"2026-01-15T08:00:00Z"}"#);
        assert!(bad.is_err());
    }
}
