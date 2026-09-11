# Configuration reference

A scenario is two YAML documents: `restoreproof.yaml` (what to restore and
where) and `checks.yaml` (what to prove).

Both are **strict**. An unknown field is an error, not a silently ignored line.
A typo like `startup_timeout_second` would otherwise leave you with a drill that
passes for the wrong reason.

All paths are resolved relative to the **directory containing the configuration
file**, never to your current working directory, so a drill behaves the same
from any shell.

---

## `restoreproof.yaml`

```yaml
version: 1

project:
  name: example-app
  description: "Nightly recovery drill"
  application_version: "2026.1.3"

backup:
  type: restic
  repository: ./fixtures/restic-repository
  password_file: ./fixtures/restic-password
  snapshot: latest

recovery:
  compose_file: ./docker-compose.recovery.yml
  project_name: example-app
  startup_timeout_seconds: 180
  total_timeout_seconds: 1800
  cleanup: true
  network_isolated: true
  wait_for:
    - database
  environment:
    APP_PROFILE: recovery

metrics:
  target_rto_seconds: 1800
  target_rpo_seconds: 14400
  rpo_reference_file: ./backup-metadata.json

checks_file: ./checks.yaml

report:
  directory: ./reports
  formats:
    - json
    - markdown

security:
  allow_command_checks: true
  allow_external_http_targets: false
  allow_external_sql_targets: false
  allow_published_ports: false
  allow_external_paths: []
  max_command_output_bytes: 65536
```

### `version` *(required)*

Schema version. Must be `1`. A future incompatible schema will bump it rather
than reinterpret your file.

### `project` *(required)*

| Field | Type | Default | Notes |
|---|---|---|---|
| `name` | string | — | 1–63 characters from `[a-z0-9_-]`, starting with a letter or digit. Used as the Compose project prefix, which is why the alphabet is narrow. |
| `description` | string | — | Free text, shown in reports. |
| `application_version` | string | — | Recorded in the report, so a drill can be tied to a release. |

### `backup` *(required)*

Selected by `type`, with the remaining fields flat.

#### `type: local`

A directory or file your own dump job writes to.

| Field | Type | Default | Notes |
|---|---|---|---|
| `path` | path | — | Directory or single file. |
| `metadata_file` | path | — | JSON file: `{"created_at": "<RFC 3339>", "snapshot_id": "..."}`. |

Without `metadata_file`, the backup timestamp is derived from the most recent
modification time under `path`. That is an approximation, the report says so,
and the RPO is reported accordingly. Writing a metadata file from your backup
job is what turns the RPO into a real measurement.

Only regular files and directories are restored. Symbolic links, sockets, FIFOs
and device nodes are skipped and reported — see [security.md](security.md).

#### `type: restic`

| Field | Type | Default | Notes |
|---|---|---|---|
| `repository` | string | — | Anything `restic -r` accepts. A path is confined like any other path; a remote URL (`s3:`, `sftp:`, …) is allowed and warned about, because it means network egress. |
| `password_file` | path | — | Mutually exclusive with `password_env`. |
| `password_env` | string | — | Name of an environment variable. |
| `snapshot` | string | `latest` | Snapshot id, or `latest`. |
| `include_path` | string | — | Restrict the restore to this path inside the snapshot. |

Only `restic snapshots` and `restic restore` are ever run. Never `forget`, never
`prune`.

If neither `password_file` nor `password_env` is set, `RESTIC_PASSWORD` and
`RESTIC_PASSWORD_FILE` are forwarded from your environment if present.
`RESTIC_PASSWORD_COMMAND` is deliberately **not** forwarded: it would make a
drill execute a command RestoreProof never audited.

#### `type: borg`

Experimental.

| Field | Type | Default |
|---|---|---|
| `repository` | string | — |
| `passphrase_file` | path | — |
| `passphrase_env` | string | — |
| `archive` | string | `latest` |

Borg reports archive timestamps without a UTC offset; they are interpreted in
this machine's local timezone, and the report says so.

### `recovery` *(required)*

| Field | Type | Default | Notes |
|---|---|---|---|
| `compose_file` | path | — | Must be inside the project directory. Audited before anything starts. |
| `project_name` | string | `project.name` | A unique suffix is always appended, so concurrent drills never collide. |
| `startup_timeout_seconds` | integer | `180` | How long services may take to become ready. |
| `total_timeout_seconds` | integer | `3600` | Ceiling for the whole drill. Must be ≥ `startup_timeout_seconds`. |
| `cleanup` | boolean | `true` | Destroy containers, networks and volumes at the end. |
| `network_isolated` | boolean | `true` | Refuse a Compose file that joins an `external: true` network. |
| `wait_for` | list | every service | Services that must be ready before checks start. |
| `environment` | map | `{}` | Extra variables for `docker compose`. |

