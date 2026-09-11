//! The `checks.yaml` document.
//!
//! A check is the unit of proof: it asserts one observable fact about the
//! recovered environment. Check definitions are data, never code — the only
//! types that execute anything are `command` and `script`, both gated by
//! `security.allow_command_checks`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Result};

/// Checks schema version understood by this build.
pub const CHECKS_SUPPORTED_VERSION: u32 = 1;

/// Default per-check timeout.
pub const DEFAULT_CHECK_TIMEOUT_SECONDS: u64 = 30;

const fn default_check_timeout() -> u64 {
    DEFAULT_CHECK_TIMEOUT_SECONDS
}

const fn default_attempts() -> u32 {
    1
}

const fn default_delay() -> u64 {
    2
}

const fn default_true() -> bool {
    true
}

const fn default_expected_status() -> u16 {
    200
}

/// Retry policy for a check.
///
/// Retrying is how a drill absorbs a service that is still warming up. It never
/// masks a failure: the number of attempts is recorded in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetrySpec {
    /// Total number of attempts, including the first one.
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Delay between two attempts, in seconds.
    #[serde(default = "default_delay")]
    pub delay_seconds: u64,
}

impl Default for RetrySpec {
    fn default() -> Self {
        Self {
            attempts: default_attempts(),
            delay_seconds: default_delay(),
        }
    }
}

/// Values applied to every check that does not override them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDefaults {
    /// Default per-check timeout, in seconds.
    #[serde(default = "default_check_timeout")]
    pub timeout_seconds: u64,
    /// Default retry policy.
    #[serde(default)]
    pub retry: RetrySpec,
}

impl Default for CheckDefaults {
    fn default() -> Self {
        Self {
            timeout_seconds: default_check_timeout(),
            retry: RetrySpec::default(),
        }
    }
}

/// The `checks.yaml` document.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChecksDocument {
    /// Schema version. Must equal [`CHECKS_SUPPORTED_VERSION`].
    pub version: u32,
    /// Values applied to every check that does not override them.
    #[serde(default)]
    pub defaults: CheckDefaults,
    /// The checks themselves, executed in declaration order.
    pub checks: Vec<CheckSpec>,
}

impl ChecksDocument {
    /// Parse a checks document from YAML text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Yaml`] when the document does not parse and
    /// [`ConfigError::UnsupportedVersion`] when `version` is not supported.
    pub fn from_yaml(path: &std::path::Path, text: &str) -> Result<Self> {
        let document: Self = serde_yaml_ng::from_str(text).map_err(|err| ConfigError::Yaml {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
        if document.version != CHECKS_SUPPORTED_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: document.version,
                supported: CHECKS_SUPPORTED_VERSION,
            });
        }
        Ok(document)
    }
}

/// One check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckSpec {
    /// Stable identifier, unique within the document.
    pub id: String,
    /// Human-readable name. Defaults to the id.
    pub name: Option<String>,
    /// Optional explanation shown in reports.
    pub description: Option<String>,
    /// Whether the check runs at all.
    pub enabled: bool,
    /// Whether a non-passing result fails the drill.
    pub required: bool,
    /// Per-check timeout override, in seconds.
    pub timeout_seconds: Option<u64>,
    /// Per-check retry override.
    pub retry: Option<RetrySpec>,
    /// What the check actually asserts.
    #[serde(flatten)]
    pub kind: CheckKind,
}

impl CheckSpec {
    /// Effective name: the explicit one, or the id.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Effective timeout, resolving the document defaults.
    #[must_use]
    pub const fn timeout(&self, defaults: &CheckDefaults) -> u64 {
        match self.timeout_seconds {
            Some(value) => value,
            None => defaults.timeout_seconds,
        }
    }

    /// Effective retry policy, resolving the document defaults.
    #[must_use]
    pub const fn retry_policy(&self, defaults: &CheckDefaults) -> RetrySpec {
        match self.retry {
            Some(value) => value,
            None => defaults.retry,
        }
    }
}

