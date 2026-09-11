# Contributing

Thanks for looking. This is an early project and the most useful contributions
right now are bug reports with a scenario that reproduces them.

## Getting set up

```bash
git clone https://github.com/nikko1700-52/RestoreProof-core
cd RestoreProof-core
cargo build
cargo test --workspace
```

You need Rust stable (1.85+). For the full drill tests you also need Docker with
the Compose v2 plugin; without it those tests print a note and return, so
`cargo test` stays usable either way.

## Before you push

The three commands CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The lints are strict on purpose. In library code `unwrap`, `expect`, `panic`,
unchecked indexing and integer division are **denied**, and `unsafe` is
forbidden across the workspace. A tool that tells you your disaster recovery is
fine should not abort on a malformed backup.

If a lint is genuinely wrong for a piece of code, an `#[allow(...)]` with a
comment saying why is fine. A blanket allow at module level is not.

## Reporting a bug

The most useful report contains a minimal scenario that reproduces it:

* `restoreproof version`
* the `restoreproof.yaml` and `checks.yaml` (redacted — `plan` output is safe to
  paste, it never contains secrets)
* what you expected and what happened
* the JSON report, if one was produced

For **security** issues, do not open a public issue. See
[SECURITY.md](SECURITY.md).

## Proposing a feature

Open an issue first, especially for anything larger than a bug fix. Two things
worth checking before you write code:

**Is it in scope?** The open-source edition is deliberately a single-machine,
single-scenario tool. Web interfaces, schedulers, multi-tenancy, Kubernetes and
hypervisor environments are out of scope here — see [PREMIUM.md](PREMIUM.md). If
you think something on that list should be open source instead, say so; the list
is a plan, not a promise to keep things from you.

**Does it break an invariant?** These are load-bearing and each has a test:

1. `unsafe` is forbidden; `unwrap`/`expect`/`panic` are denied in library code.
2. No external program is started except through `restoreproof_core::process`.
3. No configured path is used before passing through `PathPolicy`.
4. No secret reaches `argv`, a log line, or a report.
5. A drill always produces a report, including when the restore failed.
6. The recovery environment is destroyed on every path except
   `--keep-environment`.
7. Exit codes do not change meaning.

A change that breaks one of these is a design discussion, not a patch.

## Where things live

See [docs/architecture.md](docs/architecture.md). The short version:

| Crate | Responsibility |
|---|---|
| `restoreproof-core` | statuses, exit codes, secrets, redaction, RTO/RPO, sandboxed process execution |
| `restoreproof-config` | strict YAML, validation, and most of the security boundary |
| `restoreproof-storage` | `BackupSource`: local, restic, borg |
| `restoreproof-checks` | the six check executors |
| `restoreproof-runner` | environment lifecycle and orchestration |
| `restoreproof-report` | JSON, Markdown, terminal |
| `restoreproof-cli` | argument parsing and terminal output only |

Dependencies point downward. Nothing depends on the CLI — please keep it that
way, so that a future scheduler or API can reuse everything below it.

The common extension points, documented in the architecture guide:

* a backup source implements `restoreproof_storage::source::BackupSource`;
* a check type implements `restoreproof_checks::executor::CheckExecutor`;
* an orchestrator implements `restoreproof_checks::context::EnvironmentProbe`.

## Tests

Please add one. Specifically:

* **A security control needs a test that shows it working.** If you add a
  refusal, add the case it refuses *and* a legitimate case it must still accept.
  Over-refusing is a bug too.
* **A check executor needs both a passing and a failing case**, and the failure
  message should be one a tired operator can act on.
* **Do not mock what you can run.** The process, path and redaction tests run
  real programs and real files; the drill tests run real containers.

Tests are named as sentences (`a_symlink_escaping_the_restore_directory_is_not_read`).
It reads well in output and makes an unclear failure obvious.

## Commit messages

Plain imperative subject, a body explaining *why* when it is not obvious.
Conventional Commit prefixes (`feat:`, `fix:`, `docs:`) are welcome but not
enforced.

## Documentation

If you change behaviour, update the docs in the same pull request:

* user-facing behaviour → `docs/`
* a safety switch or a refusal → `docs/security.md` **and** `SECURITY.md`
* a new field → `docs/configuration.md`
* a scope decision → `PREMIUM.md` or `ROADMAP.md`

There is a French translation of the README (`README.fr.md`). If you change
`README.md` in a way that affects what the tool does or promises, please update
it too, or say in the pull request that it needs updating — a translation that
quietly says something different from the original is worse than none.

And please keep the claims honest. No invented benchmarks, no "enterprise-ready",
no capability that is not implemented. The whole point of this project is that
its output can be trusted; the documentation is held to the same standard.

## Licence

By contributing you agree that your contribution is licensed under
[Apache-2.0](LICENSE), like the rest of the project.
