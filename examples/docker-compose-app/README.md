# `docker-compose-app` — restore a web tier and prove it answers

Shows the `http` check: a drill that does not stop at "a container is running"
but asserts that the restored application actually serves the restored content.

## Run it

From the repository root:

```bash
cargo run -- validate --config examples/docker-compose-app/restoreproof.yaml
cargo run -- run      --config examples/docker-compose-app/restoreproof.yaml
```

Expected result: `PASSED` on the checks, exit code `0`.

The RPO will be reported as `RPO_FAIL` once this example is more than a day
old, because `backup/backup-metadata.json` carries a fixed timestamp. That is
deliberate: it is what a stale backup looks like in a report, and a failing RPO
does not fail the drill — only checks do.

## Notes

* Port `18080` on loopback must be free.
* No credentials are involved, so nothing needs to be exported before running.