`environment` may not set `PATH`, `LD_PRELOAD`, `LD_LIBRARY_PATH`,
`DOCKER_HOST`, `DOCKER_CONFIG` and similar: a configuration file must not be
able to change which program runs or which daemon is contacted. The
`RESTOREPROOF_` prefix is reserved.

#### Variables injected into Compose

| Variable | Value |
|---|---|
| `RESTOREPROOF_RESTORE_DIR` | Directory the backup was restored into. |
| `RESTOREPROOF_WORKDIR` | Root of the temporary workspace. |

These are the **only** variables accepted as bind-mount sources, because any
other variable's value is not audited.

#### Readiness

A service is ready when it is `running` and, if its image declares a
healthcheck, reports `healthy`. A container that has exited with code `0` also
counts as ready, which is how seed and migration containers are supported.

The state must hold across two consecutive polls one second apart. A service
that crashes immediately is briefly reported as `running`, and one poll would
mistake that for success.

A service that exits non-zero, reports `unhealthy` or is `dead` fails the drill
straight away, with the tail of the container logs.

### `metrics`

| Field | Type | Notes |
|---|---|---|
| `target_rto_seconds` | integer | Recovery Time Objective. |
| `target_rpo_seconds` | integer | Recovery Point Objective. |
| `rpo_reference_file` | path | JSON overriding the timestamps used for the RPO. |

**Measured RTO** is the time between the start of the drill and the moment every
*required* check had passed. If a required check never passes, there is no RTO
and the report says `RTO_UNKNOWN`.

**Estimated RPO** is the difference between a reference instant (the start of
the drill, by default) and the creation time of the restored backup. Without a
backup timestamp it is `RPO_UNKNOWN` — never guessed, never zero.

`rpo_reference_file` accepts:

```json
{
  "backup_timestamp": "2026-01-15T02:00:00Z",
  "reference_timestamp": "2026-01-15T10:00:00Z"
}
```

Both fields are optional. Use it when the backup source cannot report a
timestamp itself, or to compare against a fixed instant rather than "now".

Objectives do **not** decide the exit code: only checks do. A drill with
`RPO_FAIL` and all checks passing still exits `0`, and the report says both
things.

### `checks_file`

Path to `checks.yaml`. Must be inside the project directory. Defaults to
`./checks.yaml`.

### `report`

| Field | Type | Default |
|---|---|---|
| `directory` | path | `./reports` |
| `formats` | list of `json`, `markdown` | both |

The directory is created if missing, with mode `0700`; report files are `0600`.

### `security`

Every default is the conservative option. See [security.md](security.md).

| Field | Default | Effect |
|---|---|---|
| `allow_command_checks` | `true` | Whether `command` and `script` checks may run local programs. |
| `allow_external_http_targets` | `false` | Whether HTTP checks may leave loopback. |
| `allow_external_sql_targets` | `false` | Whether SQL checks may leave loopback. |
| `allow_published_ports` | `false` | Whether the Compose file may publish on `0.0.0.0`. Loopback bindings are always allowed. |
| `allow_external_paths` | `[]` | Absolute locations outside the project that *data* paths may use. Never applies to scripts, the Compose file or the checks file. |
| `max_command_output_bytes` | `65536` | Ceiling for captured output of one external command. |

---

## `checks.yaml`

```yaml
version: 1

defaults:
  timeout_seconds: 30
  retry:
    attempts: 1
    delay_seconds: 2

checks:
  - id: app-health
    name: "The application answers"
    description: "Shown in the report"
    type: http
    enabled: true
    required: true
    timeout_seconds: 15
    retry:
      attempts: 10
      delay_seconds: 3
    url: "http://127.0.0.1:18080/health"
    expected_status: 200
```

### Fields common to every check

| Field | Type | Default | Notes |
|---|---|---|---|
| `id` | string | — | Unique. Same alphabet as `project.name`. |
| `name` | string | `id` | Shown in reports. |
| `description` | string | — | Why this check exists. |
| `type` | string | — | `http`, `command`, `sql`, `file`, `container`, `script`. |
| `enabled` | boolean | `true` | A disabled check is reported as `SKIPPED`. |
| `required` | boolean | `true` | A required check that does not pass fails the drill. |
| `timeout_seconds` | integer | from `defaults` | Per attempt. |
| `retry` | object | from `defaults` | `attempts` includes the first try. |

Type-specific fields sit at the same level. They are documented in
[checks.md](checks.md).

### Retries

Retries absorb a service that is still warming up. They never hide a failure:
the number of attempts is recorded in the report.

A failure the environment has answered definitively — a table that does not
exist, a digest mismatch, a missing credential — is **not** retried. Repeating
it would only make the drill slower and the report less clear.

### `required` and the result

| Situation | Run status | Exit code |
|---|---|---|
| every check passed | `PASSED` | 0 |
| all required passed, some optional did not | `PARTIAL` | 0 |
| a required check failed | `FAILED` | 1 |
| a required check errored or was skipped | `ERROR` | 1 |

A required check that could not be executed is never reported as a success. An
unprovable recovery is not a proven one.
