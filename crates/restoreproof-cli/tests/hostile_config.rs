//! Adversarial configurations.
//!
//! Each case here is an attack a configuration file could attempt against the
//! machine running the drill. They are kept together, and run against the real
//! binary, because these refusals are the product's security claim: a
//! `restoreproof.yaml` from a template repository, a vendor or a pull request
//! must not be able to read arbitrary files, reach arbitrary hosts, escape the
//! container, or hijack the tools the drill runs.
//!
//! Every case asserts exit code 2 (invalid configuration) *and* that the
//! message names the reason, so a refusal that happens for an unrelated reason
//! — a typo in the fixture, say — does not pass for a working control.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod fixtures;

use fixtures::{Fixture, cli};

const SAFE_COMPOSE: &str = r#"services:
  db:
    image: postgres:16-alpine
    volumes:
      - ${RESTOREPROOF_RESTORE_DIR}:/restore:ro
"#;

const SAFE_CHECKS: &str = r#"version: 1
checks:
  - id: dump-present
    type: file
    base: restore
    path: dump.sql
"#;

/// A minimal scenario that validates, to be broken one field at a time.
fn scenario() -> Fixture {
    let fixture = Fixture::valid();
    fixture.write("docker-compose.recovery.yml", SAFE_COMPOSE);
    fixture.write("checks.yaml", SAFE_CHECKS);
    fixture.write(
        "restoreproof.yaml",
        r#"version: 1
project:
  name: hostile
backup:
  type: local
  path: ./backup
recovery:
  compose_file: ./docker-compose.recovery.yml
checks_file: ./checks.yaml
"#,
    );
    fixture
}

/// Validate the fixture and require a refusal mentioning `reason`.
fn refused(fixture: &Fixture, reason: &str) {
    let assertion = cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2);

    let output = assertion.get_output();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains(reason),
        "refused, but not for the expected reason.\nexpected to find: {reason}\ngot:\n{text}"
    );
}

/// The baseline must pass, or every test below would pass for the wrong reason.
#[test]
fn the_baseline_scenario_is_accepted() {
    let fixture = scenario();
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn argument_injection_through_a_snapshot_id_is_refused() {
    let fixture = scenario();
    fixture.write(
        "restoreproof.yaml",
        r#"version: 1
project:
  name: hostile
backup:
  type: restic
  repository: ./backup
  snapshot: "--password-command=touch /tmp/restoreproof-pwned"
recovery:
  compose_file: ./docker-compose.recovery.yml
checks_file: ./checks.yaml
"#,
    );
    refused(&fixture, "argument injection");
    assert!(!std::path::Path::new("/tmp/restoreproof-pwned").exists());
}

#[test]
fn path_traversal_out_of_the_project_is_refused() {
    let fixture = scenario();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config()).unwrap().replace(
            "checks_file: ./checks.yaml",
            "checks_file: ../../../../etc/passwd",
        ),
    );
    refused(&fixture, "outside the project directory");
}

#[cfg(unix)]
#[test]
fn a_symlink_cannot_smuggle_a_path_out_of_the_project() {
    let fixture = scenario();
    std::os::unix::fs::symlink("/etc/passwd", fixture.path("looks-local.yaml")).unwrap();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config()).unwrap().replace(
            "checks_file: ./checks.yaml",
            "checks_file: ./looks-local.yaml",
        ),
    );
    refused(&fixture, "outside the project directory");
}

#[test]
fn a_backup_path_pointing_at_a_system_directory_is_refused() {
    let fixture = scenario();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config())
            .unwrap()
            .replace("path: ./backup", "path: /etc"),
    );
    refused(&fixture, "outside the project directory");
}

#[test]
fn mounting_the_docker_socket_is_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    volumes:\n      - /var/run/docker.sock:/var/run/docker.sock\n",
    );
    refused(&fixture, "Docker socket");
}

#[test]
fn a_privileged_container_is_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    privileged: true\n",
    );
    refused(&fixture, "privileged");
}

#[test]
fn bind_mounting_the_host_root_is_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    volumes:\n      - \"/:/host\"\n",
    );
    refused(&fixture, "never acceptable");
}

