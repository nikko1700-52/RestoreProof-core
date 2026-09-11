//! `restoreproof plan`: what a drill *would* do, without doing any of it.
//!
//! The plan is also a security artefact: it is where an operator sees which
//! paths will be read, which programs will run, which endpoints will be probed
//! and which safety switches are off — before a single container starts. It
//! never contains a secret value, only the *name* of the place a secret is read
//! from.

use std::path::PathBuf;

use restoreproof_config::Scenario;
use restoreproof_config::checks::CheckKind;
use restoreproof_config::scenario::{ResolvedBackup, SecretSource};
use serde::Serialize;

/// Everything a drill would do.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// Project name.
    pub project: String,
    /// Project description.
    pub description: Option<String>,
    /// Configuration file, as provided.
    pub config_path: PathBuf,
    /// SHA-256 of the configuration file.
    pub config_sha256: String,
    /// Backup that would be restored.
    pub backup: PlanBackup,
    /// Environment that would be started.
    pub recovery: PlanRecovery,
    /// Objectives that would be measured.
    pub objectives: PlanObjectives,
    /// Checks that would run.
    pub checks: Vec<PlanCheck>,
    /// Reports that would be written.
    pub report: PlanReport,
    /// Effective safety switches.
    pub security: PlanSecurity,
    /// Observations from validation.
    pub warnings: Vec<String>,
}

/// Backup that would be restored.
#[derive(Debug, Clone, Serialize)]
pub struct PlanBackup {
    /// `local`, `restic` or `borg`.
    pub kind: String,
    /// Repository location.
    pub location: String,
    /// Snapshot or archive selector.
    pub selector: Option<String>,
    /// Where the credential comes from — never the credential itself.
    pub credential: String,
    /// External tool required.
    pub required_tool: Option<String>,
}

/// Environment that would be started.
#[derive(Debug, Clone, Serialize)]
pub struct PlanRecovery {
    /// Compose file.
    pub compose_file: PathBuf,
    /// Compose project name pattern.
    pub project_name: String,
    /// Services declared in the Compose file.
    pub services: Vec<String>,
    /// Services waited for before checks start.
    pub wait_for: Vec<String>,
    /// Startup timeout, in seconds.
    pub startup_timeout_seconds: u64,
    /// Whole-drill timeout, in seconds.
    pub total_timeout_seconds: u64,
    /// Whether the environment is destroyed afterwards.
    pub cleanup: bool,
    /// Whether external networks are refused.
    pub network_isolated: bool,
    /// Names of the extra environment variables passed to Compose.
    pub environment_keys: Vec<String>,
}

/// Objectives that would be measured.
#[derive(Debug, Clone, Serialize)]
pub struct PlanObjectives {
    /// Target RTO, in seconds.
    pub target_rto_seconds: Option<u64>,
    /// Target RPO, in seconds.
    pub target_rpo_seconds: Option<u64>,
    /// Optional reference file for the RPO computation.
    pub rpo_reference_file: Option<PathBuf>,
}

/// One check that would run.
#[derive(Debug, Clone, Serialize)]
pub struct PlanCheck {
    /// Check id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Check type.
    pub kind: String,
    /// Whether the drill fails when it does not pass.
    pub required: bool,
    /// Whether it runs at all.
    pub enabled: bool,
    /// Effective timeout, in seconds.
    pub timeout_seconds: u64,
    /// Effective number of attempts.
    pub attempts: u32,
    /// One-line description of what it does.
    pub summary: String,
}

/// Reports that would be written.
#[derive(Debug, Clone, Serialize)]
pub struct PlanReport {
    /// Output directory.
    pub directory: PathBuf,
    /// Formats.
    pub formats: Vec<String>,
}

/// Effective safety switches.
#[derive(Debug, Clone, Serialize)]
pub struct PlanSecurity {
    /// Whether command and script checks may run.
    pub allow_command_checks: bool,
    /// Whether HTTP checks may leave loopback.
    pub allow_external_http_targets: bool,
    /// Whether SQL checks may leave loopback.
    pub allow_external_sql_targets: bool,
    /// Whether the Compose file may publish ports.
    pub allow_published_ports: bool,
    /// Locations outside the project that configuration paths may use.
    pub allow_external_paths: Vec<PathBuf>,
    /// Ceiling for captured command output.
    pub max_command_output_bytes: usize,
}

