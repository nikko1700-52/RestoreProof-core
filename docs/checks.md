# Check types

A check is the unit of proof. A drill that only starts a container proves that
Docker works. The checks are what make it prove that *your recovery* works.

Six types, in rough order of how much they prove:

| Type | Proves |
|---|---|
| [`file`](#file) | the bytes are on disk |
| [`container`](#container) | the service started and its healthcheck passes |
| [`http`](#http) | the application answers, with the content you expect |
| [`sql`](#sql) | **the data is there** |
| [`command`](#command) | whatever your own tooling can assert |
| [`script`](#script) | a script in your project, passing or failing by exit code |

Fields common to all of them (`id`, `name`, `required`, `timeout_seconds`,
`retry`, …) are in [configuration.md](configuration.md#checksyaml).

---

## `file`

Asserts presence, size or digest of a restored file.

```yaml
  - id: dump-restored
    name: "The dump was restored to disk"
    type: file
    required: true
    base: restore
    path: pgdata/dump.sql
    min_size_bytes: 1048576
    max_size_bytes: 10737418240
    sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
```

| Field | Default | Notes |
|---|---|---|
| `path` | — | Relative to `base`. No `..`, no absolute paths when `base: restore`. |
| `base` | `restore` | `restore` (the restored tree) or `project` (the scenario directory). |
| `exists` | `true` | Set `false` to assert absence. |
| `min_size_bytes` | — | Catches a truncated dump. |
| `max_size_bytes` | — | |
| `sha256` | — | 64 hex characters. |

`min_size_bytes` is the single cheapest useful check you can add: a backup job
that silently wrote an empty file is a common failure, and this catches it
before anything else runs.

**Security.** The path is canonicalised and re-confined under its base, so a
symlink in the restored data cannot make the check read `/etc/shadow` into a
report.

---

## `container`

Asserts the state of a service in the recovery environment.

```yaml
  - id: database-healthy
    name: "PostgreSQL reports healthy"
    type: container
    required: true
    service: database
    state: healthy
```

| Field | Default | Notes |
|---|---|---|
| `service` | — | Service name from the Compose file. Validated against it. |
| `state` | `running` | `running`, `healthy` or `exited`. |
| `expected_exit_code` | — | Only with `state: exited`. |
| `port` | — | A published container port that must accept a TCP connection. |

`state: healthy` requires the image to declare a healthcheck. If it does not,
the check fails and the report says why rather than passing vacuously.

`state: exited` with `expected_exit_code: 0` is how you assert that a migration
or seed container finished successfully.

**Security.** Container access goes through a read-only interface. A check
cannot start, stop, or exec into a container.

---

## `http`

Probes an endpoint of the recovered application.

```yaml
  - id: app-health
    name: "The application answers on /health"
    type: http
    required: true
    method: GET
    url: "http://127.0.0.1:18080/health"
    expected_status: 200
    expect_body_contains: "\"database\":\"ok\""
    headers:
      Accept: application/json
    headers_from_env:
      Authorization: DRILL_HEALTH_TOKEN
    follow_redirects: false
```

| Field | Default | Notes |
|---|---|---|
| `method` | `GET` | `GET`, `HEAD` or `POST`. |
| `url` | — | Absolute. Loopback only by default. |
| `expected_status` | `200` | |
| `expect_body_contains` | — | Substring. |
| `headers` | `{}` | Credential headers are refused here — see below. |
| `headers_from_env` | `{}` | Header name → environment variable name. |
| `body` | — | `POST` only. |
| `follow_redirects` | `false` | Every hop is re-validated when enabled. |

`expect_body_contains` is what separates "a web server is listening" from "the
application recovered". Point it at something only the restored data can
produce.

**Security.** `Authorization`, `Cookie` and `X-API-Key` cannot be written
literally — a configuration file is meant to be committed. Use
`headers_from_env`; the value is registered for redaction before the request is
sent. Proxy environment variables are ignored, and credentials in the URL are
refused.

To reach a host other than loopback, set
`security.allow_external_http_targets: true` and understand that you are
pointing the tool at something outside the environment it just built.

---

## `sql`

Runs a read-only query against the recovered PostgreSQL database, and asserts
its result. This is the check that proves the *data* came back.

```yaml
  - id: recent-orders
    name: "Yesterday's orders are in the restored database"
    type: sql
    required: true
    dsn_env: DRILL_DATABASE_URL
    query: "SELECT count(*) FROM orders WHERE created_at > now() - interval '2 days'"
    min_value: 1
```

| Field | Notes |
|---|---|
| `dsn_env` | Name of the environment variable holding the connection string. |
| `query` | One read-only statement. |
| `expect_row_count` | Exact number of rows. |
| `min_rows` / `max_rows` | Bounds on the number of rows. |
| `expect_value` | Textual value of the first column of the first row. |
| `min_value` / `max_value` | Numeric bounds on that value. |

At least one assertion is required. A query with no assertion proves nothing,
and the configuration is refused.

### What is accepted

A single `SELECT` or `WITH` statement. Refused at validation time:

* more than one statement, including via comment smuggling
  (`SELECT 1 -- x\n; DROP TABLE orders`);
* dollar-quoting (`$$ … $$`);
* anything that writes (`INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`,
  `CREATE`, `TRUNCATE`, `SELECT … INTO`, `FOR UPDATE`, …);
* anything that changes the session (`SET`, `set_config`, …);
* anything that reaches the server filesystem or other backends
  (`pg_read_file`, `lo_import`, `pg_terminate_backend`, `pg_sleep`, …).

Keywords inside string literals and quoted identifiers are **not** flagged:
`WHERE kind = 'delete'` is fine.

### Three layers, not one

1. The scan above, at configuration time.
2. Execution inside `BEGIN READ ONLY` with a server-side `statement_timeout`,
   always rolled back.
3. **Use a database role with no write privileges.** Layers 1 and 2 are defence
   in depth, not a substitute for least privilege.

The DSN must point at loopback unless `security.allow_external_sql_targets` is
enabled. That switch exists so that a drill cannot be turned into a query
against production.

---

## `command`

Runs a local program and asserts its exit code and output.

```yaml
  - id: schema-version
    name: "The schema is at the expected migration"
    type: command
    required: true
    command: ["psql", "-tAc", "SELECT max(version) FROM schema_migrations"]
    environment:
      PGHOST: 127.0.0.1
      PGPORT: "15432"
      PGUSER: restoreproof
      PGDATABASE: app
    expected_exit_code: 0
    expect_stdout_contains: "20260115"
```

| Field | Default | Notes |
|---|---|---|
| `command` | — | **A list.** A single string is refused. |
| `workdir` | project root | Must be inside the project. |
| `expected_exit_code` | `0` | |
| `expect_stdout_contains` | — | |
| `environment` | `{}` | Added to the child's otherwise empty environment. |

`command` must be a list because RestoreProof never hands a command line to a
shell. Shell metacharacters in an argument are inert — `"; rm -rf /"` is passed
as one literal argument.

If you genuinely need shell features, be explicit about it:
`["sh", "-c", "..."]`. You then own the quoting, and the audit trail shows that
you asked for a shell.

**Security.** The child starts from an **empty** environment plus `PATH`,
`HOME`, `LANG`, `TZ`, the injected `RESTOREPROOF_*` variables and whatever you
declare. A credential in your shell does not reach the check. Output is capped
by `security.max_command_output_bytes`, stdin is closed, and the check timeout
kills the process.

---

## `script`

Runs a script versioned inside your project.

```yaml
  - id: business-invariants
    name: "Business invariants hold after recovery"
    type: script
    required: true
    path: ./drill/verify-invariants.sh
    args: ["--strict"]
    expected_exit_code: 0
```

| Field | Default |
|---|---|
| `path` | — |
| `args` | `[]` |
| `workdir` | project root |
| `expected_exit_code` | `0` |
| `environment` | `{}` |

The script receives:

| Variable | Value |
|---|---|
| `RESTOREPROOF_RESTORE_DIR` | the restored tree |
| `RESTOREPROOF_PROJECT_ROOT` | the scenario directory |
| `RESTOREPROOF_COMPOSE_PROJECT` | the Compose project name of this run |

Exit `0` to pass, anything else to fail. This is the extension point for
assertions no generic check can express — referential integrity across services,
a domain-specific invariant, a replay of a critical transaction.

**Security.** The path must be inside the project, and is re-checked at
execution time, so a symlink swapped in after validation is refused. A
world-writable script is refused outright: any local user could otherwise
change what your drill executes.

---

## Disabling code execution entirely

When a configuration comes from somewhere you do not fully control — a template
repository, a vendor, a pull request:

```yaml
security:
  allow_command_checks: false
```

`command` and `script` checks are then skipped, and a configuration that
declares one is refused at validation time rather than silently ignored.

---

## Writing checks that are worth running

1. **Assert on data, not on liveness.** "The container is up" is nearly free to
   satisfy and nearly worthless. "Yesterday's orders are present" is the claim
   your business cares about.
2. **Pick a number you would notice.** `min_value: 1` catches an empty table.
   `min_value: 1000` catches a half-restored one.
3. **Make one check fail on purpose, once.** Run the drill against a deliberately
   broken scenario and confirm it goes red. A suite that has never failed has
   not been tested.
4. **Mark the genuinely optional as optional.** `required: false` keeps a
   nice-to-have from blocking a CI pipeline while still recording it in the
   report as `PARTIAL`.
5. **Use `description`.** It ends up in the report, where the person reading it
   at 3 a.m. is not you.
