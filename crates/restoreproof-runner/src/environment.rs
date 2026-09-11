//! Lifecycle of the isolated recovery environment.
//!
//! # Cleanup is not best-effort
//!
//! A drill starts containers and volumes holding a copy of production data.
//! Leaving them behind is both a disk leak and a data-exposure problem, so
//! cleanup happens on three independent paths:
//!
//! 1. the normal path calls [`RecoveryEnvironment::teardown`];
//! 2. any early return unwinds through [`Drop`], which tears the project down
//!    synchronously — this covers a failed restore, a panicking check and a
//!    cancelled future alike;
//! 3. the project name is unique per run, so a leftover environment can always
//!    be found and removed with `docker compose -p <name> down -v`.
//!
//! Only `--keep-environment` disables all three, and the CLI warns when it does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use restoreproof_checks::context::{EnvironmentProbe, ProbeError, ServiceObservation};

use crate::docker::{ComposeService, DockerCli, blocking_teardown};
use crate::error::{Result, RunnerError};

/// Interval between two readiness polls.
const POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// A running Compose project, torn down when it goes out of scope.
#[derive(Debug)]
pub struct RecoveryEnvironment {
    cli: DockerCli,
    compose_file: PathBuf,
    project_directory: PathBuf,
    cleanup: bool,
    started: bool,
    torn_down: bool,
}

impl RecoveryEnvironment {
    /// Start the environment with `docker compose up --detach`.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] when the project fails to start.
    /// The partially started project is torn down before returning.
    pub async fn start(
        compose_file: PathBuf,
        project_directory: PathBuf,
        project_name: String,
        environment: BTreeMap<String, String>,
        max_output_bytes: usize,
        cleanup: bool,
        timeout: Duration,
    ) -> Result<Self> {
        let cli = DockerCli::new(
            compose_file.clone(),
            project_directory.clone(),
            project_name,
            environment,
            max_output_bytes,
        );

        let environment = Self {
            cli,
            compose_file,
            project_directory,
            cleanup,
            started: true, // armed before `up`, so a partial start is still cleaned up
            torn_down: false,
        };

        let output = environment
            .cli
            .compose(&["up", "--detach", "--no-color", "--quiet-pull"], timeout)
            .await?;

        if !output.is_success() {
            let details = output.stderr_tail(20);
            return Err(RunnerError::Environment(format!(
                "the recovery environment failed to start ({}):\n{details}",
                output.describe_status()
            )));
        }

        Ok(environment)
    }

