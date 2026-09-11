//! Loading, validating and resolving a scenario.
//!
//! [`Scenario`] is the only thing the runner ever sees. By the time one exists:
//!
//! * every path has been resolved and confined ([`crate::paths`]);
//! * every value handed to an external tool has been checked
//!   ([`crate::argsafe`]);
//! * every SQL statement has been proven read-only ([`crate::sql_guard`]);
//! * every URL has been proven to target the recovery environment
//!   ([`crate::http_guard`]);
//! * the Compose file has been audited ([`crate::compose_audit`]).
//!
//! Validation collects *every* problem before failing, so that a user fixes a
//! configuration in one pass rather than one error at a time.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use restoreproof_core::report::sha256_hex;

use crate::argsafe::{validate_cli_token, validate_env_entry, validate_identifier};
use crate::checks::{CheckKind, CheckSpec, ChecksDocument, FileBase};
use crate::compose_audit::{ComposeAudit, audit};
use crate::error::{ConfigError, Result, ValidationIssue};
use crate::http_guard::{validate_env_header, validate_literal_header, validate_url};
use crate::model::{BackupSpec, RawConfig, ReportFormat};
use crate::paths::{Confinement, PathPolicy, has_parent_component};
use crate::sql_guard::ensure_read_only;

/// Where a secret is read from at run time.
///
/// A secret is never stored in the configuration file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// Read from a file, which must be readable only by its owner.
    File(PathBuf),
    /// Read from an environment variable of the `restoreproof` process.
    Env(String),
}

/// A backup repository, split by whether it involves the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryLocation {
    /// A path on the local filesystem, already confined.
    Local(PathBuf),
    /// A remote location handled by the backup tool itself.
    Remote(String),
}

impl RepositoryLocation {
    /// Value handed to the backup tool.
    #[must_use]
    pub fn as_argument(&self) -> String {
        match self {
            Self::Local(path) => path.display().to_string(),
            Self::Remote(raw) => raw.clone(),
        }
    }
}

/// A backup source with every path resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedBackup {
    /// A local directory holding a previous dump.
    Local {
        /// Directory or file to restore from.
        path: PathBuf,
        /// Optional metadata file carrying the creation timestamp.
        metadata_file: Option<PathBuf>,
    },
    /// A restic repository.
    Restic {
        /// Repository location.
        repository: RepositoryLocation,
        /// Where the repository password is read from.
        password: Option<SecretSource>,
        /// Snapshot to restore.
        snapshot: String,
        /// Optional path filter inside the snapshot.
        include_path: Option<String>,
    },
    /// A `BorgBackup` repository.
    Borg {
        /// Repository location.
        repository: RepositoryLocation,
        /// Where the repository passphrase is read from.
        passphrase: Option<SecretSource>,
        /// Archive to extract.
        archive: String,
    },
}

impl ResolvedBackup {
    /// Discriminator as written in YAML.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::Restic { .. } => "restic",
            Self::Borg { .. } => "borg",
        }
    }

    /// Repository location as shown in plans and reports.
    #[must_use]
    pub fn location(&self) -> String {
        match self {
            Self::Local { path, .. } => path.display().to_string(),
            Self::Restic { repository, .. } | Self::Borg { repository, .. } => {
                repository.as_argument()
            }
        }
    }

    /// Where the secret for this backup comes from, if any.
    #[must_use]
    pub const fn secret_source(&self) -> Option<&SecretSource> {
        match self {
            Self::Local { .. } => None,
            Self::Restic { password, .. } => password.as_ref(),
            Self::Borg { passphrase, .. } => passphrase.as_ref(),
        }
    }

    /// External tool this backup source needs.
    #[must_use]
    pub const fn required_tool(&self) -> Option<&'static str> {
        match self {
            Self::Local { .. } => None,
            Self::Restic { .. } => Some("restic"),
            Self::Borg { .. } => Some("borg"),
        }
    }
}

