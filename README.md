# RestoreProof Core

***English** · [Français](README.fr.md)*

**Stop saying your backups work. Prove your application can actually come back.**

RestoreProof runs a *recovery drill*: it restores a backup into an isolated
environment, starts your services from it, checks that the application answers
and that the data is really there, measures how long it took, and writes a
report you can hand to someone.

A backup job that exits `0` tells you a file was written. It does not tell you
that the dump is complete, that the schema still loads, that the application
starts against it, or how long any of that takes. That gap is what this tool
closes.

> **Status: early.** Version 0.1.0. The core is tested and the security model is
> deliberate, but this is a young project. It is suitable for local and CI
> recovery drills. It is not a backup product, it does not replace one, and you
> should try it on something you do not care about first.

---

## What a run looks like

```console
$ export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'
$ restoreproof run --config examples/postgres-local/restoreproof.yaml

PASS PASSED  postgres-local · 2026-09-11 15:03:29Z · 4.9 s

  ok   The dump was restored to di…     0 ms  `dump.sql` is present (806 bytes)
  ok   PostgreSQL reports healthy     108 ms  service `database` is running and healthy
  ok   The orders table contains d…    78 ms  query returned 1 row(s) as expected
  ok   The customers table contain…    70 ms  query returned 1 row(s) as expected
  ok   The most recent order came …    74 ms  query returned 1 row(s) as expected

  Checks   5 passed, 0 failed, 0 error, 0 skipped (of 5)
  RTO      4.3 s (target 10m 00s)  RTO_PASS
  RPO      12m 58s (target 24h 00m 00s)  RPO_PASS
  Backup   local · unknown snapshot · 806 B

  Report   examples/postgres-local/reports/20260911T150329Z-postgres-local-45efee6c.json
  Report   examples/postgres-local/reports/20260911T150329Z-postgres-local-45efee6c.md
```

<sub>Real output from `examples/postgres-local`, warnings elided. A recorded
terminal session belongs here; adding one is tracked in
[ROADMAP.md](ROADMAP.md).</sub>

Exit code `0` means every required check passed. Exit code `1` means your
recovery does not work — which is the answer you actually wanted to know.

## Try it in three commands

You need Rust and Docker with the Compose v2 plugin.

```bash
git clone https://github.com/nikko1700-52/RestoreProof-core
cd RestoreProof-core

export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'
cargo run -- run --config examples/postgres-local/restoreproof.yaml
```

Then watch it catch a real failure:

```bash
cargo run -- run --config examples/postgres-local/restoreproof.failing.yaml
# FAILED, exit code 1: the invoices table is not in the backup
```

To start from scratch on your own project:

```bash
cargo run -- init my-drill
cargo run -- plan --config my-drill/restoreproof.yaml
```

## What it checks

| Check | Answers |
|---|---|
| `file` | Is the dump actually there, the right size, the right digest? |
| `container` | Did the service start, and does its healthcheck pass? |
| `http` | Does the application answer, with the content you expect? |
| `sql` | **Is the data there?** Read-only queries with assertions on rows and values. |
| `command` | Whatever your own tooling can assert. |
| `script` | A script versioned in your project, passing or failing by exit code. |

### Where the results go

| Format | For |
|---|---|
| Terminal | the person running it |
| JSON | the canonical record; the integrity digest covers it |
| Markdown | pasting into a ticket or sending to a customer |
| **JUnit XML** | your CI's test reporter — each check appears as a test case |
| **Prometheus** | `node_exporter`'s textfile collector — alert on recovery |

Monitoring a nightly drill takes no server and no account:

```promql
# Recovery is broken.
restoreproof_drill_success == 0

# No drill has run for a day. Silence is not success.
time() - restoreproof_drill_completed_timestamp_seconds > 86400
```

And `restoreproof diff` answers the question a single report cannot:

```console
$ restoreproof diff reports/monday.json reports/tuesday.json

  Status   PASSED → ERROR   WORSE

  Checks
    ! The orders table contains d… PASSED → ERROR

  Recovery got worse between these two drills.
```

It exits `1` on a regression, so it works as a CI gate on its own.

And it measures:

* **RTO** — how long the recovery actually took, compared with your objective.
* **RPO** — how old the restored data is, compared with your objective.

When a value cannot be determined, the report says `UNKNOWN`. It is never
guessed and never quietly defaulted to zero.

## Backup sources

| Type | Requires | Notes |
|---|---|---|
| `local` | nothing | a directory or file your own dump job writes to |
| `restic` | `restic` | read-only: `snapshots` and `restore` only |
| `borg` | `borg` | experimental |

Object stores, hypervisor snapshots and vendor APIs are out of scope here; see
[PREMIUM.md](PREMIUM.md).

## Security model