#[test]
fn host_namespaces_are_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    network_mode: host\n",
    );
    refused(&fixture, "removes the isolation");
}

#[test]
fn stacked_sql_hidden_behind_a_comment_is_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: sqli\n    type: sql\n    dsn_env: DB\n    min_rows: 1\n    query: \"SELECT 1 -- harmless\\n; DROP TABLE orders\"\n",
    );
    refused(&fixture, "more than one statement");
}

#[test]
fn a_writing_statement_disguised_as_a_check_is_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: sqli\n    type: sql\n    dsn_env: DB\n    min_rows: 1\n    query: \"DELETE FROM orders\"\n",
    );
    refused(&fixture, "Only SELECT and WITH");
}

#[test]
fn server_side_file_reads_through_sql_are_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: leak\n    type: sql\n    dsn_env: DB\n    min_rows: 1\n    query: \"SELECT pg_read_file('/etc/passwd')\"\n",
    );
    refused(&fixture, "PG_READ_FILE");
}

#[test]
fn a_request_to_a_cloud_metadata_endpoint_is_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: ssrf\n    type: http\n    url: \"http://169.254.169.254/latest/meta-data/iam/security-credentials/\"\n",
    );
    refused(&fixture, "loopback");
}

#[test]
fn hijacking_path_through_the_compose_environment_is_refused() {
    let fixture = scenario();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config()).unwrap().replace(
            "  compose_file: ./docker-compose.recovery.yml",
            "  compose_file: ./docker-compose.recovery.yml\n  environment:\n    PATH: /tmp/evil",
        ),
    );
    refused(&fixture, "cannot be set from a configuration file");
}

#[test]
fn redirecting_the_drill_at_another_docker_daemon_is_refused() {
    let fixture = scenario();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config()).unwrap().replace(
            "  compose_file: ./docker-compose.recovery.yml",
            "  compose_file: ./docker-compose.recovery.yml\n  environment:\n    DOCKER_HOST: tcp://attacker.example:2375",
        ),
    );
    refused(&fixture, "cannot be set from a configuration file");
}

#[test]
fn http_header_injection_is_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: hdr\n    type: http\n    url: \"http://127.0.0.1:8080/\"\n    headers:\n      X-Trace: \"a\\r\\nX-Evil: 1\"\n",
    );
    refused(&fixture, "control character");
}

#[test]
fn a_credential_written_literally_in_the_configuration_is_refused() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: hdr\n    type: http\n    url: \"http://127.0.0.1:8080/\"\n    headers:\n      Authorization: \"Bearer sk_live_example\"\n",
    );
    refused(&fixture, "headers_from_env");
}

#[test]
fn an_unaudited_variable_as_a_mount_source_is_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    volumes:\n      - ${HOME}:/restore\n",
    );
    refused(&fixture, "HOME");
}

#[test]
fn exposing_restored_data_on_every_interface_is_refused() {
    let fixture = scenario();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  db:\n    image: postgres:16-alpine\n    ports:\n      - \"5432:5432\"\n",
    );
    refused(&fixture, "every network interface");
}

#[test]
fn a_script_outside_the_project_cannot_be_allowlisted() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: escape\n    type: script\n    path: /bin/sh\n",
    );
    fixture.write(
        "restoreproof.yaml",
        &format!(
            "{}\nsecurity:\n  allow_external_paths:\n    - /bin\n",
            std::fs::read_to_string(fixture.config())
                .unwrap()
                .trim_end()
        ),
    );
    refused(&fixture, "cannot be allowlisted");
}

#[test]
fn command_checks_can_be_disabled_entirely() {
    let fixture = scenario();
    fixture.write(
        "checks.yaml",
        "version: 1\nchecks:\n  - id: run-something\n    type: command\n    command: [\"/bin/true\"]\n",
    );
    fixture.write(
        "restoreproof.yaml",
        &format!(
            "{}\nsecurity:\n  allow_command_checks: false\n",
            std::fs::read_to_string(fixture.config())
                .unwrap()
                .trim_end()
        ),
    );
    refused(&fixture, "allow_command_checks");
}
