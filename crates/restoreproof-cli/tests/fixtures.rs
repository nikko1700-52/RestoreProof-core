//! Shared helpers for the CLI integration tests.
//!
//! Every test builds a complete scenario in a temporary directory, so nothing
//! depends on the repository layout or on files left over by another test.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::must_use_candidate,
    missing_docs,
    unreachable_pub
)]

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A scenario on disk that tests can mutate before running the CLI against it.
pub struct Fixture {
    pub dir: TempDir,
}

pub const COMPOSE: &str = r#"services:
  database:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: app
      POSTGRES_USER: restoreproof
      POSTGRES_PASSWORD: restoreproof-local-drill
    volumes:
      - ${RESTOREPROOF_RESTORE_DIR}:/docker-entrypoint-initdb.d:ro
    ports:
      - "127.0.0.1:15499:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U restoreproof -d app"]
      interval: 2s
      timeout: 3s
      retries: 30
"#;

pub const CONFIG: &str = r#"version: 1

project:
  name: fixture-app
  description: "Integration test scenario"

backup:
  type: local
  path: ./backup

recovery:
  compose_file: ./docker-compose.recovery.yml
  project_name: fixture-app
  startup_timeout_seconds: 120
  total_timeout_seconds: 600
  cleanup: true
  network_isolated: true
  wait_for:
    - database

metrics:
  target_rto_seconds: 600
  target_rpo_seconds: 86400

checks_file: ./checks.yaml

report:
  directory: ./reports
  formats:
    - json
    - markdown
"#;

pub const CHECKS: &str = r#"version: 1

defaults:
  timeout_seconds: 10
  retry:
    attempts: 15
    delay_seconds: 2

checks:
  - id: dump-restored
    name: "The dump was restored"
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
"#;

pub const DUMP: &str = "CREATE TABLE orders (id serial PRIMARY KEY, customer text NOT NULL);\n\
                        INSERT INTO orders (customer) VALUES ('grace'), ('alan'), ('ada');\n";

impl Fixture {
    /// Build a valid scenario.
    pub fn valid() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let fixture = Self { dir };
        fixture.write("restoreproof.yaml", CONFIG);
        fixture.write("checks.yaml", CHECKS);
        fixture.write("docker-compose.recovery.yml", COMPOSE);
        fixture.write("backup/dump.sql", DUMP);
        fixture
    }

    /// Write a file, creating parent directories.
    pub fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, contents).expect("write fixture file");
        path
    }

    /// Absolute path inside the fixture.
    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    /// The scenario's configuration file.
    pub fn config(&self) -> PathBuf {
        self.path("restoreproof.yaml")
    }

    /// Root of the fixture.
    pub fn root(&self) -> &Path {
        self.dir.path()
    }
}

/// A CLI invocation with a predictable environment.
///
/// `env_clear` matters: the tests assert on what happens when a credential is
/// absent, and a developer's shell must not be able to change the result.
pub fn cli() -> assert_cmd::Command {
    let mut command = assert_cmd::Command::cargo_bin("restoreproof").expect("binary");
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("NO_COLOR", "1");
    command
}

/// Whether a Docker daemon is reachable, for tests that need a real drill.
pub fn docker_available() -> bool {
    std::process::Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