/// A validated, fully resolved scenario.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Configuration file path, as the user typed it.
    pub config_path: PathBuf,
    /// SHA-256 of the configuration file, recorded in reports.
    pub config_sha256: String,
    /// The document as written.
    pub raw: RawConfig,
    /// Path confinement policy for this project.
    pub policy: PathPolicy,
    /// Absolute Compose file path.
    pub compose_file: PathBuf,
    /// Absolute checks file path.
    pub checks_file: PathBuf,
    /// The parsed checks.
    pub checks: ChecksDocument,
    /// Absolute report directory, created on demand.
    pub report_dir: PathBuf,
    /// Absolute path of the RPO reference file, when configured.
    pub rpo_reference_file: Option<PathBuf>,
    /// Resolved backup source.
    pub backup: ResolvedBackup,
    /// Result of the Compose audit.
    pub compose_audit: ComposeAudit,
    /// Non-blocking observations gathered while loading.
    pub warnings: Vec<String>,
}

impl Scenario {
    /// Load, validate and resolve a scenario.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] describing every problem found. All problems
    /// are collected before returning.
    pub fn load(config_path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(config_path)
            .map_err(|source| ConfigError::io(config_path, source))?;
        Self::from_text(config_path, &text)
    }

    /// Same as [`Scenario::load`] but with the document already in memory.
    ///
    /// # Errors
    ///
    /// See [`Scenario::load`].
    pub fn from_text(config_path: &Path, text: &str) -> Result<Self> {
        let raw = RawConfig::from_yaml(config_path, text)?;
        let config_sha256 = sha256_hex(text.as_bytes());

        let project_root = config_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let policy = PathPolicy::new(&project_root, &raw.security.allow_external_paths)?;

        let mut issues: Vec<ValidationIssue> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        check(
            &mut issues,
            "project.name",
            validate_identifier("project.name", &raw.project.name),
        );
        if let Some(name) = &raw.recovery.project_name {
            check(
                &mut issues,
                "recovery.project_name",
                validate_identifier("recovery.project_name", name),
            );
        }

        validate_timeouts(&raw, &mut issues);

        for (key, value) in &raw.recovery.environment {
            check(
                &mut issues,
                "recovery.environment",
                validate_env_entry("recovery.environment", key, value),
            );
        }

        if raw.report.formats.is_empty() {
            issues.push(ValidationIssue::new(
                "report.formats",
                "declare at least one format among: json, markdown",
            ));
        }
        let mut seen_formats = BTreeSet::new();
        for format in &raw.report.formats {
            if !seen_formats.insert(*format) {
                issues.push(ValidationIssue::new(
                    "report.formats",
                    format!("`{}` is listed twice", format.extension()),
                ));
            }
        }

        if raw.security.max_command_output_bytes == 0 {
            issues.push(ValidationIssue::new(
                "security.max_command_output_bytes",
                "must be greater than zero",
            ));
        }

        // --- Compose file -------------------------------------------------
        let compose_file = take(
            &mut issues,
            policy.resolve_existing(
                "recovery.compose_file",
                &raw.recovery.compose_file,
                Confinement::ProjectOnly,
            ),
        );
        let mut compose_audit = ComposeAudit::default();
        if let Some(compose_file) = &compose_file {
            match std::fs::read_to_string(compose_file) {
                Ok(compose_text) => {
                    match audit(
                        compose_file,
                        &compose_text,
                        &policy,
                        &raw.security,
                        raw.recovery.network_isolated,
                    ) {
                        Ok(result) => {
                            for error in &result.errors {
                                issues.push(ValidationIssue::new("recovery.compose_file", error));
                            }
                            warnings.extend(result.warnings.iter().cloned());
                            compose_audit = result;
                        }
                        Err(err) => {
                            issues.push(ValidationIssue::new(
                                "recovery.compose_file",
                                err.to_string(),
                            ));
                        }
                    }
                }
                Err(err) => issues.push(ValidationIssue::new(
                    "recovery.compose_file",
                    format!("cannot read the Compose file: {err}"),
                )),
            }
        }

        for service in &raw.recovery.wait_for {
            if !compose_audit.services.is_empty() && !compose_audit.services.contains(service) {
                issues.push(ValidationIssue::new(
                    "recovery.wait_for",
                    format!(
                        "service `{service}` is not defined in the Compose file (defined: {})",
                        compose_audit.services.join(", ")
                    ),
                ));
            }
        }

        // --- Checks -------------------------------------------------------
        let checks_file = take(
            &mut issues,
            policy.resolve_existing("checks_file", &raw.checks_file, Confinement::ProjectOnly),
        );
        let mut checks = ChecksDocument {
            version: crate::checks::CHECKS_SUPPORTED_VERSION,
            defaults: crate::checks::CheckDefaults::default(),
            checks: Vec::new(),
        };
        if let Some(checks_file) = &checks_file {
            match std::fs::read_to_string(checks_file) {
                Ok(checks_text) => match ChecksDocument::from_yaml(checks_file, &checks_text) {
                    Ok(document) => {
                        validate_checks(
                            &document,
                            &raw,
                            &policy,
                            &compose_audit,
                            &mut issues,
                            &mut warnings,
                        );
                        checks = document;
                    }
                    Err(err) => issues.push(ValidationIssue::new("checks_file", err.to_string())),
                },
                Err(err) => issues.push(ValidationIssue::new(
                    "checks_file",
                    format!("cannot read the checks file: {err}"),
                )),
            }
        }

        // --- Reports and metrics -------------------------------------------
        let report_dir = take(
            &mut issues,
            policy.resolve_for_creation(
                "report.directory",
                &raw.report.directory,
                Confinement::ProjectOrAllowlisted,
            ),
        );

        let rpo_reference_file = match &raw.metrics.rpo_reference_file {
            Some(path) => take(
                &mut issues,
                policy.resolve_existing(
                    "metrics.rpo_reference_file",
                    path,
                    Confinement::ProjectOrAllowlisted,
                ),
            ),
            None => None,
        };

        if raw.metrics.target_rto_seconds.is_none() {
            warnings.push(
                "metrics.target_rto_seconds is not set: the report will show RTO_UNKNOWN instead of \
                 a pass/fail verdict"
                    .to_owned(),
            );
        }
        if raw.metrics.target_rpo_seconds.is_none() {
            warnings.push(
                "metrics.target_rpo_seconds is not set: the report will show RPO_UNKNOWN instead of \
                 a pass/fail verdict"
                    .to_owned(),
            );
        }

        // --- Backup --------------------------------------------------------
        let backup = resolve_backup(&raw.backup, &policy, &mut issues, &mut warnings);

        if !issues.is_empty() {
            return Err(ConfigError::Invalid { issues });
        }

        let (Some(compose_file), Some(checks_file), Some(report_dir), Some(backup)) =
            (compose_file, checks_file, report_dir, backup)
        else {
            return Err(ConfigError::Invalid {
                issues: vec![ValidationIssue::new(
                    "configuration",
                    "the scenario could not be resolved",
                )],
            });
        };

        Ok(Self {
            config_path: config_path.to_path_buf(),
            config_sha256,
            raw,
            policy,
            compose_file,
            checks_file,
            checks,
            report_dir,
            rpo_reference_file,
            backup,
            compose_audit,
            warnings,
        })
    }

