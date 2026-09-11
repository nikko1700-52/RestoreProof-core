# Getting started

## Requirements

* **Linux** (Debian/Ubuntu are what this is tested on). macOS may work;
  Windows is unverified.
* **Docker** with the **Compose v2** plugin. Check with `docker compose version`
  — the standalone `docker-compose` v1 script is not supported.
* **Rust** stable (1.85 or newer) if you are building from source.
* `restic` or `borg` only if you use those backup sources.

## Install

### From source

```bash
git clone https://github.com/nikko1700-52/RestoreProof-core
cd RestoreProof-core
cargo build --release
./target/release/restoreproof version
```

The binary is self-contained; copy it wherever you like.

### From a release

Prebuilt Linux binaries for `x86_64` and `aarch64` are attached to each GitHub
release, together with a `SHA256SUMS` file. Verify before you run it:

```bash
sha256sum --check SHA256SUMS --ignore-missing
```

## Your first drill

```bash
restoreproof init my-drill
```

That writes a complete, runnable scenario:

```
my-drill/
├── restoreproof.yaml             the scenario
├── checks.yaml                   what the drill proves
├── docker-compose.recovery.yml   the isolated environment
└── backup/
    ├── dump.sql                  a stand-in for your own backup
    └── backup-metadata.json      when that backup was taken
```

### 1. See what it would do

```bash
restoreproof plan --config my-drill/restoreproof.yaml
```

`plan` executes nothing. It prints the backup it would read, the services it
would start, the checks it would run, the objectives it would measure and which
safety switches are on. It is worth reading once before every `run` against
something that matters.

### 2. Provide the credentials

The scaffolded SQL checks read their connection string from the environment, so
it never sits in a file you commit:

```bash
export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'
```

### 3. Run it

```bash
restoreproof run --config my-drill/restoreproof.yaml
```

RestoreProof will:

1. check that Docker is usable;
2. create a private temporary workspace;
3. inspect the backup and restore it there;
4. start the Compose project with a unique name;
5. wait until the services are ready;
6. run the checks;
7. measure the RTO and estimate the RPO;
8. destroy the environment and the workspace;
9. write a JSON and a Markdown report.

Exit code `0` means every required check passed.

## Point it at your own system

Three edits, in order of importance:

**1. Your backup.** In `restoreproof.yaml`:

```yaml
backup:
  type: local
  path: /var/backups/myapp/latest
```

`/var/backups` is outside the project directory, so allowlist it explicitly:

```yaml
security:
  allow_external_paths:
    - /var/backups/myapp
```

Or use restic:

```yaml
backup:
  type: restic
  repository: /srv/restic/myapp
  password_file: ./restic-password
  snapshot: latest
```

**2. Your recovery environment.** Edit `docker-compose.recovery.yml` so it
starts your services from the restored data. `${RESTOREPROOF_RESTORE_DIR}` is
injected and points at the restored tree.

This file should look like production, not *be* production: no external
networks, no host mounts, no ports published outside loopback. RestoreProof
refuses those anyway and tells you why.

**3. Your proof.** Replace the checks with assertions about *your* data. A
drill that only proves "a container started" proves very little. The valuable
checks are the ones that would catch a silently truncated dump:

```yaml
  - id: recent-orders
    name: "Yesterday's orders are in the restored database"
    type: sql
    required: true
    dsn_env: DRILL_DATABASE_URL
    query: "SELECT count(*) FROM orders WHERE created_at > now() - interval '2 days'"
    min_value: 1
```

Then:

```bash
restoreproof validate --config my-drill/restoreproof.yaml
restoreproof plan     --config my-drill/restoreproof.yaml
restoreproof run      --config my-drill/restoreproof.yaml
```

## In CI

```yaml
- name: Recovery drill
  env:
    DRILL_DATABASE_URL: ${{ secrets.DRILL_DATABASE_URL }}
  run: |
    restoreproof run --config drills/nightly/restoreproof.yaml \
      --format markdown --output drill-report.md

- uses: actions/upload-artifact@v4
  if: always()
  with:
    name: recovery-drill-report
    path: drill-report.md
```

A non-zero exit fails the job. Branch on the specific code when you want to
treat "Docker is missing" (3) differently from "the recovery is broken" (1).

## Reading a report

```bash
restoreproof report reports/20260115T100000Z-myapp-0d1a5f7c.json
restoreproof report reports/20260115T100000Z-myapp-0d1a5f7c.json --format markdown
restoreproof report reports/20260115T100000Z-myapp-0d1a5f7c.json --verify
```

`--verify` recomputes the SHA-256 over the document and compares it with the
recorded one. It detects accidental modification; it is not a signature.

## Next

* [Configuration reference](configuration.md)
* [Check types](checks.md)
* [Security](security.md)
* [Troubleshooting](troubleshooting.md)
