# Troubleshooting

## Start here

```bash
restoreproof validate --config path/to/restoreproof.yaml   # is the config sound?
restoreproof plan     --config path/to/restoreproof.yaml   # what would it do?
restoreproof run      --config path/to/restoreproof.yaml -vv   # what did it do?
```

`-v` gives progress, `-vv` debug detail, `-vvv` trace. Logs go to standard
error, so they never mix with a report on standard output.

## Exit codes

| Code | Meaning | Usual cause |
|---|---|---|
| 0 | every required check passed | — |
| 1 | a required check failed | **your recovery does not work**; read the report |
| 2 | invalid configuration | run `validate` |
| 3 | a missing dependency | Docker, `restic` or `borg` not installed or not reachable |
| 4 | the backup could not be restored | wrong password, unreadable repository, no space |
| 5 | a timeout expired | services too slow, or a timeout set too low |
| 6 | internal error | please report it |
| 7 | incorrect usage | check the command line |

---

## "`docker` was not found" / "the Docker daemon is not reachable" (exit 3)

```bash
docker compose version     # must print a v2 version
docker info                # must reach a daemon
```

* The standalone `docker-compose` v1 script is **not** supported. You need the
  Compose v2 plugin.
* Rootless or remote Docker: `DOCKER_HOST` is read from your environment.
  RestoreProof forwards `DOCKER_HOST`, `DOCKER_CONTEXT`, `DOCKER_CONFIG`,
  `DOCKER_CERT_PATH`, `DOCKER_TLS_VERIFY` and `XDG_RUNTIME_DIR` to `docker`. A
  configuration file cannot set them, on purpose.
* Permission denied on the socket: your user is not in the `docker` group.

## "resolves outside the project directory" (exit 2)

Paths are confined to the directory containing the configuration file, after
resolving `..` and symlinks.

For **data** — a backup repository, a password file, a report directory:

```yaml
security:
  allow_external_paths:
    - /var/backups/myapp
```

Entries must be absolute and must exist.

For **executable content** — scripts, the Compose file, the checks file — this
is not available. Move the file inside the project. What runs during a drill
must be versioned alongside the drill.

If the path looks like it should be inside the project, it is probably a
symlink: canonicalisation follows symlinks before the check.

## "`--...` starts with `-`, which an external tool would interpret as an option"

A value that reaches an external tool as an argument may not start with `-`.
This is not cosmetic: `snapshot: "--password-command=id"` would otherwise be
handed to `restic` as an option. Use the real snapshot id, or `latest`.

## "the query starts with `X`. Only SELECT and WITH are accepted"

`sql` checks are read-only by construction. If you need to prepare state, do it
in the Compose file (an init container, a seed script), not in a check. A check
observes; it does not change what it is observing.

Keywords inside string literals are fine: `WHERE kind = 'delete'` passes.

## "is not a loopback address"

Point the check at the environment the drill just started. If you genuinely need
another host:

```yaml
security:
  allow_external_http_targets: true   # or allow_external_sql_targets
```

Understand what you are enabling: for SQL, it is the difference between querying
a restored copy and querying production.

## "publishes `...` on every network interface"

```yaml
ports:
  - "15432:5432"             # refused: binds 0.0.0.0
  - "127.0.0.1:15432:5432"   # accepted
```

The first form makes a copy of your production data reachable from the network.
Loopback is what checks need, so there is rarely a reason for the other form.

## "the recovery environment was not ready within Ns" (exit 5)

The report lists which services were still pending and why, with the tail of the
container logs.

* Pulling images the first time can take longer than the timeout. Pull once
  by hand, or raise `recovery.startup_timeout_seconds`.
* A service stuck in `health: starting` means its healthcheck never passes. Test
  it directly:
  ```bash
  restoreproof run --config ... --keep-environment
  docker compose -p <project> exec database pg_isready -U restoreproof -d app
  ```
  Then clean up (see below).
* A service that keeps restarting fails the drill immediately rather than
  waiting out the timeout.

## The database starts but the restored dump was ignored

PostgreSQL only runs `/docker-entrypoint-initdb.d/*` when it initialises a
**new** data directory. If the Compose file declares a named volume for
`/var/lib/postgresql/data` that already has data in it, the dump is skipped
silently.

RestoreProof uses a unique project name per run, so volumes do not persist
between drills — but check for a hard-coded `external: true` volume in your
Compose file.

## SQL checks fail with "connection refused"

The check connects from the **host**, not from inside the network. The port must
be published on loopback:

```yaml
ports:
  - "127.0.0.1:15432:5432"
```

and the DSN must use that host port:

```bash
export DRILL_DATABASE_URL='postgres://user:pass@127.0.0.1:15432/app'
```

## SQL checks report ERROR: "the environment variable is not set"

Credentials are read from the environment before the drill starts. Export the
variable named in `dsn_env`. `plan` lists every variable a scenario needs.

Note that checks do **not** inherit your whole shell environment — only what
they declare. That is deliberate: a credential sitting in your shell should not
silently reach a check.

## "`...` is world-writable and will not be executed"

```bash
chmod 755 drill/verify-invariants.sh
```

A world-writable script means any local user can change what your drill runs.

## I pressed Ctrl-C — did it leave anything behind?

No. `SIGINT` and `SIGTERM` are caught, the environment is destroyed, and the
process then exits with 130 or 143. You will see:

```
interrupted: destroying 1 recovery environment(s) before exiting. Do not kill
this process again — restored data would be left behind.
interrupted: recovery environments destroyed.
```

Do not send a second signal while that message is on screen: the first one is
already cleaning up, and the second would kill the process mid-teardown.

If something still could not be destroyed, it is named with the exact command to
remove it.

## A leftover environment

Cleanup runs on every path, including panics and timeouts, unless you passed
`--keep-environment`. If something is still there:

```bash
docker ps -a --filter "name=-rp-"
docker compose -p <project-name> down --volumes --remove-orphans
ls -d /tmp/restoreproof-*
rm -rf /tmp/restoreproof-<run-id>
```

The project name and workspace path are printed in the report and by the CLI.

## The RPO is huge, or `RPO_UNKNOWN`

* `RPO_UNKNOWN` means the source could not report when the backup was taken.
  For `type: local`, add a `metadata_file`. Nothing is guessed on purpose.
* A large RPO with `type: local` and no metadata file usually means the
  timestamp came from file modification times — for instance the moment you
  cloned the repository. The report says when it is an approximation.
* A large RPO with an accurate timestamp means the backup really is old. That is
  the finding, not a bug.

Objectives never change the exit code. Only checks do.

## `--verify` says the integrity check failed

The report was modified after it was written. Even reformatting the JSON changes
the digest.

Note what this does and does not prove: it detects accidental modification. It
is not a signature, and anyone who can edit the report can recompute the digest.

## Still stuck

Open an issue with:

* `restoreproof version`
* the output of `restoreproof plan` (it never contains secrets)
* the JSON report, if one was produced
* `docker compose version` and `docker info | head -20`

Please redact anything specific to your data before posting.