    /// Absolute project root.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        self.policy.project_root()
    }

    /// Base name used for the Compose project.
    #[must_use]
    pub fn compose_project_base(&self) -> &str {
        self.raw
            .recovery
            .project_name
            .as_deref()
            .unwrap_or(&self.raw.project.name)
    }

    /// Checks that will actually run, in order.
    #[must_use]
    pub fn enabled_checks(&self) -> Vec<&CheckSpec> {
        self.checks
            .checks
            .iter()
            .filter(|check| check.enabled)
            .collect()
    }

    /// Report formats requested.
    #[must_use]
    pub fn formats(&self) -> &[ReportFormat] {
        &self.raw.report.formats
    }
}

fn validate_timeouts(raw: &RawConfig, issues: &mut Vec<ValidationIssue>) {
    const MAX_TIMEOUT: u64 = 24 * 3600;

    for (field, value) in [
        (
            "recovery.startup_timeout_seconds",
            raw.recovery.startup_timeout_seconds,
        ),
        (
            "recovery.total_timeout_seconds",
            raw.recovery.total_timeout_seconds,
        ),
    ] {
        if value == 0 {
            issues.push(ValidationIssue::new(field, "must be greater than zero"));
        } else if value > MAX_TIMEOUT {
            issues.push(ValidationIssue::new(
                field,
                format!("must not exceed {MAX_TIMEOUT} seconds (24 hours)"),
            ));
        }
    }

    if raw.recovery.total_timeout_seconds < raw.recovery.startup_timeout_seconds {
        issues.push(ValidationIssue::new(
            "recovery.total_timeout_seconds",
            "must be greater than or equal to recovery.startup_timeout_seconds",
        ));
    }
}

