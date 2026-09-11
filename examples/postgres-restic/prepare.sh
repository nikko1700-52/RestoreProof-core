#!/usr/bin/env bash
#
# Build a local restic repository for this example.
#
# The snapshot is created from standard input with an explicit filename, so it
# contains exactly one file at `/dump.sql`. `restic restore --target <dir>`
# then places it at `<dir>/dump.sql` on every machine — restic otherwise
# preserves the absolute path of whatever it backed up, which would make this
# example depend on where the repository happens to be checked out.
#
# Nothing here is specific to RestoreProof: it stands in for your own backup
# job. Run it once, then run the drill.

set -euo pipefail

cd "$(dirname "$0")"

if ! command -v restic >/dev/null 2>&1; then
    echo "restic is not installed." >&2
    echo "Install it (https://restic.net) or use examples/postgres-local, which needs no extra tool." >&2
    exit 3
fi

mkdir -p fixtures

PASSWORD_FILE="fixtures/restic-password"
if [ ! -s "$PASSWORD_FILE" ]; then
    # A throwaway repository password, generated locally and never committed.
    umask 077
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$PASSWORD_FILE"
    echo "Generated $PASSWORD_FILE"
fi
chmod 600 "$PASSWORD_FILE"

export RESTIC_PASSWORD_FILE="$PWD/$PASSWORD_FILE"
REPOSITORY="fixtures/restic-repository"

if [ ! -d "$REPOSITORY" ]; then
    restic init --repo "$REPOSITORY"
fi

restic backup \
    --repo "$REPOSITORY" \
    --stdin \
    --stdin-filename dump.sql \
    --tag restoreproof-example \
    < seed/dump.sql

echo
echo "Repository ready: $REPOSITORY"
restic snapshots --repo "$REPOSITORY" --latest 1
echo
echo "Now run the drill:"
echo "  export RESTOREPROOF_DEMO_DATABASE_URL='postgres://restoreproof:restoreproof-local-drill@127.0.0.1:15433/app'"
echo "  cargo run -- run --config examples/postgres-restic/restoreproof.yaml"
