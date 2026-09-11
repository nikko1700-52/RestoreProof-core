//! The configuration document, exactly as written in `restoreproof.yaml`.
//!
//! Every structure here is `deny_unknown_fields`: a typo such as
//! `startup_timeout_second` is an error, not a silently ignored field that
//! makes a drill pass for the wrong reason.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};

/// Configuration schema version understood by this build.
pub const SUPPORTED_VERSION: u32 = 1;

/// Default time allowed for the recovery environment to become ready.
pub const DEFAULT_STARTUP_TIMEOUT_SECONDS: u64 = 180;

/// Default ceiling for a whole drill.
pub const DEFAULT_TOTAL_TIMEOUT_SECONDS: u64 = 3600;

/// Default ceiling for captured output of a single external command.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

const fn default_true() -> bool {
    true
}

const fn default_startup_timeout() -> u64 {
    DEFAULT_STARTUP_TIMEOUT_SECONDS
}

const fn default_total_timeout() -> u64 {
    DEFAULT_TOTAL_TIMEOUT_SECONDS
}

const fn default_max_output_bytes() -> usize {
    DEFAULT_MAX_OUTPUT_BYTES
}

fn default_snapshot() -> String {
    "latest".to_owned()
}

fn default_report_dir() -> PathBuf {
    PathBuf::from("./reports")
}

fn default_formats() -> Vec<ReportFormat> {
    vec![ReportFormat::Json, ReportFormat::Markdown]
}

fn default_checks_file() -> PathBuf {
    PathBuf::from("./checks.yaml")
}

/// The `restoreproof.yaml` document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    /// Schema version. Must equal [`SUPPORTED_VERSION`].
    pub version: u32,
    /// Identity of the system under test.
    pub project: Project,
    /// Where the backup comes from.
    pub backup: BackupSpec,
    /// How the recovery environment is built.
    pub recovery: RecoverySpec,
    /// Objectives to measure against.
    #[serde(default)]
    pub metrics: MetricsSpec,
    /// Path to the checks document.
    #[serde(default = "default_checks_file")]
    pub checks_file: PathBuf,
    /// Where reports are written.
    #[serde(default)]
    pub report: ReportSpec,
    /// Safety switches. Every default is the conservative choice.
    #[serde(default)]
    pub security: SecuritySpec,
}

/// Identity of the system under test.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Short machine-friendly name, also used as a Compose project prefix.
    pub name: String,
    /// Free-form description shown in reports.
    #[serde(default)]
    pub description: Option<String>,
    /// Version of the application under test, recorded in the report.
    #[serde(default)]
    pub application_version: Option<String>,
}

/// Where the backup comes from.
///
/// Serialized with a `type` discriminator and flat fields, matching the way it
/// is written in YAML:
///
/// ```yaml
/// backup:
///   type: restic
///   repository: "./fixtures/restic-repository"
///   password_file: "./fixtures/restic-password"
///   snapshot: latest
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackupSpec {
    /// A directory on the local filesystem holding a previous dump.
    Local(LocalBackup),
    /// A restic repository.
    Restic(ResticBackup),
    /// A `BorgBackup` repository.
    Borg(BorgBackup),
}

impl BackupSpec {
    /// Discriminator as written in YAML.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Restic(_) => "restic",
            Self::Borg(_) => "borg",
        }
    }
}

/// A local directory used as a backup source.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalBackup {
    /// Directory (or single file) holding the backup.
    pub path: PathBuf,
    /// Optional JSON file carrying `{"created_at": "<RFC 3339>"}`.
    ///
    /// Without it the creation time is derived from the most recent
    /// modification time under `path`, which is an approximation and is
    /// reported as such.
    #[serde(default)]
    pub metadata_file: Option<PathBuf>,
}

/// A restic repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResticBackup {
    /// Repository location, as understood by `restic -r`.
    pub repository: String,
    /// File holding the repository password. Never written to a report.
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    /// Name of an environment variable holding the repository password.
    #[serde(default)]
    pub password_env: Option<String>,
    /// Snapshot to restore, or `latest`.
    #[serde(default = "default_snapshot")]
    pub snapshot: String,
    /// Restrict the restore to this path inside the snapshot.
    #[serde(default)]
    pub include_path: Option<String>,
}

/// A `BorgBackup` repository.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BorgBackup {
    /// Repository location, as understood by `borg`.
    pub repository: String,
    /// File holding the repository passphrase.
    #[serde(default)]
    pub passphrase_file: Option<PathBuf>,
    /// Name of an environment variable holding the passphrase.
    #[serde(default)]
    pub passphrase_env: Option<String>,
    /// Archive to extract, or `latest`.
    #[serde(default = "default_snapshot")]
    pub archive: String,
}