fn validate_checks(
    document: &ChecksDocument,
    raw: &RawConfig,
    policy: &PathPolicy,
    compose: &ComposeAudit,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) {
    if document.checks.is_empty() {
        issues.push(ValidationIssue::new(
            "checks",
            "declare at least one check: a drill without checks proves nothing",
        ));
        return;
    }

    if document.defaults.timeout_seconds == 0 {
        issues.push(ValidationIssue::new(
            "defaults.timeout_seconds",
            "must be greater than zero",
        ));
    }
    if document.defaults.retry.attempts == 0 {
        issues.push(ValidationIssue::new(
            "defaults.retry.attempts",
            "must be at least 1",
        ));
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut enabled_required = 0usize;

    for spec in &document.checks {
        let field = format!("checks[{}]", spec.id);
        check(issues, &field, validate_identifier(&field, &spec.id));
        if !seen.insert(spec.id.as_str()) {
            issues.push(ValidationIssue::new(&field, "duplicate check id"));
        }
        if let Some(timeout) = spec.timeout_seconds
            && timeout == 0
        {
            issues.push(ValidationIssue::new(
                format!("{field}.timeout_seconds"),
                "must be greater than zero",
            ));
        }
        if let Some(retry) = spec.retry
            && retry.attempts == 0
        {
            issues.push(ValidationIssue::new(
                format!("{field}.retry.attempts"),
                "must be at least 1",
            ));
        }
        if spec.enabled && spec.required {
            enabled_required += 1;
        }
        if spec.enabled {
            validate_check_kind(spec, &field, raw, policy, compose, issues, warnings);
        }
    }

    if enabled_required == 0 {
        warnings.push(
            "no enabled check is marked `required: true`: the drill can only ever report PARTIAL or \
             PASSED, never a meaningful failure"
                .to_owned(),
        );
    }
}

#[allow(clippy::too_many_lines)]
fn validate_check_kind(
    spec: &CheckSpec,
    field: &str,
    raw: &RawConfig,
    policy: &PathPolicy,
    compose: &ComposeAudit,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) {
    if spec.kind.executes_local_program() && !raw.security.allow_command_checks {
        issues.push(ValidationIssue::new(
            field,
            "command and script checks are disabled by `security.allow_command_checks: false`",
        ));
        return;
    }

    match &spec.kind {
        CheckKind::Http(http) => {
            check(
                issues,
                field,
                validate_url(
                    &format!("{field}.url"),
                    &http.url,
                    raw.security.allow_external_http_targets,
                )
                .map(|_| ()),
            );
            if !(100..=599).contains(&http.expected_status) {
                issues.push(ValidationIssue::new(
                    format!("{field}.expected_status"),
                    "must be a valid HTTP status code between 100 and 599",
                ));
            }
            for (name, value) in &http.headers {
                check(
                    issues,
                    field,
                    validate_literal_header(&format!("{field}.headers"), name, value),
                );
            }
            for (name, variable) in &http.headers_from_env {
                check(
                    issues,
                    field,
                    validate_env_header(&format!("{field}.headers_from_env"), name, variable),
                );
            }
            if http.body.is_some() && http.method != crate::checks::HttpMethod::Post {
                issues.push(ValidationIssue::new(
                    format!("{field}.body"),
                    "a body may only be sent with `method: POST`",
                ));
            }
            if http.follow_redirects {
                warnings.push(format!(
                    "{field}: redirects are followed; every hop is re-validated, but the check no \
                     longer proves which service answered"
                ));
            }
        }
        CheckKind::Command(command) => {
            if command.command.is_empty() {
                issues.push(ValidationIssue::new(
                    format!("{field}.command"),
                    "must contain at least the program to run, as a list: [\"psql\", \"--version\"]",
                ));
            } else if let Some(program) = command.command.first() {
                check(
                    issues,
                    field,
                    validate_cli_token(&format!("{field}.command[0]"), program),
                );
                if program.contains('/') {
                    check(
                        issues,
                        field,
                        policy
                            .resolve_existing(
                                &format!("{field}.command[0]"),
                                Path::new(program),
                                Confinement::ProjectOnly,
                            )
                            .map(|_| ()),
                    );
                }
            }
            for (index, argument) in command.command.iter().enumerate().skip(1) {
                if argument.contains('\0') {
                    issues.push(ValidationIssue::new(
                        format!("{field}.command[{index}]"),
                        "contains a NUL byte",
                    ));
                }
            }
            if let Some(workdir) = &command.workdir {
                check(
                    issues,
                    field,
                    policy
                        .resolve_existing(
                            &format!("{field}.workdir"),
                            workdir,
                            Confinement::ProjectOnly,
                        )
                        .map(|_| ()),
                );
            }
            for (key, value) in &command.environment {
                check(
                    issues,
                    field,
                    validate_env_entry(&format!("{field}.environment"), key, value),
                );
            }
        }
        CheckKind::Sql(sql) => {
            check(
                issues,
                field,
                validate_env_header(&format!("{field}.dsn_env"), "dsn", &sql.dsn_env),
            );
            check(
                issues,
                field,
                ensure_read_only(&format!("{field}.query"), &sql.query),
            );
            if let (Some(min), Some(max)) = (sql.min_rows, sql.max_rows)
                && min > max
            {
                issues.push(ValidationIssue::new(
                    format!("{field}.min_rows"),
                    "min_rows is greater than max_rows",
                ));
            }
            if let (Some(min), Some(max)) = (sql.min_value, sql.max_value)
                && min > max
            {
                issues.push(ValidationIssue::new(
                    format!("{field}.min_value"),
                    "min_value is greater than max_value",
                ));
            }
            if sql.expect_row_count.is_none()
                && sql.min_rows.is_none()
                && sql.max_rows.is_none()
                && sql.expect_value.is_none()
                && sql.min_value.is_none()
                && sql.max_value.is_none()
            {
                issues.push(ValidationIssue::new(
                    field,
                    "declare at least one assertion (expect_row_count, min_rows, max_rows, \
                     expect_value, min_value or max_value): a query without an assertion proves \
                     nothing",
                ));
            }
        }
        CheckKind::File(file) => {
            match file.base {
                FileBase::Restore => {
                    if file.path.is_absolute() || has_parent_component(&file.path) {
                        issues.push(ValidationIssue::new(
                            format!("{field}.path"),
                            "must be a relative path without `..` when `base: restore`",
                        ));
                    }
                }
                FileBase::Project => {
                    check(
                        issues,
                        field,
                        policy
                            .resolve_for_creation(
                                &format!("{field}.path"),
                                &file.path,
                                Confinement::ProjectOnly,
                            )
                            .map(|_| ()),
                    );
                }
            }
            if let (Some(min), Some(max)) = (file.min_size_bytes, file.max_size_bytes)
                && min > max
            {
                issues.push(ValidationIssue::new(
                    format!("{field}.min_size_bytes"),
                    "min_size_bytes is greater than max_size_bytes",
                ));
            }
            if let Some(digest) = &file.sha256
                && (digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()))
            {
                issues.push(ValidationIssue::new(
                    format!("{field}.sha256"),
                    "must be a 64-character hexadecimal SHA-256 digest",
                ));
            }
            if !file.exists
                && (file.min_size_bytes.is_some()
                    || file.max_size_bytes.is_some()
                    || file.sha256.is_some())
            {
                issues.push(ValidationIssue::new(
                    field,
                    "`exists: false` cannot be combined with size or digest assertions",
                ));
            }
        }
        CheckKind::Container(container) => {
            check(
                issues,
                field,
                validate_cli_token(&format!("{field}.service"), &container.service),
            );
            if !compose.services.is_empty() && !compose.services.contains(&container.service) {
                issues.push(ValidationIssue::new(
                    format!("{field}.service"),
                    format!(
                        "service `{}` is not defined in the Compose file (defined: {})",
                        container.service,
                        compose.services.join(", ")
                    ),
                ));
            }
            if container.expected_exit_code.is_some()
                && container.state != crate::checks::ContainerState::Exited
            {
                issues.push(ValidationIssue::new(
                    format!("{field}.expected_exit_code"),
                    "only meaningful with `state: exited`",
                ));
            }
        }
        CheckKind::Script(script) => {
            let resolved = policy.resolve_existing(
                &format!("{field}.path"),
                &script.path,
                Confinement::ProjectOnly,
            );
            match resolved {
                Ok(path) => validate_script_permissions(&path, field, issues, warnings),
                Err(err) => issues.push(ValidationIssue::new(field, err.to_string())),
            }
            for (index, argument) in script.args.iter().enumerate() {
                if argument.contains('\0') {
                    issues.push(ValidationIssue::new(
                        format!("{field}.args[{index}]"),
                        "contains a NUL byte",
                    ));
                }
            }
            if let Some(workdir) = &script.workdir {
                check(
                    issues,
                    field,
                    policy
                        .resolve_existing(
                            &format!("{field}.workdir"),
                            workdir,
                            Confinement::ProjectOnly,
                        )
                        .map(|_| ()),
                );
            }
            for (key, value) in &script.environment {
                check(
                    issues,
                    field,
                    validate_env_entry(&format!("{field}.environment"), key, value),
                );
            }
        }
    }
}

