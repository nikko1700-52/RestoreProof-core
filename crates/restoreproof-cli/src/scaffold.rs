//! Files written by `restoreproof init`.
//!
//! The scaffold is a drill that actually runs: a small `PostgreSQL` dump, a
//! Compose file that loads it, and checks that prove the data came back. It is
//! also the shortest explanation of how the pieces fit together, so every file
//! is commented.

/// `restoreproof.yaml`.
pub const CONFIG: &str = r#"# RestoreProof scenario.
# Every field is documented in docs/configuration.md.
version: 1

project:
  name: demo-app
  description: "Local recovery drill for a PostgreSQL-backed application"
  # Recorded in the report so a drill can be tied to a release.
  # application_version: "1.4.2"

backup:
  # `local` restores from a directory your own dump job writes to.
  # `restic` and `borg` are also supported: see docs/configuration.md.
  type: local
  path: ./backup
  # Without this file the backup timestamp is guessed from modification times
  # and the report says so. Write it from your backup job to get a real RPO.
  metadata_file: ./backup/backup-metadata.json

recovery:
  compose_file: ./docker-compose.recovery.yml
  # A unique suffix is always appended, so concurrent drills never collide.
  project_name: demo-app
  startup_timeout_seconds: 180
  total_timeout_seconds: 1800
  # Destroy containers, networks and volumes when the drill ends.
  cleanup: true
  # Refuse a Compose file that joins a pre-existing network.
  network_isolated: true
  wait_for:
    - database

metrics:
  # How long a real recovery is allowed to take.
  target_rto_seconds: 900
  # How much data loss is acceptable.
  target_rpo_seconds: 86400

checks_file: ./checks.yaml

report:
  directory: ./reports
  formats:
    - json
    - markdown

# Safety switches. Every default is the conservative option; a drill has to opt
# in to anything that widens its blast radius. See docs/security.md.
security:
  # `command` and `script` checks run local programs.
  allow_command_checks: true
  # HTTP checks may only target loopback.
  allow_external_http_targets: false
  # SQL checks may only target loopback.
  allow_external_sql_targets: false
  # The Compose file may only publish ports on 127.0.0.1.
  allow_published_ports: false
  # Locations outside this directory that the configuration may reference.
  allow_external_paths: []
"#;

/// `checks.yaml`.
pub const CHECKS: &str = r#"# What this drill proves. A drill without checks proves nothing.
version: 1

defaults:
  timeout_seconds: 20
  # Services take a moment to come up; retries absorb that without hiding a
  # failure, because the number of attempts is recorded in the report.
  retry:
    attempts: 15
    delay_seconds: 2

checks:
  - id: dump-restored
    name: "The database dump was restored to disk"
    type: file
    required: true
    # Relative to the directory the backup was restored into.
    base: restore
    path: dump.sql
    min_size_bytes: 100

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
    # The connection string is read from this environment variable, so it never
    # sits in a file you commit.
    dsn_env: RESTOREPROOF_DEMO_DATABASE_URL
    query: "SELECT count(*) FROM orders"
    min_value: 1

  - id: latest-order
    name: "The most recent order came back"
    type: sql
    required: false
    dsn_env: RESTOREPROOF_DEMO_DATABASE_URL
    query: "SELECT customer FROM orders ORDER BY id DESC LIMIT 1"
    expect_value: "ada"
"#;

/// `docker-compose.recovery.yml`.
pub const COMPOSE: &str = r#"# The isolated environment the backup is restored into.
#
# RestoreProof audits this file before starting anything and refuses privileged
# containers, the Docker socket, host namespaces and mounts that leave this
# directory. See docs/security.md.
services:
  database:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: app
      POSTGRES_USER: restoreproof
      # A throwaway credential for a throwaway container. Never reuse a
      # production password here.
      POSTGRES_PASSWORD: restoreproof-local-drill
    volumes:
      # RestoreProof injects this variable; it points at the restored backup.
      # PostgreSQL runs every .sql file found here on first start.
      - ${RESTOREPROOF_RESTORE_DIR}:/docker-entrypoint-initdb.d:ro
    # Bound to loopback on purpose: the restored copy of production data is
    # reachable from this machine only. Publishing on 0.0.0.0 is refused.
    ports:
      - "127.0.0.1:15432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U restoreproof -d app"]
      interval: 2s
      timeout: 3s
      retries: 30
"#;

/// `backup/dump.sql`: stands in for whatever your backup job produces.
pub const DUMP: &str = r#"-- Stand-in for a real dump. Replace ./backup with the directory your own
-- backup job writes to, and this drill starts testing your data instead.
CREATE TABLE orders (
    id          serial PRIMARY KEY,
    customer    text NOT NULL,
    total_cents integer NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO orders (customer, total_cents) VALUES
    ('grace', 1299),
    ('alan',  4500),
    ('ada',   8800);
"#;

/// `.gitignore` for the scenario directory.
pub const GITIGNORE: &str = "reports/\n";

/// `backup/backup-metadata.json`, stamped with the current time.
#[must_use]
pub fn metadata(now: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{{\n  \"created_at\": \"{}\",\n  \"snapshot_id\": \"demo-seed\"\n}}\n",
        now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}
