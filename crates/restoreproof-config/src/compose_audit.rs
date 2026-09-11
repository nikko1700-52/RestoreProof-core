//! Static safety audit of the Docker Compose file used for recovery.
//!
//! A recovery drill restores production data and starts containers from it. If
//! that environment can reach the host, the drill becomes the incident. This
//! module reads the Compose file **before** anything is started and refuses
//! configurations that would break isolation.
//!
//! The audit is a static one: it reasons about the Compose document, not about
//! what the images do at runtime. It is a guardrail against mistakes and
//! copy-pasted production Compose files, not a container sandbox. This
//! limitation is stated in `docs/security.md`.

use std::path::Path;

use serde_yaml_ng::Value;

use crate::error::{ConfigError, Result};
use crate::model::SecuritySpec;
use crate::paths::{Confinement, PathPolicy};

/// Result of auditing a Compose file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeAudit {
    /// Service names declared in the file, in declaration order.
    pub services: Vec<String>,
    /// Problems that prevent the drill from running.
    pub errors: Vec<String>,
    /// Problems worth reading but not blocking.
    pub warnings: Vec<String>,
}

impl ComposeAudit {
    /// Whether the Compose file is safe to start.
    #[must_use]
    pub fn is_safe(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Container capabilities that effectively grant host access.
const DANGEROUS_CAPABILITIES: [&str; 7] = [
    "ALL",
    "SYS_ADMIN",
    "SYS_MODULE",
    "SYS_PTRACE",
    "SYS_RAWIO",
    "DAC_READ_SEARCH",
    "DAC_OVERRIDE",
];

/// Host paths that must never be bind-mounted into a recovery environment.
const FORBIDDEN_MOUNT_PREFIXES: [&str; 10] = [
    "/",
    "/boot",
    "/dev",
    "/etc",
    "/proc",
    "/root",
    "/run",
    "/sys",
    "/usr",
    "/var",
];

/// Compose variables the runner itself injects, and which are therefore safe to
/// use as a bind-mount source.
const INJECTED_VARIABLES: [&str; 2] = ["RESTOREPROOF_RESTORE_DIR", "RESTOREPROOF_WORKDIR"];

/// Audit a Compose document.
///
/// # Errors
///
/// Returns [`ConfigError::Yaml`] when the document does not parse. Unsafe
/// constructs are reported in [`ComposeAudit::errors`] rather than as an error,
/// so that `restoreproof validate` can list all of them at once.
pub fn audit(
    compose_path: &Path,
    text: &str,
    policy: &PathPolicy,
    security: &SecuritySpec,
    network_isolated: bool,
) -> Result<ComposeAudit> {
    let document: Value = serde_yaml_ng::from_str(text).map_err(|err| ConfigError::Yaml {
        path: compose_path.to_path_buf(),
        message: err.to_string(),
    })?;

    let mut audit = ComposeAudit::default();

    if let Some(version) = document.get("version").and_then(Value::as_str) {
        audit.warnings.push(format!(
            "the Compose file declares `version: {version}`, which Compose v2 ignores; it can be removed"
        ));
    }

    if document.get("include").is_some() {
        audit.errors.push(
            "`include:` is not supported: every service definition must live in the audited \
             Compose file"
                .to_owned(),
        );
    }

    audit_top_level_networks(&document, network_isolated, &mut audit);

    let Some(services) = document.get("services").and_then(Value::as_mapping) else {
        audit
            .errors
            .push("the Compose file declares no `services:` section".to_owned());
        return Ok(audit);
    };

    if services.is_empty() {
        audit
            .errors
            .push("the Compose file declares no service".to_owned());
    }

    for (name, service) in services {
        let Some(name) = name.as_str() else {
            audit
                .errors
                .push("a service name is not a string".to_owned());
            continue;
        };
        audit.services.push(name.to_owned());
        audit_service(name, service, compose_path, policy, security, &mut audit);
    }

    Ok(audit)
}

fn audit_top_level_networks(document: &Value, network_isolated: bool, audit: &mut ComposeAudit) {
    let Some(networks) = document.get("networks").and_then(Value::as_mapping) else {
        return;
    };
    for (name, network) in networks {
        let name = name.as_str().unwrap_or("<unnamed>");
        let external = network
            .get("external")
            .is_some_and(|value| value.as_bool() == Some(true) || value.as_mapping().is_some());
        if external && network_isolated {
            audit.errors.push(format!(
                "network `{name}` is declared `external`, which would attach the recovery \
                 environment to a pre-existing network. Set `recovery.network_isolated: false` \
                 only if you understand the consequences."
            ));
        }
    }
}

fn audit_service(
    name: &str,
    service: &Value,
    compose_path: &Path,
    policy: &PathPolicy,
    security: &SecuritySpec,
    audit: &mut ComposeAudit,
) {
    if service.get("privileged").and_then(Value::as_bool) == Some(true) {
        audit.errors.push(format!(
            "service `{name}` requests `privileged: true`. A privileged container is equivalent to \
             root on the host; RestoreProof never starts one."
        ));
    }

    for (field, value) in [
        ("network_mode", "host"),
        ("pid", "host"),
        ("ipc", "host"),
        ("uts", "host"),
        ("userns_mode", "host"),
        ("cgroup", "host"),
    ] {
        if service.get(field).and_then(Value::as_str) == Some(value) {
            audit.errors.push(format!(
                "service `{name}` sets `{field}: {value}`, which removes the isolation the drill \
                 depends on."
            ));
        }
    }

    if let Some(caps) = service.get("cap_add").and_then(Value::as_sequence) {
        for cap in caps {
            let cap_name = cap.as_str().unwrap_or_default().to_uppercase();
            if DANGEROUS_CAPABILITIES.contains(&cap_name.as_str()) {
                audit.errors.push(format!(
                    "service `{name}` adds capability `{cap_name}`, which allows escaping the \
                     container."
                ));
            }
        }
    }

    if let Some(opts) = service.get("security_opt").and_then(Value::as_sequence) {
        for opt in opts {
            let opt = opt.as_str().unwrap_or_default().to_lowercase();
            if opt.contains("unconfined") || opt.contains("label:disable") || opt.contains("no-new-privileges:false") {
                audit.errors.push(format!(
                    "service `{name}` sets `security_opt: {opt}`, which disables a kernel \
                     confinement mechanism."
                ));
            }
        }
    }

    if service
        .get("devices")
        .and_then(Value::as_sequence)
        .is_some_and(|devices| !devices.is_empty())
    {
        audit.errors.push(format!(
            "service `{name}` maps host devices, which exposes host hardware to restored data."
        ));
    }

    if let Some(ports) = service.get("ports").and_then(Value::as_sequence)
        && !ports.is_empty()
        && !security.allow_published_ports
    {
        audit.errors.push(format!(
            "service `{name}` publishes ports on the host. A recovery environment usually holds a \
             copy of production data and should not be reachable from the host network. Set \
             `security.allow_published_ports: true` to accept this."
        ));
    }

    audit_volumes(name, service, compose_path, policy, audit);
    audit_image(name, service, audit);
}

fn audit_volumes(
    name: &str,
    service: &Value,
    compose_path: &Path,
    policy: &PathPolicy,
    audit: &mut ComposeAudit,
) {
    let Some(volumes) = service.get("volumes").and_then(Value::as_sequence) else {
        return;
    };

    for volume in volumes {
        let source = match volume {
            Value::String(spec) => spec.split(':').next().unwrap_or_default().to_owned(),
            Value::Mapping(_) => {
                let kind = volume.get("type").and_then(Value::as_str).unwrap_or("volume");
                if kind != "bind" {
                    continue;
                }
                volume
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            }
            _ => continue,
        };

        if source.is_empty() {
            continue;
        }

        // Named volumes have no path separator and are managed by Docker.
        let is_path = source.starts_with('/')
            || source.starts_with('.')
            || source.starts_with('~')
            || source.starts_with("${");
        if !is_path {
            continue;
        }

        if source.contains("docker.sock") {
            audit.errors.push(format!(
                "service `{name}` mounts the Docker socket (`{source}`). That grants full control \
                 of the host's Docker daemon to a container started from restored data."
            ));
            continue;
        }

        if let Some(variable) = interpolated_variable(&source) {
            if INJECTED_VARIABLES.contains(&variable.as_str()) {
                continue; // provided by the runner, already confined
            }
            audit.errors.push(format!(
                "service `{name}` uses the variable `${{{variable}}}` as a bind-mount source. Only \
                 variables injected by RestoreProof ({}) may be used there, because the value of \
                 any other variable is not audited.",
                INJECTED_VARIABLES.join(", ")
            ));
            continue;
        }

        if FORBIDDEN_MOUNT_PREFIXES.contains(&source.trim_end_matches('/')) || source == "/" {
            audit.errors.push(format!(
                "service `{name}` bind-mounts the host path `{source}`, which is never acceptable \
                 in a recovery environment."
            ));
            continue;
        }

        // Relative bind mounts are resolved against the Compose file directory.
        let compose_dir = compose_path.parent().unwrap_or(Path::new("."));
        let candidate = if Path::new(&source).is_absolute() {
            std::path::PathBuf::from(&source)
        } else {
            compose_dir.join(&source)
        };
        if let Err(err) = policy.resolve_for_creation(
            &format!("services.{name}.volumes"),
            &candidate,
            Confinement::ProjectOrAllowlisted,
        ) {
            audit.errors.push(format!(
                "service `{name}` bind-mounts `{source}`, which is not permitted: {err}"
            ));
        }
    }
}

fn audit_image(name: &str, service: &Value, audit: &mut ComposeAudit) {
    let Some(image) = service.get("image").and_then(Value::as_str) else {
        if service.get("build").is_none() {
            audit.errors.push(format!(
                "service `{name}` declares neither `image:` nor `build:`"
            ));
        }
        return;
    };
    let tag = image.rsplit('/').next().unwrap_or(image);
    if !tag.contains(':') || tag.ends_with(":latest") {
        audit.warnings.push(format!(
            "service `{name}` uses `{image}`: pin an explicit tag or digest so a drill is \
             reproducible over time"
        ));
    }
}

/// Extract `NAME` from a source written as `${NAME}` or `${NAME}/sub`.
fn interpolated_variable(source: &str) -> Option<String> {
    let start = source.find("${")?;
    let rest = source.get(start + 2..)?;
    let end = rest.find('}')?;
    let name = rest.get(..end)?;
    let name = name.split([':', '-', '?']).next().unwrap_or(name);
    Some(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _dir: TempDir,
        root: std::path::PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(root.join("data")).unwrap();
        Fixture { _dir: dir, root }
    }

    fn run(text: &str) -> ComposeAudit {
        run_with(text, &SecuritySpec::default(), true)
    }

    fn run_with(text: &str, security: &SecuritySpec, isolated: bool) -> ComposeAudit {
        let f = fixture();
        let policy = PathPolicy::new(&f.root, &[]).unwrap();
        audit(
            &f.root.join("docker-compose.recovery.yml"),
            text,
            &policy,
            security,
            isolated,
        )
        .unwrap()
    }

    #[test]
    fn a_minimal_safe_file_passes() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27-alpine\n    volumes:\n      - ./data:/srv:ro\n",
        );
        assert!(result.is_safe(), "{:?}", result.errors);
        assert_eq!(result.services, vec!["app"]);
    }