/// What a check asserts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckKind {
    /// Probe an HTTP endpoint of the recovered application.
    Http(HttpCheck),
    /// Run a local program and assert its exit code and output.
    Command(CommandCheck),
    /// Run a read-only SQL query against the recovered database.
    Sql(SqlCheck),
    /// Assert the presence, size or digest of a restored file.
    File(FileCheck),
    /// Assert the state of a service in the recovery environment.
    Container(ContainerCheck),
    /// Run a script versioned inside the project and assert its exit code.
    Script(ScriptCheck),
}

impl CheckKind {
    /// Discriminator as written in YAML.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Command(_) => "command",
            Self::Sql(_) => "sql",
            Self::File(_) => "file",
            Self::Container(_) => "container",
            Self::Script(_) => "script",
        }
    }

    /// Whether this check executes a local program.
    #[must_use]
    pub const fn executes_local_program(&self) -> bool {
        matches!(self, Self::Command(_) | Self::Script(_))
    }
}

/// HTTP method allowed in checks.
///
/// Only safe and idempotent-by-convention methods are offered; a check exists
/// to observe the recovered system, not to mutate it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// `GET`.
    #[default]
    Get,
    /// `HEAD`.
    Head,
    /// `POST`, for applications whose health endpoint requires it.
    Post,
}

impl HttpMethod {
    /// Method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
        }
    }
}

/// Probe an HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCheck {
    /// Request method.
    #[serde(default)]
    pub method: HttpMethod,
    /// Absolute URL. Must not embed credentials.
    pub url: String,
    /// Status code that makes the check pass.
    #[serde(default = "default_expected_status")]
    pub expected_status: u16,
    /// Substring the response body must contain.
    #[serde(default)]
    pub expect_body_contains: Option<String>,
    /// Extra request headers. Must not carry credentials: a configuration file
    /// is meant to be committed, so credentials go through
    /// [`HttpCheck::headers_from_env`] instead.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Headers whose value is read from an environment variable at run time.
    ///
    /// `Authorization: APP_HEALTH_TOKEN` reads `$APP_HEALTH_TOKEN`. The value is
    /// registered with the redactor before the request is sent.
    #[serde(default)]
    pub headers_from_env: BTreeMap<String, String>,
    /// Request body, for `POST`.
    #[serde(default)]
    pub body: Option<String>,
    /// Follow redirects. Disabled by default to keep the probe pointed at the
    /// service under test.
    #[serde(default)]
    pub follow_redirects: bool,
}

/// Run a local program.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandCheck {
    /// Program and arguments, as a list. A single string is refused on purpose:
    /// `RestoreProof` never hands a command line to a shell.
    pub command: Vec<String>,
    /// Working directory, relative to the project. Defaults to the project root.
    #[serde(default)]
    pub workdir: Option<PathBuf>,
    /// Exit code that makes the check pass.
    #[serde(default)]
    pub expected_exit_code: i32,
    /// Substring the standard output must contain.
    #[serde(default)]
    pub expect_stdout_contains: Option<String>,
    /// Extra environment for the child process. The child starts from an empty
    /// environment, never from the parent's.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

/// Run a read-only SQL query.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SqlCheck {
    /// Name of the environment variable holding the `PostgreSQL` connection
    /// string. The DSN itself never appears in the configuration file.
    pub dsn_env: String,
    /// A single read-only statement. See [`crate::sql_guard`].
    pub query: String,
    /// Exact expected number of rows.
    #[serde(default)]
    pub expect_row_count: Option<u64>,
    /// Minimum number of rows.
    #[serde(default)]
    pub min_rows: Option<u64>,
    /// Maximum number of rows.
    #[serde(default)]
    pub max_rows: Option<u64>,
    /// Expected textual value of the first column of the first row.
    #[serde(default)]
    pub expect_value: Option<String>,
    /// Minimum numeric value of the first column of the first row.
    #[serde(default)]
    pub min_value: Option<f64>,
    /// Maximum numeric value of the first column of the first row.
    #[serde(default)]
    pub max_value: Option<f64>,
}

