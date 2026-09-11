//! What a check is allowed to know about the recovery environment.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use restoreproof_config::SecuritySpec;
use restoreproof_core::{Redactor, Secret};

/// State of a service in the recovery environment, as observed by the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceObservation {
    /// Service name from the Compose file.
    pub service: String,
    /// Container state: `running`, `exited`, `created`, ...
    pub state: String,
    /// Health status when the image declares a healthcheck.
    pub health: Option<String>,
    /// Exit code when the container has exited.
    pub exit_code: Option<i32>,
}

/// Errors a probe can report.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The service is not part of the environment.
    #[error("service `{0}` is not part of the recovery environment")]
    UnknownService(String),
    /// The environment could not be queried.
    #[error("{0}")]
    Unavailable(String),
}

/// Read-only view of the running recovery environment.
///
/// This trait is the boundary that keeps container orchestration out of the
/// check executors: `restoreproof-runner` implements it, `restoreproof-checks`
/// only consumes it. A check can never start, stop or modify a container.
#[async_trait]
pub trait EnvironmentProbe: Send + Sync {
    /// Observe one service.
    ///
    /// # Errors
    ///
    /// See [`ProbeError`].
    async fn observe(&self, service: &str) -> Result<ServiceObservation, ProbeError>;

    /// Host address a published container port is reachable at, if any.
    ///
    /// # Errors
    ///
    /// See [`ProbeError`].
    async fn published_port(&self, service: &str, port: u16) -> Result<Option<String>, ProbeError>;
}

/// Everything a check executor is given.
///
/// Note what is *not* here: no credentials beyond the ones the configuration
/// explicitly named, no handle on the container runtime, no mutable state.
#[derive(Clone)]
pub struct CheckContext {
    /// Absolute project directory. Command and script checks run under it.
    pub project_root: PathBuf,
    /// Absolute directory the backup was restored into.
    pub restore_dir: PathBuf,
    /// Compose project name of this drill.
    pub compose_project: String,
    /// Safety switches from the configuration.
    pub security: SecuritySpec,
    /// Redactor applied to every message and captured output.
    pub redactor: Redactor,
    /// Secrets resolved once by the runner, keyed by environment variable name.
    pub secrets: BTreeMap<String, Secret>,
    /// Read-only view of the environment, when one is running.
    pub probe: Option<Arc<dyn EnvironmentProbe>>,
}

impl std::fmt::Debug for CheckContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckContext")
            .field("project_root", &self.project_root)
            .field("restore_dir", &self.restore_dir)
            .field("compose_project", &self.compose_project)
            .field("secrets", &format!("{} resolved", self.secrets.len()))
            .field("probe", &self.probe.is_some())
            .finish_non_exhaustive()
    }
}

impl CheckContext {
    /// Environment variables injected into every command and script check.
    ///
    /// The child process starts from an empty environment; these three
    /// variables plus whatever the check declares are all it receives.
    #[must_use]
    pub fn injected_environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                "RESTOREPROOF_RESTORE_DIR".to_owned(),
                self.restore_dir.display().to_string(),
            ),
            (
                "RESTOREPROOF_PROJECT_ROOT".to_owned(),
                self.project_root.display().to_string(),
            ),
            (
                "RESTOREPROOF_COMPOSE_PROJECT".to_owned(),
                self.compose_project.clone(),
            ),
        ])
    }

    /// Look up a secret the runner resolved for this drill.
    #[must_use]
    pub fn secret(&self, variable: &str) -> Option<&Secret> {
        self.secrets.get(variable)
    }
}
