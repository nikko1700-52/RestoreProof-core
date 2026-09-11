//! The Docker Compose v2 client.
//!
//! Everything goes through `docker compose` with an explicit project name and
//! project directory, executed through
//! [`restoreproof_core::process::CommandSpec`] — no shell, bounded time,
//! bounded output.
//!
//! # Which environment reaches `docker`
//!
//! Two kinds of variables, treated differently on purpose:
//!
//! * variables that decide **which daemon** is contacted (`DOCKER_HOST`,
//!   `DOCKER_CONTEXT`, TLS settings) are forwarded from the operator's own
//!   environment, because that is how rootless and remote Docker are configured;
//! * variables a **configuration file** asks for are passed through as well, but
//!   the configuration is not allowed to define any of the above — see
//!   `restoreproof_config::argsafe::validate_env_entry`. A YAML file therefore
//!   cannot redirect a drill at another Docker daemon.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use restoreproof_core::CommandSpec;
use restoreproof_core::process::{CommandOutput, ProcessError};
use serde::Deserialize;

use crate::error::{Result, RunnerError};

/// Variables forwarded from the operator's environment to `docker`.
const FORWARDED_DOCKER_ENV: [&str; 6] = [
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "XDG_RUNTIME_DIR",
];

/// What `docker` and `docker compose` reported about themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerInfo {
    /// Output of `docker compose version --short`.
    pub compose_version: String,
    /// Server version reported by `docker info`.
    pub server_version: String,
}

/// State of one service, as reported by `docker compose ps`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComposeService {
    /// Container name.
    #[serde(default, rename = "Name")]
    pub name: String,
    /// Service name from the Compose file.
    #[serde(default, rename = "Service")]
    pub service: String,
    /// `running`, `exited`, `created`, ...
    #[serde(default, rename = "State")]
    pub state: String,
    /// `healthy`, `unhealthy`, `starting`, or empty when there is no healthcheck.
    #[serde(default, rename = "Health")]
    pub health: String,
    /// Exit code once the container has exited.
    #[serde(default, rename = "ExitCode")]
    pub exit_code: Option<i32>,
}

impl ComposeService {
    /// Health status, or `None` when the image declares no healthcheck.
    #[must_use]
    pub fn health(&self) -> Option<&str> {
        (!self.health.is_empty()).then_some(self.health.as_str())
    }

    /// Whether the service is ready to be checked.
    ///
    /// A service with a healthcheck must be healthy. A one-shot container that
    /// exited successfully counts as ready, which is how seed/init containers
    /// are supported.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        match self.state.as_str() {
            "running" => self.health().is_none_or(|health| health == "healthy"),
            "exited" => self.exit_code == Some(0),
            _ => false,
        }
    }

    /// Whether the service has failed in a way waiting cannot fix.
    #[must_use]
    pub fn has_failed(&self) -> bool {
        (self.state == "exited" && self.exit_code.is_some_and(|code| code != 0))
            || self.health() == Some("unhealthy")
            || self.state == "dead"
    }
}

/// A configured `docker compose` invocation target.
#[derive(Debug, Clone)]
pub struct DockerCli {
    compose_file: PathBuf,
    project_directory: PathBuf,
    project_name: String,
    environment: BTreeMap<String, String>,
    max_output_bytes: usize,
}

