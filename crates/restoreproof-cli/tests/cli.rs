//! End-to-end tests of the command line interface.
//!
//! These run the real binary against real files. They are the tests that would
//! catch a regression a unit test cannot see: a wrong exit code, a secret in
//! the output, a scenario that validates when it should not.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr
)]

mod fixtures;

use fixtures::{CHECKS, CONFIG, Fixture, cli};
use predicates::prelude::*;

// --- init ---------------------------------------------------------------

#[test]
fn init_creates_a_scenario_that_validates() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("drill");

    cli()
        .args(["init", target.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Created a recovery drill"));

    for file in [
        "restoreproof.yaml",
        "checks.yaml",
        "docker-compose.recovery.yml",
        "backup/dump.sql",
        "backup/backup-metadata.json",
    ] {
        assert!(
            target.join(file).is_file(),
            "{file} should have been created"
        );
    }

    cli()
        .args([
            "validate",
            "--config",
            target.join("restoreproof.yaml").to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn init_refuses_to_overwrite_without_force() {
    let dir = tempfile::TempDir::new().unwrap();
    let target = dir.path().join("drill");

    cli()
        .args(["init", target.to_str().unwrap()])
        .assert()
        .success();

    cli()
        .args(["init", target.to_str().unwrap()])
        .assert()
        .code(7)
        .stderr(predicate::str::contains("--force"));

    cli()
        .args(["init", target.to_str().unwrap(), "--force"])
        .assert()
        .success();
}

// --- validate -----------------------------------------------------------

#[test]
fn validate_accepts_a_correct_scenario() {
    let fixture = Fixture::valid();
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Configuration is valid"));
}

#[test]
fn validate_reports_a_missing_configuration_with_advice() {
    cli()
        .args(["validate", "--config", "/nonexistent/restoreproof.yaml"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("restoreproof init"));
}

#[test]
fn validate_refuses_unknown_fields() {
    let fixture = Fixture::valid();
    fixture.write(
        "restoreproof.yaml",
        &CONFIG.replace("checks_file:", "cheks_file: ./checks.yaml\nchecks_file:"),
    );
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cheks_file"));
}

#[test]
fn validate_reports_every_problem_at_once() {
    let fixture = Fixture::valid();
    fixture.write(
        "checks.yaml",
        r#"version: 1
checks:
  - id: BadId
    type: http
    url: "http://169.254.169.254/latest/meta-data/"
  - id: BadId
    type: sql
    dsn_env: DATABASE_URL
    query: "DROP TABLE orders"
"#,
    );

    let output = cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();

    assert!(
        stderr.contains("BadId"),
        "the invalid id should be reported: {stderr}"
    );
    assert!(
        stderr.contains("loopback"),
        "the SSRF target should be reported: {stderr}"
    );
    assert!(
        stderr.contains("duplicate check id"),
        "the duplicate id should be reported: {stderr}"
    );
    assert!(
        stderr.contains("DROP"),
        "the writing statement should be reported: {stderr}"
    );
}

#[test]
fn validate_refuses_a_path_escaping_the_project() {
    let fixture = Fixture::valid();
    fixture.write(
        "restoreproof.yaml",
        &CONFIG.replace("path: ./backup", "path: /etc"),
    );
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("outside the project directory"));
}

#[test]
fn validate_refuses_a_compose_file_that_breaks_isolation() {
    let fixture = Fixture::valid();
    fixture.write(
        "docker-compose.recovery.yml",
        "services:\n  database:\n    image: postgres:16-alpine\n    privileged: true\n    volumes:\n      - /var/run/docker.sock:/var/run/docker.sock\n",
    );

    let output = cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2);
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
    assert!(stderr.contains("privileged"), "{stderr}");
    assert!(stderr.contains("Docker socket"), "{stderr}");
}

#[test]
fn validate_refuses_a_port_published_on_every_interface() {
    let fixture = Fixture::valid();
    fixture.write(
        "docker-compose.recovery.yml",
        &fixtures::COMPOSE.replace("127.0.0.1:15499:5432", "15499:5432"),
    );
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("every network interface"));
}

#[test]
fn validate_refuses_a_snapshot_that_looks_like_an_option() {
    let fixture = Fixture::valid();
    fixture.write(
        "restoreproof.yaml",
        &CONFIG.replace(
            "  type: local\n  path: ./backup",
            "  type: restic\n  repository: ./backup\n  snapshot: \"--password-command=id\"",
        ),
    );
    cli()
        .args(["validate", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("argument injection"));
}

#[test]
fn validate_emits_machine_readable_output() {
    let fixture = Fixture::valid();
    let output = cli()
        .args([
            "validate",
            "--config",
            fixture.config().to_str().unwrap(),
            "--format",
            "json",
        ])
        .assert()
        .success();

    let document: serde_json::Value =
        serde_json::from_slice(&output.get_output().stdout).expect("valid JSON");
    assert_eq!(document["valid"], serde_json::Value::Bool(true));
    assert_eq!(document["project"], "fixture-app");
}

// --- plan ---------------------------------------------------------------

#[test]
fn plan_describes_the_drill_without_running_it() {
    let fixture = Fixture::valid();
    cli()
        .args(["plan", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Nothing below has been executed"))
        .stdout(predicate::str::contains("database-healthy"));

    assert!(
        !fixture.path("reports").exists(),
        "plan must not create anything"
    );
}

#[test]
fn plan_names_the_source_of_a_secret_but_never_its_value() {
    let fixture = Fixture::valid();
    fixture.write("restic-password", "super-secret-repository-password\n");
    fixture.write(
        "restoreproof.yaml",
        &CONFIG.replace(
            "  type: local\n  path: ./backup",
            "  type: restic\n  repository: ./backup\n  password_file: ./restic-password",
        ),
    );

    let output = cli()
        .args(["plan", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout).into_owned();

    assert!(stdout.contains("read from the file"), "{stdout}");
    assert!(
        !stdout.contains("super-secret-repository-password"),
        "the plan leaked a secret: {stdout}"
    );
}

// --- run ----------------------------------------------------------------

#[test]
fn run_without_docker_fails_clearly_and_still_writes_a_report() {
    if fixtures::docker_available() {
        // The dedicated drill test covers the case where Docker is present.
        return;
    }

    let fixture = Fixture::valid();
    cli()
        .args(["run", "--config", fixture.config().to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("Docker"));

    let reports: Vec<_> = std::fs::read_dir(fixture.path("reports"))
        .expect("the report directory should exist")
        .flatten()
        .collect();
    assert_eq!(
        reports.len(),
        2,
        "a JSON and a Markdown report are expected"
    );
}

#[test]
fn a_dry_run_touches_nothing() {
    let fixture = Fixture::valid();
    cli()
        .args([
            "run",
            "--config",
            fixture.config().to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("SKIP"));
}

// --- report -------------------------------------------------------------

#[test]
fn a_report_can_be_verified_and_tampering_is_detected() {
    let fixture = Fixture::valid();
    cli()
        .args([
            "run",
            "--config",
            fixture.config().to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success();

    let report = std::fs::read_dir(fixture.path("reports"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("a JSON report");

    cli()
        .args(["report", report.to_str().unwrap(), "--verify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Integrity verified"));

    let text = std::fs::read_to_string(&report).unwrap();
    std::fs::write(&report, text.replace("\"SKIPPED\"", "\"PASSED\"")).unwrap();

    cli()
        .args(["report", report.to_str().unwrap(), "--verify"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("integrity check FAILED"));
}

#[test]
fn a_corrupt_report_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("broken.json");
    std::fs::write(&path, b"{ not a report").unwrap();

    cli()
        .args(["report", path.to_str().unwrap()])
        .assert()
        .code(7)
        .stderr(predicate::str::contains(
            "not a valid RestoreProof JSON report",
        ));
}

// --- version and usage ---------------------------------------------------

#[test]
fn version_documents_every_exit_code() {
    let output = cli().arg("version").assert().success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout).into_owned();

    assert!(stdout.contains("RestoreProof Core"));
    for code in 0..=7 {
        assert!(
            stdout.contains(&format!("  {code}  ")),
            "exit code {code} is not documented"
        );
    }
}

#[test]
fn help_succeeds_and_an_unknown_command_is_a_usage_error() {
    cli().arg("--help").assert().success();
    cli().arg("definitely-not-a-command").assert().code(7);
}

// --- secret handling -----------------------------------------------------

#[test]
fn a_secret_from_the_environment_never_reaches_a_report() {
    let fixture = Fixture::valid();
    fixture.write(
        "checks.yaml",
        &CHECKS.replace(
            "  - id: dump-restored",
            r#"  - id: query-orders
    name: "Orders are present"
    type: sql
    required: false
    dsn_env: RESTOREPROOF_TEST_DSN
    query: "SELECT count(*) FROM orders"
    min_value: 1

  - id: dump-restored"#,
        ),
    );

    let secret = "postgres://app:TopSecretPassword123@127.0.0.1:1/app";
    // The drill fails (nothing is listening on port 1); what matters here is
    // what ends up in the report, not the exit code.
    let _ = cli()
        .env("RESTOREPROOF_TEST_DSN", secret)
        .args(["run", "--config", fixture.config().to_str().unwrap()])
        .assert();

    for entry in std::fs::read_dir(fixture.path("reports"))
        .unwrap()
        .flatten()
    {
        let text = std::fs::read_to_string(entry.path()).unwrap();
        assert!(
            !text.contains("TopSecretPassword123"),
            "a credential leaked into {}",
            entry.path().display()
        );
    }
}