/// Refuse to execute a script that other users on the machine can rewrite.
fn validate_script_permissions(
    path: &Path,
    field: &str,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    if !metadata.is_file() {
        issues.push(ValidationIssue::new(
            format!("{field}.path"),
            "is not a regular file",
        ));
        return;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = metadata.permissions().mode();
        if mode & 0o002 != 0 {
            issues.push(ValidationIssue::new(
                format!("{field}.path"),
                format!(
                    "`{}` is world-writable (mode {:o}). Any local user could change what this \
                     drill executes.",
                    path.display(),
                    mode & 0o7777
                ),
            ));
        }
        if mode & 0o111 == 0 {
            warnings.push(format!(
                "{field}: `{}` is not executable; it will be refused at run time",
                path.display()
            ));
        }
    }
}

fn resolve_backup(
    spec: &BackupSpec,
    policy: &PathPolicy,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) -> Option<ResolvedBackup> {
    match spec {
        BackupSpec::Local(local) => {
            let path = take(
                issues,
                policy.resolve_existing(
                    "backup.path",
                    &local.path,
                    Confinement::ProjectOrAllowlisted,
                ),
            )?;
            let metadata_file = match &local.metadata_file {
                Some(raw) => Some(take(
                    issues,
                    policy.resolve_existing(
                        "backup.metadata_file",
                        raw,
                        Confinement::ProjectOrAllowlisted,
                    ),
                )?),
                None => {
                    warnings.push(
                        "backup.metadata_file is not set: the backup timestamp will be derived from \
                         file modification times, which is an approximation"
                            .to_owned(),
                    );
                    None
                }
            };
            Some(ResolvedBackup::Local {
                path,
                metadata_file,
            })
        }
        BackupSpec::Restic(restic) => {
            check(
                issues,
                "backup.snapshot",
                validate_cli_token("backup.snapshot", &restic.snapshot),
            );
            if let Some(include) = &restic.include_path {
                check(
                    issues,
                    "backup.include_path",
                    validate_cli_token("backup.include_path", include),
                );
            }
            let repository = resolve_repository(
                "backup.repository",
                &restic.repository,
                &RESTIC_REMOTE_PREFIXES,
                policy,
                issues,
                warnings,
            )?;
            let password = resolve_secret_source(
                "backup",
                restic.password_file.as_deref(),
                restic.password_env.as_deref(),
                policy,
                issues,
                warnings,
            );
            Some(ResolvedBackup::Restic {
                repository,
                password,
                snapshot: restic.snapshot.clone(),
                include_path: restic.include_path.clone(),
            })
        }
        BackupSpec::Borg(borg) => {
            check(
                issues,
                "backup.archive",
                validate_cli_token("backup.archive", &borg.archive),
            );
            let repository = resolve_repository(
                "backup.repository",
                &borg.repository,
                &BORG_REMOTE_PREFIXES,
                policy,
                issues,
                warnings,
            )?;
            let passphrase = resolve_secret_source(
                "backup",
                borg.passphrase_file.as_deref(),
                borg.passphrase_env.as_deref(),
                policy,
                issues,
                warnings,
            );
            Some(ResolvedBackup::Borg {
                repository,
                passphrase,
                archive: borg.archive.clone(),
            })
        }
    }
}