impl Plan {
    /// Build a plan from a validated scenario.
    #[must_use]
    pub fn from_scenario(scenario: &Scenario) -> Self {
        let defaults = &scenario.checks.defaults;
        let checks = scenario
            .checks
            .checks
            .iter()
            .map(|spec| PlanCheck {
                id: spec.id.clone(),
                name: spec.display_name().to_owned(),
                kind: spec.kind.kind().to_owned(),
                required: spec.required,
                enabled: spec.enabled,
                timeout_seconds: spec.timeout(defaults),
                attempts: spec.retry_policy(defaults).attempts,
                summary: summarize(&spec.kind),
            })
            .collect();

        Self {
            project: scenario.raw.project.name.clone(),
            description: scenario.raw.project.description.clone(),
            config_path: scenario.config_path.clone(),
            config_sha256: scenario.config_sha256.clone(),
            backup: plan_backup(&scenario.backup),
            recovery: PlanRecovery {
                compose_file: scenario.compose_file.clone(),
                project_name: format!("{}-rp-<run id>", scenario.compose_project_base()),
                services: scenario.compose_audit.services.clone(),
                wait_for: if scenario.raw.recovery.wait_for.is_empty() {
                    scenario.compose_audit.services.clone()
                } else {
                    scenario.raw.recovery.wait_for.clone()
                },
                startup_timeout_seconds: scenario.raw.recovery.startup_timeout_seconds,
                total_timeout_seconds: scenario.raw.recovery.total_timeout_seconds,
                cleanup: scenario.raw.recovery.cleanup,
                network_isolated: scenario.raw.recovery.network_isolated,
                environment_keys: scenario.raw.recovery.environment.keys().cloned().collect(),
            },
            objectives: PlanObjectives {
                target_rto_seconds: scenario.raw.metrics.target_rto_seconds,
                target_rpo_seconds: scenario.raw.metrics.target_rpo_seconds,
                rpo_reference_file: scenario.rpo_reference_file.clone(),
            },
            checks,
            report: PlanReport {
                directory: scenario.report_dir.clone(),
                formats: scenario
                    .formats()
                    .iter()
                    .map(|format| format.extension().to_owned())
                    .collect(),
            },
            security: PlanSecurity {
                allow_command_checks: scenario.raw.security.allow_command_checks,
                allow_external_http_targets: scenario.raw.security.allow_external_http_targets,
                allow_external_sql_targets: scenario.raw.security.allow_external_sql_targets,
                allow_published_ports: scenario.raw.security.allow_published_ports,
                allow_external_paths: scenario.raw.security.allow_external_paths.clone(),
                max_command_output_bytes: scenario.raw.security.max_command_output_bytes,
            },
            warnings: scenario.warnings.clone(),
        }
    }

    /// Render the plan for a terminal.
    #[must_use]
    pub fn render_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::with_capacity(2048);

        let _ = writeln!(out, "\nPlan for `{}`", self.project);
        if let Some(description) = &self.description {
            let _ = writeln!(out, "  {description}");
        }
        let _ = writeln!(out, "\n  Nothing below has been executed.\n");

        let _ = writeln!(out, "  Backup");
        let _ = writeln!(out, "    type        {}", self.backup.kind);
        let _ = writeln!(out, "    location    {}", self.backup.location);
        if let Some(selector) = &self.backup.selector {
            let _ = writeln!(out, "    snapshot    {selector}");
        }
        let _ = writeln!(out, "    credential  {}", self.backup.credential);
        if let Some(tool) = &self.backup.required_tool {
            let _ = writeln!(out, "    requires    {tool}");
        }

        let _ = writeln!(out, "\n  Recovery environment");
        let _ = writeln!(
            out,
            "    compose     {}",
            self.recovery.compose_file.display()
        );
        let _ = writeln!(out, "    project     {}", self.recovery.project_name);
        let _ = writeln!(
            out,
            "    services    {}",
            join_or_dash(&self.recovery.services)
        );
        let _ = writeln!(
            out,
            "    wait for    {}",
            join_or_dash(&self.recovery.wait_for)
        );
        let _ = writeln!(
            out,
            "    timeouts    {}s to start, {}s total",
            self.recovery.startup_timeout_seconds, self.recovery.total_timeout_seconds
        );
        let _ = writeln!(
            out,
            "    cleanup     {}",
            if self.recovery.cleanup {
                "containers, networks and volumes are destroyed afterwards"
            } else {
                "DISABLED — the environment will be left running"
            }
        );
        if !self.recovery.environment_keys.is_empty() {
            let _ = writeln!(
                out,
                "    env         {} (values hidden)",
                self.recovery.environment_keys.join(", ")
            );
        }