    /// Attach to a project that is already running, without starting anything.
    ///
    /// Used by `restoreproof check`, which inspects an environment the operator
    /// started themselves. Nothing is torn down on drop in this mode.
    #[must_use]
    pub fn attach(
        compose_file: PathBuf,
        project_directory: PathBuf,
        project_name: String,
        environment: BTreeMap<String, String>,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            cli: DockerCli::new(
                compose_file.clone(),
                project_directory.clone(),
                project_name,
                environment,
                max_output_bytes,
            ),
            compose_file,
            project_directory,
            cleanup: false,
            started: false,
            torn_down: true,
        }
    }

    /// Compose project name of this environment.
    #[must_use]
    pub fn project_name(&self) -> &str {
        self.cli.project_name()
    }

    /// A read-only probe for check executors.
    #[must_use]
    pub fn probe(&self) -> ComposeProbe {
        ComposeProbe {
            cli: self.cli.clone(),
        }
    }

    /// Wait until every requested service is ready.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] when a service fails outright, and
    /// [`RunnerError::Timeout`] when the deadline expires. Both carry the state
    /// of every service and the tail of the container logs.
    pub async fn wait_until_ready(
        &self,
        wait_for: &[String],
        timeout: Duration,
    ) -> Result<Vec<ComposeService>> {
        let deadline = Instant::now() + timeout;

        loop {
            let services = self.cli.services().await?;

            if services.is_empty() {
                return Err(RunnerError::Environment(
                    "the Compose project started no container".to_owned(),
                ));
            }

            let watched: Vec<&ComposeService> = if wait_for.is_empty() {
                services.iter().collect()
            } else {
                services
                    .iter()
                    .filter(|service| wait_for.contains(&service.service))
                    .collect()
            };

            if let Some(failed) = watched.iter().find(|service| service.has_failed()) {
                let logs = self.logs_tail(30).await;
                return Err(RunnerError::Environment(format!(
                    "service `{}` failed to start (state `{}`{}).\n{logs}",
                    failed.service,
                    failed.state,
                    failed
                        .exit_code
                        .map(|code| format!(", exit code {code}"))
                        .unwrap_or_default()
                )));
            }

            if watched.iter().all(|service| service.is_ready()) {
                return Ok(services);
            }

            if Instant::now() >= deadline {
                let pending: Vec<String> = watched
                    .iter()
                    .filter(|service| !service.is_ready())
                    .map(|service| {
                        format!(
                            "{} ({}{})",
                            service.service,
                            service.state,
                            service
                                .health()
                                .map(|health| format!(", health: {health}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect();
                let logs = self.logs_tail(30).await;
                return Err(RunnerError::Timeout(format!(
                    "the recovery environment was not ready within {}s.\nStill waiting on: {}.\n{logs}",
                    timeout.as_secs(),
                    pending.join(", ")
                )));
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Tail of the container logs, used to explain a failure.
    pub async fn logs_tail(&self, lines: usize) -> String {
        let lines = lines.to_string();
        match self
            .cli
            .compose(
                &["logs", "--no-color", "--tail", &lines],
                Duration::from_secs(30),
            )
            .await
        {
            Ok(output) if !output.stdout.trim().is_empty() => {
                format!("Container logs (last {lines} lines):\n{}", output.stdout)
            }
            _ => "No container logs were available.".to_owned(),
        }
    }

    /// Destroy containers, networks and volumes.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::Environment`] if the teardown command fails, but
    /// the environment is marked as torn down either way so the drop guard does
    /// not try again.
    pub async fn teardown(&mut self) -> Result<()> {
        if self.torn_down || !self.started {
            self.torn_down = true;
            return Ok(());
        }
        self.torn_down = true;

        if !self.cleanup {
            tracing::warn!(
                project = self.cli.project_name(),
                "the recovery environment was kept; remove it with `docker compose -p {} down -v`",
                self.cli.project_name()
            );
            return Ok(());
        }

        let output = self
            .cli
            .compose(
                &["down", "--volumes", "--remove-orphans", "--timeout", "30"],
                Duration::from_secs(180),
            )
            .await?;

        if output.is_success() {
            Ok(())
        } else {
            Err(RunnerError::Environment(format!(
                "the recovery environment could not be destroyed ({}). Remove it manually with \
                 `docker compose -p {} down -v`.\n{}",
                output.describe_status(),
                self.cli.project_name(),
                output.stderr_tail(10)
            )))
        }
    }

    /// Whether the environment was actually destroyed.
    #[must_use]
    pub const fn was_cleaned_up(&self) -> bool {
        self.torn_down && self.cleanup
    }
}

impl Drop for RecoveryEnvironment {
    fn drop(&mut self) {
        if self.torn_down || !self.started || !self.cleanup {
            return;
        }
        // Last-resort cleanup on an unwind or an early return. This is
        // synchronous on purpose: `Drop` cannot await, and leaving a container
        // holding restored production data behind is worse than blocking here.
        tracing::warn!(
            project = self.cli.project_name(),
            "tearing down the recovery environment from the drop guard"
        );
        if !blocking_teardown(
            &self.compose_file,
            &self.project_directory,
            self.cli.project_name(),
        ) {
            tracing::error!(
                project = self.cli.project_name(),
                "could not destroy the recovery environment; remove it with `docker compose -p {} down -v`",
                self.cli.project_name()
            );
        }
    }
}

/// Read-only view of a Compose project handed to check executors.
#[derive(Debug, Clone)]
pub struct ComposeProbe {
    cli: DockerCli,
}

#[async_trait]
impl EnvironmentProbe for ComposeProbe {
    async fn observe(&self, service: &str) -> std::result::Result<ServiceObservation, ProbeError> {
        let services = self
            .cli
            .services()
            .await
            .map_err(|err| ProbeError::Unavailable(err.to_string()))?;

        services
            .into_iter()
            .find(|candidate| candidate.service == service)
            .map(|candidate| ServiceObservation {
                service: candidate.service.clone(),
                state: candidate.state.clone(),
                health: candidate.health().map(str::to_owned),
                exit_code: candidate.exit_code,
            })
            .ok_or_else(|| ProbeError::UnknownService(service.to_owned()))
    }

    async fn published_port(
        &self,
        service: &str,
        port: u16,
    ) -> std::result::Result<Option<String>, ProbeError> {
        self.cli
            .published_port(service, port)
            .await
            .map_err(|err| ProbeError::Unavailable(err.to_string()))
    }
}

/// Compose project name for one run: the configured base plus a unique suffix.
///
/// The suffix guarantees that two concurrent drills, or a drill and a leftover
/// environment, never collide.
#[must_use]
pub fn unique_project_name(base: &str, run_id: &str) -> String {
    let suffix: String = run_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    let base: String = base.chars().take(40).collect();
    format!("{base}-rp-{suffix}")
}

/// Absolute path a Compose file should be resolved against.
#[must_use]
pub fn project_directory(compose_file: &Path) -> PathBuf {
    compose_file
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names_are_unique_and_valid() {
        let name = unique_project_name("example-app", "0d1a5f7c-2b3e-4a1d");
        assert!(name.starts_with("example-app-rp-"));
        assert!(name.len() <= 63);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
    }

    #[test]
    fn very_long_base_names_are_truncated() {
        let name = unique_project_name(&"a".repeat(80), "abcdefgh");
        assert!(name.len() <= 63);
    }

    #[test]
    fn attached_environments_never_tear_anything_down() {
        let environment = RecoveryEnvironment::attach(
            PathBuf::from("/tmp/compose.yml"),
            PathBuf::from("/tmp"),
            "already-running".to_owned(),
            BTreeMap::new(),
            1024,
        );
        assert!(!environment.was_cleaned_up());
        // Dropping must not try to run `docker compose down`.
        drop(environment);
    }

    #[test]
    fn the_project_directory_is_the_compose_file_directory() {
        assert_eq!(
            project_directory(Path::new("/srv/app/docker-compose.recovery.yml")),
            PathBuf::from("/srv/app")
        );
    }
}
