# Security guide

[SECURITY.md](../SECURITY.md) has the threat model and the reporting process.
This page is the practical companion: how to configure a drill safely, what
each switch does, and what to do when RestoreProof refuses to run.

## The one-paragraph version

A recovery drill restores production data onto a machine and starts software
from it. Treat the machine running the drill as being as sensitive as the
database being restored, keep the environment on loopback, keep cleanup on, and
do not attach reports to public tickets.

## The safety switches

All under `security:` in `restoreproof.yaml`. Every default is the conservative
option; you opt *in* to risk, never out of it by accident.

### `allow_command_checks` (default `true`)

`command` and `script` checks run local programs. That is their purpose.

Set it to `false` when the configuration did not come from you — a template, a
vendor, a pull request. A configuration that declares such a check is then
refused at validation time rather than silently ignored, so you find out before
running anything.

### `allow_external_paths` (default `[]`)

Every path in the configuration is resolved against the directory containing it,
canonicalised (which resolves `..` **and symlinks**) and required to stay inside
the project. To read a backup from elsewhere, say so explicitly:

```yaml
security:
  allow_external_paths:
    - /var/backups/myapp
```

Entries must be absolute and must exist. `/` is refused.

This **never** applies to executable content. Scripts, the Compose file and the
checks file must live inside the project, so that what runs during a drill is
versioned alongside the drill.

### `allow_external_http_targets` (default `false`)

HTTP checks may only target loopback. A drill probes the environment it just
built; a non-loopback target usually means a mistake, and at worst turns the
tool into a request-forgery primitive aimed at a metadata endpoint or an
internal service.

### `allow_external_sql_targets` (default `false`)

The same for SQL DSNs. This is the switch that stops a "drill" from querying
production.

### `allow_published_ports` (default `false`)

Publishing on a **loopback** address is always allowed:

```yaml
ports:
  - "127.0.0.1:15432:5432"   # fine, and what checks use
```

Publishing on **every interface** is refused unless you enable this:

```yaml
ports:
  - "15432:5432"             # refused: binds 0.0.0.0
```

The difference matters: the second form makes a copy of your production
database reachable from whatever network the drill machine is on.

### `max_command_output_bytes` (default `65536`)

Ceiling on captured output from one external command, so a runaway process
cannot produce a gigabyte-sized report.

## What the Compose audit refuses

Before anything starts. Each of these has produced a real incident somewhere:

| Refused | Why |
|---|---|
| `privileged: true` | Equivalent to root on the host. |
| mounting `/var/run/docker.sock` | Full control of the Docker daemon, from a container started on restored data. |
| `network_mode: host`, `pid: host`, `ipc: host`, `userns_mode: host` | Removes the isolation the drill depends on. |
| `cap_add: [SYS_ADMIN, SYS_PTRACE, SYS_MODULE, ALL, …]` | Container escape. |
| `security_opt` with `unconfined` or `label:disable` | Disables seccomp, AppArmor or SELinux. |
| `devices:` | Exposes host hardware. |
| mounts of `/`, `/etc`, `/var`, `/usr`, `/proc`, `/sys`, … | Host filesystem access. |
| mounts resolving outside the project | Same, less obviously. |
| `${ANY_OTHER_VARIABLE}` as a mount source | Its value is not audited. Only `RESTOREPROOF_RESTORE_DIR` and `RESTOREPROOF_WORKDIR` are accepted. |
| ports on `0.0.0.0` | Exposes restored data to the network. |
| `external: true` networks | Attaches the drill to a pre-existing network. |
| `include:` | Pulls in definitions that were never audited. |

Warnings, not refusals: an unpinned image tag (`nginx`, `nginx:latest`) makes a
drill non-reproducible over time.

**This audit is static.** It reads the Compose file. It does not constrain what
an image does once running. It is a guardrail against mistakes and copied
production files, not a container sandbox.

## Handling secrets

### Never put them in the configuration

```yaml
# Refused: RestoreProof will not let you write this.
headers:
  Authorization: "Bearer sk_live_..."
```

```yaml
# Do this instead.
headers_from_env:
  Authorization: DRILL_HEALTH_TOKEN
```

The same applies to backup repositories (`password_file` or `password_env`) and
databases (`dsn_env`).

### What happens to them

* Resolved once, before anything runs, so a missing credential is one clear
  warning rather than a surprise mid-drill.
* Never placed on a command line, where they would show in the host's process
  list. For restic, only the *path* is handed over via `RESTIC_PASSWORD_FILE`,
  so the plaintext never enters the `restoreproof` process at all.
* Held in a type with no `Display`, a redacted `Debug`, and zeroing on drop.
* Registered with a redactor that filters every log line, check message and
  report. URL credentials are additionally scrubbed structurally, which catches
  secrets nobody registered — such as one printed by a failing external tool.

### Secret files

```bash
chmod 600 restic-password
```

RestoreProof warns if a password file is readable by other users.

## Drilling on real production data

The drill worth running uses the real backup. It is also the one that can cause
a breach.

1. **Run it where you would run the production database.** The restored copy is
   exactly as sensitive as the original.
2. **Keep everything on loopback.** The default.
3. **Keep cleanup on.** `--keep-environment` is for debugging. Afterwards:
   ```bash
   docker compose -p <project> down -v
   rm -rf /tmp/restoreproof-<run id>
   ```
   The CLI prints both commands for you.
4. **Treat reports as sensitive.** They are `0600` in a `0700` directory, and
   they contain table names, row counts and error messages from your production
   data.
5. **Consider a subset.** A drill on a representative subset proves most of what
   matters with a fraction of the exposure.
6. **Remember your obligations.** Under the GDPR and similar regimes, a restored
   copy of personal data is still personal data: the same retention, access and
   record-keeping duties apply. Automatic masking is not part of this edition.

## Interrupting a drill

`Ctrl-C` and a cancelled CI job are handled: the signal is caught, every live
recovery environment is destroyed, and only then does the process exit (130 for
`SIGINT`, 143 for `SIGTERM`). If anything could not be destroyed, it is named
along with the command to remove it.

Do not send a second signal while that is happening — the message says so. The
first one is already destroying the environment; the second kills the process
mid-teardown and leaves restored data behind.

## If cleanup fails

The project name is unique per run and printed in the report:

```bash
docker compose -p <project-name> down --volumes --remove-orphans
rm -rf /tmp/restoreproof-<run-id>
```

To find anything left behind by an earlier drill:

```bash
docker ps -a --filter "name=-rp-"
ls -d /tmp/restoreproof-*
```

## What this does not protect you from

Stated plainly, because a security page that only lists strengths is not a
security page:

* **Report digests are not signatures.** They detect accidental modification.
  Anyone who can edit a report can recompute the digest.
* **A hostile container image** in your own Compose file is outside the model.
* **`command` and `script` checks run arbitrary local code** by design.
* **Nothing is encrypted at rest.** Restored data sits unencrypted in a
  temporary directory for the duration of the drill.
* **A compromised backup tool** on your `PATH` compromises the drill.
* **Remote repositories mean network egress** under that tool's own
  credentials. The plan output warns about it.