        let _ = writeln!(out, "\n  Checks ({} total)", self.checks.len());
        for check in &self.checks {
            let _ = writeln!(
                out,
                "    {} {:<22} {:<9} {:<8} {}",
                if check.enabled { "•" } else { "·" },
                truncate(&check.id, 22),
                check.kind,
                if check.required {
                    "required"
                } else {
                    "optional"
                },
                check.summary
            );
        }

        let _ = writeln!(out, "\n  Objectives");
        let _ = writeln!(
            out,
            "    RTO target  {}",
            self.objectives.target_rto_seconds.map_or_else(
                || "not set (report will show RTO_UNKNOWN)".to_owned(),
                |v| format!("{v}s")
            )
        );
        let _ = writeln!(
            out,
            "    RPO target  {}",
            self.objectives.target_rpo_seconds.map_or_else(
                || "not set (report will show RPO_UNKNOWN)".to_owned(),
                |v| format!("{v}s")
            )
        );

        let _ = writeln!(out, "\n  Reports");
        let _ = writeln!(
            out,
            "    {} ({})",
            self.report.directory.display(),
            self.report.formats.join(", ")
        );

        let _ = writeln!(out, "\n  Safety");
        let _ = writeln!(
            out,
            "    command/script checks   {}",
            enabled_label(self.security.allow_command_checks)
        );
        let _ = writeln!(
            out,
            "    non-loopback HTTP       {}",
            enabled_label(self.security.allow_external_http_targets)
        );
        let _ = writeln!(
            out,
            "    non-loopback SQL        {}",
            enabled_label(self.security.allow_external_sql_targets)
        );
        let _ = writeln!(
            out,
            "    published ports         {}",
            if self.security.allow_published_ports {
                "ALLOWED on any interface"
            } else {
                "loopback only"
            }
        );
        if !self.security.allow_external_paths.is_empty() {
            let _ = writeln!(
                out,
                "    paths outside project   {}",
                self.security
                    .allow_external_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        if !self.warnings.is_empty() {
            let _ = writeln!(out, "\n  Warnings");
            for warning in &self.warnings {
                let _ = writeln!(out, "    ! {warning}");
            }
        }

        let _ = writeln!(out);
        out
    }
}

fn plan_backup(backup: &ResolvedBackup) -> PlanBackup {
    let (selector, credential) = match backup {
        ResolvedBackup::Local { metadata_file, .. } => (
            None,
            metadata_file.as_ref().map_or_else(
                || "none required".to_owned(),
                |path| format!("none required (metadata from {})", path.display()),
            ),
        ),
        ResolvedBackup::Restic { snapshot, .. } => {
            (Some(snapshot.clone()), describe_secret(backup))
        }
        ResolvedBackup::Borg { archive, .. } => (Some(archive.clone()), describe_secret(backup)),
    };

    PlanBackup {
        kind: backup.kind().to_owned(),
        location: backup.location(),
        selector,
        credential,
        required_tool: backup.required_tool().map(str::to_owned),
    }
}

fn describe_secret(backup: &ResolvedBackup) -> String {
    match backup.secret_source() {
        Some(SecretSource::File(path)) => format!("read from the file {}", path.display()),
        Some(SecretSource::Env(variable)) => {
            format!("read from the environment variable {variable}")
        }
        None => "not configured; the backup tool's own environment will be used".to_owned(),
    }
}

fn summarize(kind: &CheckKind) -> String {
    match kind {
        CheckKind::Http(http) => format!(
            "{} {} expecting HTTP {}",
            http.method.as_str(),
            http.url,
            http.expected_status
        ),
        CheckKind::Command(command) => format!(
            "run {} expecting exit code {}",
            command
                .command
                .first()
                .map_or_else(|| "<nothing>".to_owned(), |program| format!("`{program}`")),
            command.expected_exit_code
        ),
        CheckKind::Sql(sql) => format!("query the database from ${} (read-only)", sql.dsn_env),
        CheckKind::File(file) => format!(
            "{} `{}` in the {:?} directory",
            if file.exists { "expect" } else { "expect no" },
            file.path.display(),
            file.base
        )
        .to_lowercase(),
        CheckKind::Container(container) => format!(
            "expect service `{}` to be {:?}",
            container.service, container.state
        )
        .to_lowercase(),
        CheckKind::Script(script) => format!(
            "run `{}` expecting exit code {}",
            script.path.display(),
            script.expected_exit_code
        ),
    }
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        "—".to_owned()
    } else {
        values.join(", ")
    }
}

const fn enabled_label(value: bool) -> &'static str {
    if value { "ALLOWED" } else { "refused" }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    value.chars().take(width).collect()
}