/// Where a file path is resolved from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileBase {
    /// Inside the directory the backup was restored into.
    #[default]
    Restore,
    /// Inside the project directory.
    Project,
}

/// Assert something about a restored file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileCheck {
    /// Path relative to [`FileCheck::base`].
    pub path: PathBuf,
    /// What the path is relative to.
    #[serde(default)]
    pub base: FileBase,
    /// Whether the file is expected to exist.
    #[serde(default = "default_true")]
    pub exists: bool,
    /// Minimum size in bytes.
    #[serde(default)]
    pub min_size_bytes: Option<u64>,
    /// Maximum size in bytes.
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
    /// Expected SHA-256 digest, hex encoded.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Expected state of a service in the recovery environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    /// The container is running.
    #[default]
    Running,
    /// The container is running and its healthcheck reports healthy.
    Healthy,
    /// The container has exited.
    Exited,
}

/// Assert the state of a service.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContainerCheck {
    /// Service name, as written in the Compose file.
    pub service: String,
    /// Expected state.
    #[serde(default)]
    pub state: ContainerState,
    /// Expected exit code, only meaningful with `state: exited`.
    #[serde(default)]
    pub expected_exit_code: Option<i32>,
    /// Container port that must be published and accepting TCP connections.
    #[serde(default)]
    pub port: Option<u16>,
}

/// Run a script versioned inside the project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptCheck {
    /// Path to the script, always relative to the project directory.
    pub path: PathBuf,
    /// Arguments passed to the script.
    #[serde(default)]
    pub args: Vec<String>,
    /// Exit code that makes the check pass.
    #[serde(default)]
    pub expected_exit_code: i32,
    /// Working directory, relative to the project.
    #[serde(default)]
    pub workdir: Option<PathBuf>,
    /// Extra environment for the child process.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

impl<'de> Deserialize<'de> for CheckSpec {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use crate::tagged::{as_mapping, from_mapping, take_optional, take_optional_string, take_tag};
        use serde::de::Error as _;

        let value = serde_yaml_ng::Value::deserialize(deserializer)?;
        let mut mapping = as_mapping::<D::Error>("checks[]", value)?;

        let id = take_tag::<D::Error>("checks[]", &mut mapping, "id")?;
        let context = format!("checks[{id}]");
        let name = take_optional_string::<D::Error>(&context, &mut mapping, "name")?;
        let description = take_optional_string::<D::Error>(&context, &mut mapping, "description")?;
        let enabled = take_optional::<bool, D::Error>(&context, &mut mapping, "enabled")?.unwrap_or(true);
        let required =
            take_optional::<bool, D::Error>(&context, &mut mapping, "required")?.unwrap_or(true);
        let timeout_seconds = take_optional::<u64, D::Error>(&context, &mut mapping, "timeout_seconds")?;
        let retry = take_optional::<RetrySpec, D::Error>(&context, &mut mapping, "retry")?;
        let tag = take_tag::<D::Error>(&context, &mut mapping, "type")?;

        let kind = match tag.as_str() {
            "http" => CheckKind::Http(from_mapping::<_, D::Error>(&context, mapping)?),
            "command" => CheckKind::Command(from_mapping::<_, D::Error>(&context, mapping)?),
            "sql" => CheckKind::Sql(from_mapping::<_, D::Error>(&context, mapping)?),
            "file" => CheckKind::File(from_mapping::<_, D::Error>(&context, mapping)?),
            "container" => CheckKind::Container(from_mapping::<_, D::Error>(&context, mapping)?),
            "script" => CheckKind::Script(from_mapping::<_, D::Error>(&context, mapping)?),
            other => {
                return Err(D::Error::custom(format!(
                    "{context}: unknown check type `{other}`. Supported types are: http, command, \
                     sql, file, container, script."
                )));
            }
        };

        Ok(Self {
            id,
            name,
            description,
            enabled,
            required,
            timeout_seconds,
            retry,
            kind,
        })
    }
}
