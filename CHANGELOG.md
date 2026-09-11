# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Before 1.0, minor versions may contain breaking changes to the configuration
schema. The **exit codes** are already treated as a stable contract.

## [Unreleased]

Nothing yet.

## [0.1.0] — 2026-09-11

First release. Early-stage software: suitable for local and CI recovery drills,
not a replacement for a backup platform.

### Added

**Command line**

* `restoreproof init` — scaffold a drill that actually runs
* `restoreproof validate` — strict configuration check that reports every
  problem at once
* `restoreproof plan` — show what a drill would do, executing nothing
* `restoreproof run` — restore, start, check, measure, report
* `restoreproof check` — run only the checks, against an environment that is
  already running
* `restoreproof report` — display a report, and `--verify` its integrity digest
* `restoreproof version` — version, build, platform and the exit-code table
* global options: `--config`, `--format`, `--output`, `--verbose`, `--quiet`,
  `--keep-environment`, `--no-cleanup`, `--timeout`, `--dry-run`, `--no-color`
* documented exit codes 0–7, treated as a stable contract

**Backup sources**

* `local` — a directory or file, with an optional metadata file for an accurate
  backup timestamp
* `restic` — read-only (`snapshots` and `restore` only)
* `borg` — experimental

**Checks**

* `file` — presence, size bounds, SHA-256
* `container` — service state, healthcheck, published port reachability
* `http` — method, status, body content, headers from the environment
* `sql` — read-only PostgreSQL queries with row and value assertions
* `command` — a local program, `argv` only
* `script` — a script versioned inside the project
* per-check timeouts and retries; definitive failures are not retried

**Measurement and reporting**

* measured RTO and estimated RPO, compared against configured objectives
* `RTO_UNKNOWN` / `RPO_UNKNOWN` when a value cannot be determined — never
  guessed
* JSON, Markdown and terminal reports
* SHA-256 integrity digest over the canonical JSON, verifiable with
  `report --verify`
* reports written `0600` inside a `0700` directory

**Security**

* no shell: every external program runs from an `argv` array
* argument-injection refusal for values handed to external tools
* child processes start from an empty environment with closed stdin, bounded
  time and bounded output
* path confinement with symlink-aware canonicalisation; executable content
  cannot be allowlisted outside the project
* read-only SQL enforced by statement scanning, a `READ ONLY` transaction and a
  server-side statement timeout
* loopback-only HTTP and SQL targets by default; proxies ignored, redirects
  re-validated
* Compose auditing: privileged containers, the Docker socket, host namespaces,
  dangerous capabilities, host-path mounts, unaudited interpolation and
  `0.0.0.0` port publishing are refused
* secrets from files or environment variables only, never from YAML; never on a
  command line; redacted from every log, message and report
* guaranteed teardown of the recovery environment on every path, including
  panics and timeouts
* `unsafe` forbidden workspace-wide; `unwrap`/`expect`/`panic` denied in library
  code

**Examples**

* `postgres-local` — a passing drill and a deliberately failing one, needing
  nothing but Docker
* `postgres-restic` — the same drill behind restic
* `docker-compose-app` — a web tier restored and probed over HTTP

[Unreleased]: https://github.com/nikko1700-52/RestoreProof-core/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nikko1700-52/RestoreProof-core/releases/tag/v0.1.0
