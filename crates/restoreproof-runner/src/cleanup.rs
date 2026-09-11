//! Process-wide registry of live recovery environments.
//!
//! # Why this exists
//!
//! A drill's teardown normally runs on the async path, and that covers success,
//! failure and timeouts. It does **not** cover the process being killed:
//! `Ctrl-C` in a terminal, a cancelled CI job, a `docker stop` on the runner.
//! Those deliver a signal, and without handling it the process dies leaving
//! containers and volumes holding a copy of production data on the machine.
//!
//! That is the exact outcome this product exists to prevent, so it is handled
//! explicitly: every environment registers itself here while it is live, and the
//! signal handler destroys whatever is still registered before exiting.
//!
//! The registry holds only Compose project names. Destroying a project needs
//! nothing else — see [`crate::docker::compose_down`] — which is what makes
//! cleanup work even when the Compose file can no longer be parsed.

use std::sync::{Mutex, OnceLock};

/// A Compose project that is currently live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveEnvironment {
    /// Unique Compose project name.
    pub project_name: String,
}

fn registry() -> &'static Mutex<Vec<ActiveEnvironment>> {
    static REGISTRY: OnceLock<Mutex<Vec<ActiveEnvironment>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Record a project as live.
pub fn register(project_name: &str) {
    if let Ok(mut active) = registry().lock() {
        if active
            .iter()
            .any(|entry| entry.project_name == project_name)
        {
            return;
        }
        active.push(ActiveEnvironment {
            project_name: project_name.to_owned(),
        });
    }
}

/// Forget a project that has been destroyed.
pub fn unregister(project_name: &str) {
    if let Ok(mut active) = registry().lock() {
        active.retain(|entry| entry.project_name != project_name);
    }
}

/// Projects currently registered as live.
#[must_use]
pub fn active() -> Vec<ActiveEnvironment> {
    registry()
        .lock()
        .map(|active| active.clone())
        .unwrap_or_default()
}

/// Empty the registry and return what was in it.
fn take_all() -> Vec<ActiveEnvironment> {
    let Ok(mut active) = registry().lock() else {
        return Vec::new();
    };
    std::mem::take(&mut *active)
}

/// Destroy every registered project, awaiting each one.
///
/// This is the path the signal handler uses. Unlike [`teardown_all_blocking`]
/// it can observe completion reliably, which matters because the process exits
/// immediately afterwards: a teardown that was started but not waited for
/// leaves the containers running.
///
/// Returns the projects that could **not** be destroyed, each with the reason,
/// so the operator is told what is left and why.
pub async fn teardown_all() -> Vec<(String, String)> {
    let mut failed = Vec::new();
    for environment in take_all() {
        if let Err(reason) = crate::docker::compose_down(&environment.project_name).await {
            failed.push((environment.project_name, reason));
        }
    }
    failed
}

/// Destroy every registered project, synchronously.
///
/// The last-resort path. Returns the project names that could not be destroyed.
#[must_use]
pub fn teardown_all_blocking() -> Vec<String> {
    let mut failed = Vec::new();
    for environment in take_all() {
        if !crate::docker::blocking_teardown(&environment.project_name) {
            failed.push(environment.project_name);
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_and_unregistering_tracks_live_projects() {
        let name = "registry-test-alpha";
        register(name);
        assert!(active().iter().any(|entry| entry.project_name == name));

        unregister(name);
        assert!(!active().iter().any(|entry| entry.project_name == name));
    }

    #[test]
    fn registering_twice_records_one_entry() {
        let name = "registry-test-beta";
        register(name);
        register(name);
        assert_eq!(
            active()
                .iter()
                .filter(|entry| entry.project_name == name)
                .count(),
            1
        );
        unregister(name);
    }

    #[test]
    fn unregistering_an_unknown_project_is_harmless() {
        unregister("registry-test-never-registered");
    }
}