const RESTIC_REMOTE_PREFIXES: [&str; 8] = [
    "s3:", "sftp:", "rest:", "b2:", "azure:", "gs:", "swift:", "rclone:",
];

const BORG_REMOTE_PREFIXES: [&str; 1] = ["ssh://"];

fn resolve_repository(
    field: &str,
    raw: &str,
    remote_prefixes: &[&str],
    policy: &PathPolicy,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) -> Option<RepositoryLocation> {
    if let Err(err) = validate_cli_token(field, raw) {
        issues.push(ValidationIssue::new(field, err.to_string()));
        return None;
    }
    let is_remote = remote_prefixes.iter().any(|prefix| raw.starts_with(prefix))
        || raw.contains("://")
        || (raw.contains('@') && raw.contains(':'));
    if is_remote {
        warnings.push(format!(
            "{field}: `{raw}` is a remote repository. The drill will make network requests, and the \
             backup tool's own credentials apply."
        ));
        return Some(RepositoryLocation::Remote(raw.to_owned()));
    }
    take(
        issues,
        policy.resolve_existing(field, Path::new(raw), Confinement::ProjectOrAllowlisted),
    )
    .map(RepositoryLocation::Local)
}

fn resolve_secret_source(
    field: &str,
    file: Option<&Path>,
    env: Option<&str>,
    policy: &PathPolicy,
    issues: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<String>,
) -> Option<SecretSource> {
    match (file, env) {
        (Some(_), Some(_)) => {
            issues.push(ValidationIssue::new(
                field,
                "declare either a password/passphrase file or an environment variable, not both",
            ));
            None
        }
        (Some(path), None) => {
            let resolved = take(
                issues,
                policy.resolve_existing(
                    &format!("{field}.password_file"),
                    path,
                    Confinement::ProjectOrAllowlisted,
                ),
            )?;
            warn_on_loose_permissions(&resolved, warnings);
            Some(SecretSource::File(resolved))
        }
        (None, Some(variable)) => {
            match validate_env_header(&format!("{field}.password_env"), "secret", variable) {
                Ok(()) => Some(SecretSource::Env(variable.to_owned())),
                Err(err) => {
                    issues.push(ValidationIssue::new(field, err.to_string()));
                    None
                }
            }
        }
        (None, None) => {
            warnings.push(format!(
                "{field}: no password source configured; the backup tool will use its own \
                 environment (for instance RESTIC_PASSWORD_FILE) if it is set"
            ));
            None
        }
    }
}

fn warn_on_loose_permissions(path: &Path, warnings: &mut Vec<String>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode();
            if mode & 0o077 != 0 {
                warnings.push(format!(
                    "`{}` is readable by other users (mode {:o}); restrict it with `chmod 600`",
                    path.display(),
                    mode & 0o7777
                ));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, warnings);
    }
}

/// Record an error as a validation issue and keep going.
fn check(issues: &mut Vec<ValidationIssue>, field: &str, result: Result<()>) {
    if let Err(err) = result {
        issues.push(ValidationIssue::new(field, err.to_string()));
    }
}

/// Record an error as a validation issue and yield `None`.
fn take<T>(issues: &mut Vec<ValidationIssue>, result: Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(err) => {
            let field = match &err {
                ConfigError::MissingPath { field, .. }
                | ConfigError::PathEscapesProject { field, .. } => field.clone(),
                _ => "configuration".to_owned(),
            };
            issues.push(ValidationIssue::new(field, err.to_string()));
            None
        }
    }
}