    #[test]
    fn privileged_containers_are_refused() {
        let result = run("services:\n  app:\n    image: nginx:1.27\n    privileged: true\n");
        assert!(!result.is_safe());
        assert!(result.errors.iter().any(|e| e.contains("privileged")));
    }

    #[test]
    fn docker_socket_mounts_are_refused() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    volumes:\n      - /var/run/docker.sock:/var/run/docker.sock\n",
        );
        assert!(!result.is_safe());
        assert!(result.errors.iter().any(|e| e.contains("Docker socket")));
    }

    #[test]
    fn host_namespaces_are_refused() {
        for field in ["network_mode", "pid", "ipc", "userns_mode"] {
            let result = run(&format!(
                "services:\n  app:\n    image: nginx:1.27\n    {field}: host\n"
            ));
            assert!(!result.is_safe(), "{field} should be refused");
        }
    }

    #[test]
    fn dangerous_capabilities_are_refused() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    cap_add:\n      - SYS_ADMIN\n",
        );
        assert!(!result.is_safe());
        assert!(result.errors.iter().any(|e| e.contains("SYS_ADMIN")));
    }

    #[test]
    fn unconfined_security_options_are_refused() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    security_opt:\n      - seccomp:unconfined\n",
        );
        assert!(!result.is_safe());
    }

    #[test]
    fn host_root_mounts_are_refused() {
        for path in ["/", "/etc", "/var", "/usr"] {
            let result = run(&format!(
                "services:\n  app:\n    image: nginx:1.27\n    volumes:\n      - {path}:/host\n"
            ));
            assert!(!result.is_safe(), "mounting {path} should be refused");
        }
    }

    #[test]
    fn mounts_outside_the_project_are_refused() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    volumes:\n      - ../../elsewhere:/srv\n",
        );
        assert!(!result.is_safe());
    }

    #[test]
    fn injected_variables_are_accepted_as_mount_sources() {
        let result = run(
            "services:\n  db:\n    image: postgres:16\n    volumes:\n      - ${RESTOREPROOF_RESTORE_DIR}:/restore:ro\n",
        );
        assert!(result.is_safe(), "{:?}", result.errors);
    }

    #[test]
    fn other_variables_are_refused_as_mount_sources() {
        let result = run(
            "services:\n  db:\n    image: postgres:16\n    volumes:\n      - ${HOME}:/restore\n",
        );
        assert!(!result.is_safe());
        assert!(result.errors.iter().any(|e| e.contains("HOME")));
    }

    #[test]
    fn published_ports_are_refused_by_default() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n",
        );
        assert!(!result.is_safe());
        assert!(result.errors.iter().any(|e| e.contains("publishes ports")));
    }

    #[test]
    fn published_ports_can_be_opted_into() {
        let security = SecuritySpec {
            allow_published_ports: true,
            ..SecuritySpec::default()
        };
        let result = run_with(
            "services:\n  app:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n",
            &security,
            true,
        );
        assert!(result.is_safe(), "{:?}", result.errors);
    }

    #[test]
    fn external_networks_are_refused_when_isolation_is_required() {
        let text = "services:\n  app:\n    image: nginx:1.27\nnetworks:\n  prod:\n    external: true\n";
        assert!(!run_with(text, &SecuritySpec::default(), true).is_safe());
        assert!(run_with(text, &SecuritySpec::default(), false).is_safe());
    }

    #[test]
    fn include_is_refused() {
        let result = run("include:\n  - other.yml\nservices:\n  app:\n    image: nginx:1.27\n");
        assert!(!result.is_safe());
    }

    #[test]
    fn unpinned_images_are_warned_about() {
        let result = run("services:\n  app:\n    image: nginx\n");
        assert!(result.is_safe());
        assert!(result.warnings.iter().any(|w| w.contains("pin an explicit tag")));
    }

    #[test]
    fn a_file_without_services_is_refused() {
        let result = run("networks: {}\n");
        assert!(!result.is_safe());
    }

    #[test]
    fn device_mappings_are_refused() {
        let result = run(
            "services:\n  app:\n    image: nginx:1.27\n    devices:\n      - /dev/sda:/dev/sda\n",
        );
        assert!(!result.is_safe());
    }

    #[test]
    fn interpolated_variable_is_extracted() {
        assert_eq!(interpolated_variable("${FOO}/bar").as_deref(), Some("FOO"));
        assert_eq!(
            interpolated_variable("${RESTOREPROOF_RESTORE_DIR}").as_deref(),
            Some("RESTOREPROOF_RESTORE_DIR")
        );
        assert_eq!(interpolated_variable("/plain/path"), None);
    }
}
