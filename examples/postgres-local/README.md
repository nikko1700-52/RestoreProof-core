# `postgres-local` — restore a PostgreSQL dump from a directory

The smallest complete drill. It needs nothing but Docker: no restic, no cloud
account, no credentials.

## What it proves

1. the dump is present in the restored data (`file` check);
2. PostgreSQL starts from it and reports healthy (`container` check);
3. the `orders` and `customers` tables actually contain rows (`sql` checks);
4. the most recent row is the one you expect (`sql` check, optional).

## Run it

From the repository root:

```bash
# The connection string is read from the environment, never from a file you
# commit. This one is a throwaway credential for a throwaway container.
export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15432/app'

cargo run -- validate --config examples/postgres-local/restoreproof.yaml
cargo run -- plan     --config examples/postgres-local/restoreproof.yaml
cargo run -- run      --config examples/postgres-local/restoreproof.yaml
```

Expected result: `PASSED`, exit code `0`, and two reports in
`examples/postgres-local/reports/`.

## See a failure

A tool that only ever prints green is not worth much. Run the same drill
against an assertion that cannot hold:

```bash
cargo run -- run --config examples/postgres-local/restoreproof.failing.yaml
```

Expected result: `FAILED`, exit code `1`, and a report naming `invoices` as the
table that is missing.

## Notes

* Port `15432` on loopback must be free. Nothing is published on any other
  interface — RestoreProof refuses a Compose file that would.
* This example has no `metadata_file`, so the backup timestamp is derived from
  file modification times. The report says so, and the RPO is reported as an
  approximation. A real setup writes a metadata file from the backup job; see
  [`docs/configuration.md`](../../docs/configuration.md).
* Everything is destroyed at the end. Pass `--keep-environment` to inspect the
  restored database yourself, and remember it then holds a copy of whatever was
  in the backup.