impl<'de> Deserialize<'de> for BackupSpec {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut mapping = crate::tagged::as_mapping::<D::Error>(
            "backup",
            serde_yaml_ng::Value::deserialize(deserializer)?,
        )?;
        let tag = crate::tagged::take_tag::<D::Error>("backup", &mut mapping, "type")?;
        match tag.as_str() {
            "local" => Ok(Self::Local(crate::tagged::from_mapping::<_, D::Error>(
                "backup (type: local)",
                mapping,
            )?)),
            "restic" => Ok(Self::Restic(crate::tagged::from_mapping::<_, D::Error>(
                "backup (type: restic)",
                mapping,
            )?)),
            "borg" => Ok(Self::Borg(crate::tagged::from_mapping::<_, D::Error>(
                "backup (type: borg)",
                mapping,
            )?)),
            other => Err(D::Error::custom(format!(
                "backup: unknown type `{other}`. Supported types in the open-source edition are: \
                 local, restic, borg."
            ))),
        }
    }
}

/// How the recovery environment is built and torn down.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySpec {
    /// Docker Compose file describing the isolated recovery environment.
    pub compose_file: PathBuf,
    /// Base name for the Compose project. A unique suffix is always appended.
    #[serde(default)]
    pub project_name: Option<String>,
    /// How long the environment may take to become ready.
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_seconds: u64,
    /// Ceiling for the whole drill, restore included.
    #[serde(default = "default_total_timeout")]
    pub total_timeout_seconds: u64,
    /// Destroy containers, networks and volumes when the drill ends.
    #[serde(default = "default_true")]
    pub cleanup: bool,
    /// Refuse to start if the Compose file joins an external network.
    #[serde(default = "default_true")]
    pub network_isolated: bool,
    /// Services that must report healthy or running before checks start.
    ///
    /// Empty means "every service defined in the Compose file".
    #[serde(default)]
    pub wait_for: Vec<String>,
    /// Extra non-secret environment passed to `docker compose`.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Objectives to measure the drill against.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsSpec {
    /// JSON file providing `backup_timestamp` and/or `reference_timestamp`.
    ///
    /// Use it when the backup source cannot expose a creation time itself.
    #[serde(default)]
    pub rpo_reference_file: Option<PathBuf>,
    /// Recovery Time Objective, in seconds.
    #[serde(default)]
    pub target_rto_seconds: Option<u64>,
    /// Recovery Point Objective, in seconds.
    #[serde(default)]
    pub target_rpo_seconds: Option<u64>,
}

/// Report output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    /// Machine-readable report.
    Json,
    /// Human-readable report, suitable for a ticket.
    Markdown,
}

impl ReportFormat {
    /// File extension used for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
        }
    }
}

/// Where reports are written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSpec {
    /// Directory receiving the reports. Created if missing.
    #[serde(default = "default_report_dir")]
    pub directory: PathBuf,
    /// Formats to produce.
    #[serde(default = "default_formats")]
    pub formats: Vec<ReportFormat>,
}

impl Default for ReportSpec {
    fn default() -> Self {
        Self {
            directory: default_report_dir(),
            formats: default_formats(),
        }
    }
}

/// Safety switches.
///
/// Each default is the conservative option: a configuration has to *opt in* to
/// anything that widens the blast radius of a drill.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecuritySpec {
    /// Allow `command` and `script` checks to execute local programs.
    ///
    /// Set to `false` to run a drill whose checks cannot execute anything, for
    /// instance when the configuration comes from a less trusted source.
    #[serde(default = "default_true")]
    pub allow_command_checks: bool,
    /// Absolute locations outside the project that configuration paths may use.
    ///
    /// Never applies to executable content: scripts, the Compose file and the
    /// checks file must live inside the project directory.
    #[serde(default)]
    pub allow_external_paths: Vec<PathBuf>,
    /// Allow HTTP checks to target hosts other than the loopback interface.
    ///
    /// Disabled by default: a recovery drill probes the environment it just
    /// started, so a non-loopback target is usually a mistake, and at worst
    /// turns the drill into a request forgery tool.
    #[serde(default)]
    pub allow_external_http_targets: bool,
    /// Allow SQL checks to connect to a database other than one on loopback.
    ///
    /// Disabled by default for the same reason: a drill queries the database it
    /// just restored, so a remote DSN means either a mistake or a query aimed at
    /// production.
    #[serde(default)]
    pub allow_external_sql_targets: bool,
    /// Allow the Compose file to publish ports on every network interface.
    ///
    /// Publishing on a loopback address (`127.0.0.1:15432:5432`) is always
    /// allowed and is what HTTP and SQL checks use. This switch only governs
    /// bindings that expose a restored copy of production data to the network.
    #[serde(default)]
    pub allow_published_ports: bool,
    /// Maximum bytes captured from a single external command.
    #[serde(default = "default_max_output_bytes")]
    pub max_command_output_bytes: usize,
}

impl Default for SecuritySpec {
    fn default() -> Self {
        Self {
            allow_command_checks: true,
            allow_external_paths: Vec::new(),
            allow_external_http_targets: false,
            allow_external_sql_targets: false,
            allow_published_ports: false,
            max_command_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

impl RawConfig {
    /// Parse a configuration document from YAML text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Yaml`] when the document does not parse, and
    /// [`ConfigError::UnsupportedVersion`] when `version` is not supported.
    pub fn from_yaml(path: &std::path::Path, text: &str) -> Result<Self> {
        let config: Self = serde_yaml_ng::from_str(text).map_err(|err| ConfigError::Yaml {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
        if config.version != SUPPORTED_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: config.version,
                supported: SUPPORTED_VERSION,
            });
        }
        Ok(config)
    }
}
