#!/usr/bin/env bash
#
# Run a recovery drill and leave the evidence where it can be found.
#
# Written to be driven by a systemd timer or cron; see docs/operations.md.
# Deliberately boring: it adds nothing to what `restoreproof run` already does
# except a lock, a log line and a non-zero exit that a scheduler will notice.
#
#   ./scripts/nightly-drill.sh /srv/drills/billing
#
# Credentials come from the environment (an EnvironmentFile or a root-only
# export), never from this script.

set -euo pipefail

DRILL_DIR="${1:?usage: nightly-drill.sh <scenario directory>}"
RESTOREPROOF="${RESTOREPROOF:-restoreproof}"
CONFIG="$DRILL_DIR/restoreproof.yaml"

if [ ! -f "$CONFIG" ]; then
    echo "no scenario at $CONFIG" >&2
    exit 2
fi

# One drill at a time per scenario. Two concurrent drills would fight over the
# published loopback ports, and the failure would look like a broken recovery
# rather than a scheduling mistake.
LOCK="/var/lock/restoreproof-$(basename "$DRILL_DIR").lock"
exec 9>"$LOCK"
if ! flock --nonblock 9; then
    echo "a drill for $DRILL_DIR is already running; skipping this run" >&2
    exit 0
fi

echo "$(date --iso-8601=seconds) starting recovery drill for $DRILL_DIR"

# Pre-pull so a cold image download does not eat the startup budget.
COMPOSE_FILE="$DRILL_DIR/docker-compose.recovery.yml"
if [ -f "$COMPOSE_FILE" ]; then
    docker compose --file "$COMPOSE_FILE" pull --quiet || true
fi

set +e
"$RESTOREPROOF" run --config "$CONFIG"
CODE=$?
set -e

case "$CODE" in
    0) echo "$(date --iso-8601=seconds) drill passed" ;;
    1) echo "$(date --iso-8601=seconds) DRILL FAILED: recovery could not be proven" >&2 ;;
    2) echo "$(date --iso-8601=seconds) configuration is invalid" >&2 ;;
    3) echo "$(date --iso-8601=seconds) a required dependency is missing" >&2 ;;
    4) echo "$(date --iso-8601=seconds) the backup could not be restored" >&2 ;;
    5) echo "$(date --iso-8601=seconds) the drill timed out" >&2 ;;
    *) echo "$(date --iso-8601=seconds) drill ended with exit code $CODE" >&2 ;;
esac

exit "$CODE"
