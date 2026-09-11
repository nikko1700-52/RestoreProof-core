# `postgres-restic` — restore the latest restic snapshot

Same drill as [`postgres-local`](../postgres-local), with a real backup tool in
front of it. It proves the part that matters most about restic in practice: the
repository password is usable, the snapshot is readable, and the data inside it
is complete.

## Requirements

* Docker with the Compose v2 plugin
* [`restic`](https://restic.net) on `PATH`

> **Status.** This example is written against restic's documented behaviour and
> the `restic` code path has unit tests, but the end-to-end run has not been
> executed in this repository's CI, because CI does not install restic. If it
> does not work for you, that is a bug worth reporting.

## Run it

From the repository root:

```bash
# 1. Stand in for your backup job: create a local restic repository.
./examples/postgres-restic/prepare.sh

# 2. The connection string stays out of the configuration file.
export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15433/app'

# 3. Run the drill.
cargo run -- run --config examples/postgres-restic/restoreproof.yaml
```

Expected result: `PASSED`, exit code `0`, and reports in
`examples/postgres-restic/reports/`.

Unlike `postgres-local`, the RPO here is exact: restic records when the
snapshot was taken, so the report compares a real backup time against
`metrics.target_rpo_seconds`.

## How the password is handled

`prepare.sh` generates `fixtures/restic-password` with mode `600`. It is
gitignored, and `restoreproof.yaml` refers to it by path.

The password is never placed on a command line, where it would be visible in
the host's process list: RestoreProof passes the *path* to restic through
`RESTIC_PASSWORD_FILE`, so the plaintext never enters the `restoreproof`
process either. Whatever restic prints is still filtered through the redactor
before it can reach a report.

RestoreProof only ever runs `restic snapshots` and `restic restore`. It never
runs `forget`, `prune`, or anything else that could alter the backup it is
supposed to be verifying.