impl DockerCli {
    /// Build a client for one Compose project.
    #[must_use]
    pub fn new(
        compose_file: PathBuf,
        project_directory: PathBuf,
        project_name: String,
        environment: BTreeMap<String, String>,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            compose_file,
            project_directory,
            project_name,
            environment,
            max_output_bytes,
        }
    }

    /// Compose project name used by this client.
    #[must_use]
    pub fn project_name(&self) -> &str {
        &self.project_name
    }

    /// Verify that Docker and Compose v2 are usable before anything is started.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::MissingDependency`] with actionable advice when
    /// the CLI is absent, too old, or the daemon is unreachable.
    pub async fn ensure_available() -> Result<DockerInfo> {
        let version = base_command()
            .args(["compose", "version", "--short"])
            .timeout(Duration::from_secs(30))
            .run()
            .await
            .map_err(|err| {
                match err {
                ProcessError::NotFound { .. } => RunnerError::MissingDependency(
                    "`docker` was not found.\nRestoreProof needs Docker with the Compose v2 plugin \
                     to build an isolated recovery environment.\nInstall Docker Engine, then check \
                     `docker compose version`."
                        .to_owned(),
                ),
                other => RunnerError::MissingDependency(other.to_string()),
            }
            })?;

        if !version.is_success() {
            return Err(RunnerError::MissingDependency(format!(
                "`docker compose version` failed ({}).\nRestoreProof requires the Compose v2 \
                 plugin; the standalone `docker-compose` v1 script is not supported.\n{}",
                version.describe_status(),
                version.stderr_tail(5)
            )));
        }

        let info = base_command()
            .args(["info", "--format", "{{.ServerVersion}}"])
            .timeout(Duration::from_secs(30))
            .run()
            .await
            .map_err(|err| RunnerError::MissingDependency(err.to_string()))?;

        if !info.is_success() {
            return Err(RunnerError::MissingDependency(format!(
                "the Docker daemon is not reachable.\nStart Docker (or set DOCKER_HOST for a remote \
                 or rootless daemon) and try again.\n{}",
                info.stderr_tail(5)
            )));
        }

        Ok(DockerInfo {
            compose_version: version.stdout.trim().to_owned(),
            server_version: info.stdout.trim().to_owned(),
        })
    }

    /// Run a `docker compose` subcommand for this project.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] when the process cannot be run.
    pub async fn compose(&self, args: &[&str], timeout: Duration) -> Result<CommandOutput> {
        let mut spec = base_command()
            .args([
                "compose",
                "--file",
                &self.compose_file.display().to_string(),
                "--project-directory",
                &self.project_directory.display().to_string(),
                "--project-name",
                &self.project_name,
            ])
            .args(args.iter().copied())
            .timeout(timeout)
            .max_output_bytes(self.max_output_bytes);

        for (key, value) in &self.environment {
            spec = spec.env(key.as_str(), value.as_str());
        }

        spec.run().await.map_err(|err| match err {
            ProcessError::Timeout { seconds, .. } => RunnerError::Timeout(format!(
                "`docker compose {}` did not finish within {seconds}s",
                args.join(" ")
            )),
            other => RunnerError::Environment(other.to_string()),
        })
    }

    /// List the services of this project and their state.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] when the project cannot be queried.
    pub async fn services(&self) -> Result<Vec<ComposeService>> {
        let output = self
            .compose(
                &["ps", "--all", "--format", "json"],
                Duration::from_secs(60),
            )
            .await?;
        if !output.is_success() {
            return Err(RunnerError::Environment(format!(
                "`docker compose ps` failed ({}): {}",
                output.describe_status(),
                output.stderr_tail(5)
            )));
        }
        parse_services(&output.stdout)
    }

    /// Resolve the host address a published container port is reachable at.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] when the command cannot be run.
    pub async fn published_port(&self, service: &str, port: u16) -> Result<Option<String>> {
        let port = port.to_string();
        let output = self
            .compose(&["port", service, &port], Duration::from_secs(30))
            .await?;
        if !output.is_success() {
            return Ok(None);
        }
        let address = output.stdout.trim();
        if address.is_empty() {
            return Ok(None);
        }
        // `docker compose port` answers `0.0.0.0:15432`; connect over loopback.
        Ok(Some(address.replace("0.0.0.0:", "127.0.0.1:")))
    }
}

/// A `docker` invocation with the operator's daemon settings forwarded.
fn base_command() -> CommandSpec {
    let mut spec = CommandSpec::new("docker");
    for key in FORWARDED_DOCKER_ENV {
        if let Some(value) = std::env::var_os(key) {
            spec = spec.env(key, value);
        }
    }
    spec
}

/// Parse `docker compose ps --format json`.
///
/// Compose has emitted both a JSON array and newline-delimited objects
/// depending on the version, so both are accepted.
fn parse_services(stdout: &str) -> Result<Vec<ComposeService>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).map_err(|err| {
            RunnerError::Environment(format!("cannot parse `docker compose ps` output: {err}"))
        });
    }
    let mut services = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let service: ComposeService = serde_json::from_str(line).map_err(|err| {
            RunnerError::Environment(format!("cannot parse `docker compose ps` output: {err}"))
        })?;
        services.push(service);
    }
    Ok(services)
}

/// Arguments that destroy a Compose project, addressed by name alone.
///
/// Teardown deliberately does **not** pass `--file`. `docker compose down`
/// re-reads and interpolates the Compose file, so a file referring to
/// `${RESTOREPROOF_RESTORE_DIR}` fails to parse unless that variable is set
/// again — and a teardown that depends on reconstructing the environment it is
/// tearing down is a teardown that fails exactly when it is needed most.
///
/// Compose v2 resolves a project from the labels on its containers, so the
/// project name is sufficient and nothing on disk has to still be valid.
fn down_arguments(project: &str) -> [&str; 8] {
    [
        "compose",
        "--project-name",
        project,
        "down",
        "--volumes",
        "--remove-orphans",
        "--timeout",
        "30",
    ]
}

