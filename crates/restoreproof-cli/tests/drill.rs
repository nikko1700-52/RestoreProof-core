//! Full recovery drills against a real Docker daemon.
//!
//! These are the tests that exercise what the product actually claims: restore
//! a dump, start `PostgreSQL` from it, query the restored data, measure the
//! result and destroy everything.
//!
//! They need a reachable Docker daemon with the Compose v2 plugin. When there
//! is none they return early with a printed note rather than failing, so that
//! `cargo test` stays usable on a machine without Docker. CI runs them for
//! real: see `.github/workflows/ci.yml`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr
)]

mod fixtures;

use fixtures::{Fixture, cli, docker_available};

fn skip(reason: &str) -> bool {
    eprintln!("skipping drill test: {reason}");
    true
}

/// Connection string for a fixture database published on loopback.
fn dsn(port: u16) -> String {
    format!("postgres://restoreproof:restoreproof-local-drill@127.0.0.1:{port}/app")
}

/// A scenario whose SQL checks assert the restored data.
///
/// Cargo runs integration tests concurrently, so each drill gets its own
/// Compose project name *and* its own host port: two drills binding the same
/// loopback port would fail to start for a reason that has nothing to do with
/// what is being tested.
fn drill_fixture(project: &str, port: u16) -> Fixture {
    let fixture = Fixture::valid();
    fixture.write(
        "docker-compose.recovery.yml",
        &fixtures::COMPOSE.replace("15499", &port.to_string()),
    );
    fixture.write(
        "restoreproof.yaml",
        &fixtures::CONFIG.replace("fixture-app", project),
    );
    fixture.write(
        "checks.yaml",
        r#"version: 1

defaults:
  timeout_seconds: 20
  retry:
    attempts: 30
    delay_seconds: 2

checks:
  - id: dump-restored
    name: "The dump was restored to disk"
    type: file
    required: true
    base: restore
    path: dump.sql
    min_size_bytes: 10

  - id: database-healthy
    name: "PostgreSQL reports healthy"
    type: container
    required: true
    service: database
    state: healthy

  - id: orders-present
    name: "The orders table contains data"
    type: sql
    required: true
    dsn_env: RESTOREPROOF_TEST_DSN
    query: "SELECT count(*) FROM orders"
    min_value: 1

  - id: latest-order
    name: "The most recent order came back"
    type: sql
    required: true
    dsn_env: RESTOREPROOF_TEST_DSN
    query: "SELECT customer FROM orders ORDER BY id DESC LIMIT 1"
    expect_value: "ada"
"#,
    );
    fixture
}

/// Containers still alive for a Compose project name prefix.
fn containers_for(prefix: &str) -> Vec<String> {
    let output = std::process::Command::new("docker")
        .args([
            "ps",
            "--all",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .expect("docker ps");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn a_successful_drill_passes_measures_and_cleans_up() {
    if !docker_available() && skip("no Docker daemon is reachable") {
        return;
    }

    let fixture = drill_fixture("fixture-pass", 15491);
    let assertion = cli()
        .env("RESTOREPROOF_TEST_DSN", dsn(15491))
        .args(["run", "--config", fixture.config().to_str().unwrap()])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("PASS"),
        "the drill should have passed:\n{stdout}"
    );

    // The report must carry a measured RTO and a verifiable digest.
    let report_path = std::fs::read_dir(fixture.path("reports"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("a JSON report");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).unwrap()).unwrap();

    assert_eq!(report["run"]["status"], "PASSED");
    assert_eq!(report["summary"]["failed"], 0);
    assert!(
        report["objectives"]["rto"]["measured_seconds"].is_number(),
        "the RTO should have been measured: {}",
        report["objectives"]["rto"]
    );
    assert_eq!(report["objectives"]["rto"]["outcome"], "PASS");
    assert_eq!(report["recovery"]["environment_cleaned_up"], true);

    cli()
        .args(["report", report_path.to_str().unwrap(), "--verify"])
        .assert()
        .success();

    assert!(
        containers_for("fixture-pass-rp-").is_empty(),
        "the recovery environment must have been destroyed"
    );
}

#[test]
fn a_failing_drill_reports_the_failure_and_still_cleans_up() {
    if !docker_available() && skip("no Docker daemon is reachable") {
        return;
    }

    let fixture = drill_fixture("fixture-fail", 15492);
    // A table that was never in the backup: the drill must say so.
    fixture.write(
        "checks.yaml",
        &std::fs::read_to_string(fixture.path("checks.yaml"))
            .unwrap()
            .replace("FROM orders\"", "FROM invoices\""),
    );

    cli()
        .env("RESTOREPROOF_TEST_DSN", dsn(15492))
        .args(["run", "--config", fixture.config().to_str().unwrap()])
        .timeout(std::time::Duration::from_secs(600))
        .assert()
        .code(1);

    assert!(
        containers_for("fixture-fail-rp-").is_empty(),
        "the environment must be destroyed after a failure too"
    );
}

#[test]
fn an_environment_that_never_becomes_ready_times_out_and_is_destroyed() {
    if !docker_available() && skip("no Docker daemon is reachable") {
        return;
    }

    let fixture = Fixture::valid();
    fixture.write(
        "restoreproof.yaml",
        &std::fs::read_to_string(fixture.config())
            .unwrap()
            .replace("fixture-app", "fixture-stuck")
            .replace(
                "startup_timeout_seconds: 120",
                "startup_timeout_seconds: 15",
            ),
    );
    // A healthcheck that can never pass.
    fixture.write(
        "docker-compose.recovery.yml",
        &fixtures::COMPOSE
            .replace("15499:5432", "15493:5432")
            .replace(
                r#"test: ["CMD-SHELL", "pg_isready -U restoreproof -d app"]"#,
                r#"test: ["CMD-SHELL", "exit 1"]"#,
            ),
    );

    let assertion = cli()
        .args(["run", "--config", fixture.config().to_str().unwrap()])
        .timeout(std::time::Duration::from_secs(300))
        .assert()
        .failure();

    let code = assertion.get_output().status.code().unwrap_or(-1);
    assert!(
        code == 5 || code == 1,
        "expected a timeout (5) or a failed drill (1), got {code}"
    );

    assert!(
        containers_for("fixture-stuck-rp-").is_empty(),
        "a stuck environment must still be destroyed"
    );
}