A recovery drill restores production data and starts containers from it. Done
carelessly, the drill *is* the incident. The design assumes that and pushes
back:

| Risk | What the tool does |
|---|---|
| Command injection | No shell, ever. Programs run from an `argv` array. |
| Argument injection | Values that an external tool would read as options are refused at validation time. |
| Path traversal | Every configured path is canonicalised — symlinks included — and confined to the project directory. |
| Destructive "checks" | SQL runs in a `READ ONLY` transaction with a server-side timeout, always rolled back, and only single `SELECT`/`WITH` statements are accepted. |
| Request forgery | HTTP checks target loopback only unless you opt in. Proxies are ignored, redirects re-validated. |
| Container escape | The Compose file is audited before anything starts: privileged containers, the Docker socket, host namespaces, dangerous capabilities and host-path mounts are refused. |
| Data exposure | Ports published on `0.0.0.0` are refused; loopback is fine. Reports are `0600`, restored data lives in a `0700` workspace. |
| Leftovers after `Ctrl-C` | `SIGINT` and `SIGTERM` are caught: the environment is destroyed *before* the process exits, and anything that could not be destroyed is named. |
| Secret leakage | Secrets come from files or environment variables, never from YAML. They never reach `argv`, a log, or a report. |
| Leftover environments | Teardown runs on every path, including panics and timeouts. |

`unsafe` code is forbidden across the whole workspace, and `unwrap`/`expect`/
`panic` are denied in library code.

The full threat model, including what is **not** covered, is in
[SECURITY.md](SECURITY.md) and [docs/security.md](docs/security.md).

## How it fits together

```
restoreproof-cli        argument parsing and terminal output only
      │
restoreproof-runner     restore → start → wait → check → tear down → report
      │
      ├── restoreproof-storage   BackupSource: local, restic, borg
      ├── restoreproof-checks    six check executors
      ├── restoreproof-config    strict YAML, validation, path/SQL/URL/Compose guards
      ├── restoreproof-report    JSON, Markdown, terminal
      └── restoreproof-core      statuses, exit codes, secrets, redaction, RTO/RPO,
                                 sandboxed process execution
```

Nothing but the CLI depends on the CLI. A scheduler, an HTTP API or a web
console would sit where `restoreproof-cli` sits and reuse everything below it
unchanged. See [docs/architecture.md](docs/architecture.md).

## Documentation

* [Getting started](docs/getting-started.md)
* [Configuration reference](docs/configuration.md)
* [Check types](docs/checks.md)
* [Running drills in production](docs/operations.md) — scheduling, retention, alerting
* [Security](docs/security.md)
* [Architecture](docs/architecture.md)
* [Troubleshooting](docs/troubleshooting.md)

## Exit codes

| Code | Meaning |
|---|---|
| 0 | every required check passed |
| 1 | at least one required check failed |
| 2 | invalid configuration |
| 3 | a required external dependency is missing |
| 4 | the backup could not be restored |
| 5 | a timeout expired |
| 6 | internal error |
| 7 | incorrect command line usage |

These are a stable contract: CI pipelines are expected to branch on them.

## Limits

Worth knowing before you rely on it:

* Recovery environments are **Docker Compose** only. No Kubernetes, VMware or
  Proxmox.
* One scenario at a time, on one machine. No scheduler, no central history, no
  web interface.
* The Compose audit is **static**. It reasons about the Compose file, not about
  what your images do at runtime. It is a guardrail, not a container sandbox.
* Report digests detect accidental modification. They are **not signatures**.
* Checks run sequentially, one scenario at a time. Nothing is parallelised.
* BorgBackup support is experimental, and the restic example is not exercised
  by CI (CI does not install restic).
* Tested on Linux (Debian/Ubuntu). Other platforms are unverified.

## Running it for real

A drill you run by hand once proves the procedure. A drill that runs every night
proves the backups. [docs/operations.md](docs/operations.md) covers the second:
systemd timers, where reports go and for how long, alert rules that also fire
when the drill goes *silent*, and a checklist to work through before trusting
any of it — starting with "make it fail on purpose and confirm it goes red".

## Contributing

Issues and pull requests are welcome — especially bug reports with a scenario
that reproduces them. Start with [CONTRIBUTING.md](CONTRIBUTING.md).

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Licence

[Apache-2.0](LICENSE).

A commercial edition is being built, separately and privately, for the things
deliberately left out of this one — central scheduling, multi-tenant history,
signed reports, Kubernetes and hypervisor environments. It depends on these
crates as published; it does not fork them, and nothing moves out of here to
make room for it. What is in scope for it, and why, is written down in
[PREMIUM.md](PREMIUM.md). Nothing in this repository is time-limited, feature-
gated or phoning home, and nothing here will start doing so.