/// Destroy a Compose project from inside an async runtime.
///
/// Preferred over [`blocking_teardown`] wherever a runtime exists. Waiting on a
/// child through `std::process` while Tokio's process driver is reaping
/// children can fail with `ECHILD`, and a teardown whose completion cannot be
/// observed is one the caller may abandon half-done — which in practice leaves
/// the containers behind.
///
/// # Errors
///
/// Returns the reason the project could not be destroyed, so the operator can
/// be told what is still running on their machine and why.
pub async fn compose_down(project: &str) -> std::result::Result<(), String> {
    let mut spec = CommandSpec::new("docker")
        .args(down_arguments(project))
        .timeout(Duration::from_secs(180));

    for key in FORWARDED_DOCKER_ENV {
        if let Some(value) = std::env::var_os(key) {
            spec = spec.env(key, value);
        }
    }

    match spec.run().await {
        Ok(output) if output.is_success() => Ok(()),
        Ok(output) => Err(format!(
            "`docker compose down` exited with {}: {}",
            output.describe_status(),
            output.stderr_tail(5).trim()
        )),
        Err(err) => Err(err.to_string()),
    }
}

/// Destroy a Compose project without an async runtime.
///
/// The last-resort path, used from `Drop`. Prefer [`compose_down`].
#[must_use]
pub fn blocking_teardown(project: &str) -> bool {
    let mut command = std::process::Command::new("docker");
    command
        .args(down_arguments(project))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    for key in FORWARDED_DOCKER_ENV {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }

    command.status().is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_addresses_the_project_by_name_and_reads_no_file() {
        let arguments = down_arguments("example-rp-abcd1234");
        assert!(
            !arguments.contains(&"--file"),
            "teardown must not re-parse the Compose file: {arguments:?}"
        );
        assert!(arguments.contains(&"--project-name"));
        assert!(arguments.contains(&"example-rp-abcd1234"));
        assert!(arguments.contains(&"--volumes"));
        assert!(arguments.contains(&"--remove-orphans"));
    }

    #[test]
    fn newline_delimited_output_is_parsed() {
        let stdout = concat!(
            r#"{"Name":"p-db-1","Service":"db","State":"running","Health":"healthy","ExitCode":0}"#,
            "\n",
            r#"{"Name":"p-app-1","Service":"app","State":"running","Health":"","ExitCode":0}"#,
            "\n"
        );
        let services = parse_services(stdout).unwrap();
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].service, "db");
        assert_eq!(services[0].health(), Some("healthy"));
        assert_eq!(services[1].health(), None);
    }

    #[test]
    fn array_output_is_parsed() {
        let stdout = r#"[{"Name":"p-db-1","Service":"db","State":"running","Health":"healthy"}]"#;
        let services = parse_services(stdout).unwrap();
        assert_eq!(services.len(), 1);
    }

    #[test]
    fn empty_output_is_an_empty_project_not_an_error() {
        assert!(parse_services("   \n").unwrap().is_empty());
    }

    #[test]
    fn malformed_output_is_an_error() {
        assert!(parse_services("{not json}").is_err());
    }

    #[test]
    fn readiness_requires_health_when_a_healthcheck_exists() {
        let starting = ComposeService {
            name: "c".to_owned(),
            service: "db".to_owned(),
            state: "running".to_owned(),
            health: "starting".to_owned(),
            exit_code: None,
        };
        assert!(!starting.is_ready());
        assert!(!starting.has_failed());

        let healthy = ComposeService {
            health: "healthy".to_owned(),
            ..starting.clone()
        };
        assert!(healthy.is_ready());

        let unhealthy = ComposeService {
            health: "unhealthy".to_owned(),
            ..starting.clone()
        };
        assert!(unhealthy.has_failed());
    }

    #[test]
    fn a_successful_one_shot_container_counts_as_ready() {
        let seed = ComposeService {
            name: "c".to_owned(),
            service: "seed".to_owned(),
            state: "exited".to_owned(),
            health: String::new(),
            exit_code: Some(0),
        };
        assert!(seed.is_ready());
        assert!(!seed.has_failed());

        let failed = ComposeService {
            exit_code: Some(1),
            ..seed
        };
        assert!(failed.has_failed());
        assert!(!failed.is_ready());
    }

    #[test]
    fn a_service_without_a_healthcheck_is_ready_when_running() {
        let service = ComposeService {
            name: "c".to_owned(),
            service: "app".to_owned(),
            state: "running".to_owned(),
            health: String::new(),
            exit_code: None,
        };
        assert!(service.is_ready());
    }
}
