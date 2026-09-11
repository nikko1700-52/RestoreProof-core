# Running drills in production

Everything up to here describes running a drill by hand. This page is about
running them every night, for real systems, in a way that someone else can take
over.

## The short version

1. One directory per system, committed to a repository, containing the
   scenario, the Compose file and the checks.
2. A scheduled job per system, on a machine you would trust with the production
   database itself.
3. Reports written to a durable location, with a retention policy.
4. An alert on `restoreproof_drill_success == 0` **and** on the drill going
   silent.
5. A quarterly review of the checks, because checks rot faster than code.

## Where to run drills

The machine running a drill holds a decrypted copy of the backup for the
duration. Treat it accordingly:

* same security zone and same access controls as the production database;
* not a shared build agent, unless that agent is already trusted with
  production data;
* enough disk for the restored data plus Docker images, with headroom — a drill
  that fails on `no space left on device` tells you nothing about your recovery;
* nothing else important on it, because a drill starts containers and consumes
  I/O.

A dedicated small VM per environment is the usual answer. An MSP running drills
for several customers needs one per customer, not one shared.

## Scheduling

### systemd timer

The straightforward option on a Linux host. `scripts/nightly-drill.sh` in this
repository is a starting point.

`/etc/systemd/system/restoreproof@.service`:

```ini
[Unit]
Description=Recovery drill for %i
After=docker.service
Requires=docker.service

[Service]
Type=oneshot
User=restoreproof
WorkingDirectory=/srv/drills/%i
# Credentials come from a file readable only by this user.
EnvironmentFile=/etc/restoreproof/%i.env
ExecStart=/usr/local/bin/restoreproof run --config /srv/drills/%i/restoreproof.yaml

# A drill must never outlive its window.
TimeoutStartSec=3600
# SIGTERM is handled: the recovery environment is destroyed before exit.
KillSignal=SIGTERM
KillMode=mixed

# The tool needs the Docker socket, so it cannot be fully sandboxed, but
# everything it does not need can still be taken away.
NoNewPrivileges=true
PrivateTmp=false
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/srv/drills/%i /var/lib/node_exporter/textfile /tmp
```

`/etc/systemd/system/restoreproof@.timer`:

```ini
[Unit]
Description=Nightly recovery drill for %i

[Timer]
OnCalendar=*-*-* 02:30:00
RandomizedDelaySec=900
Persistent=true

[Install]
WantedBy=timers.target
```

```bash
systemctl enable --now restoreproof@billing.timer
systemctl list-timers 'restoreproof@*'
journalctl -u restoreproof@billing.service --since yesterday
```

`PrivateTmp=false` is deliberate: the restored data goes into a workspace under
`/tmp`, and Compose bind-mounts it into containers started by the *host's*
Docker daemon, which cannot see a private tmpfs.

### cron

```cron
30 2 * * * restoreproof  cd /srv/drills/billing && /usr/local/bin/restoreproof run --config restoreproof.yaml >> /var/log/restoreproof/billing.log 2>&1
```

Works, but you lose the timeout and the journal. Prefer a timer where you have
one.

### CI

See [getting-started.md](getting-started.md#in-ci). A drill in CI proves the
recovery *procedure* works; a drill on a schedule proves the *backups* work.
They answer different questions, and most places want both.

## Credentials

Never in the scenario files, which are committed. Use a root-only environment
file:

```bash
install -m 600 -o root -g root /dev/null /etc/restoreproof/billing.env
cat > /etc/restoreproof/billing.env <<'EOF'
DRILL_DATABASE_URL=postgres://drill:...@127.0.0.1:15432/billing
RESTIC_PASSWORD_FILE=/etc/restoreproof/billing.restic
EOF
```

The database role in that DSN should have **no write privileges**. The read-only
SQL guard is defence in depth, not a substitute for least privilege.

## Reports: where they go and how long they stay

Reports are written `0600` in a `0700` directory. They contain table names, row
counts and error messages from production data — sensitive, if not catastrophic.

```yaml
report:
  directory: /var/lib/restoreproof/billing/reports
  formats:
    - json        # keep these: `diff` and `--verify` need them
    - markdown    # for tickets
    - junit       # if a CI system consumes them
    - prometheus  # see below
```

A reasonable policy:

* keep JSON reports for as long as you need to answer "was recovery working in
  March?" — often 12 months, sometimes longer if an auditor asks;
* prune the rest aggressively;
* back the report directory up, or the evidence disappears with the machine.

```cron
# Keep a year of JSON, a month of everything else.
0 4 * * * find /var/lib/restoreproof -name '*.json' -mtime +365 -delete
5 4 * * * find /var/lib/restoreproof ! -name '*.json' -type f -mtime +31 -delete
```

## Alerting

Point one drill's report directory at the `node_exporter` textfile collector, or
copy the `.prom` file there after each run:

```yaml
report:
  directory: /var/lib/node_exporter/textfile
  formats: [json, prometheus]
```

```yaml
groups:
  - name: recovery
    rules:
      - alert: RecoveryDrillFailing
        expr: restoreproof_drill_success == 0
        for: 10m
        annotations:
          summary: "{{ $labels.project }} cannot be recovered"

      - alert: RecoveryDrillSilent
        expr: time() - restoreproof_drill_completed_timestamp_seconds > 129600
        annotations:
          summary: "No recovery drill for {{ $labels.project }} in 36 hours"

      - alert: RecoveryTooSlow
        expr: restoreproof_rto_seconds > restoreproof_rto_target_seconds
        annotations:
          summary: "{{ $labels.project }} recovery exceeds its RTO"

      - alert: BackupTooOld
        expr: restoreproof_rpo_seconds > restoreproof_rpo_target_seconds
        annotations:
          summary: "{{ $labels.project }} backup is older than its RPO"
```

`RecoveryDrillSilent` is the one people leave out and later regret. A drill that
stopped running produces no failures at all.

## Catching slow degradation

A drill that passes today and passes next month can still be getting worse.
Keep a known-good report and compare:

```bash
restoreproof diff /var/lib/restoreproof/billing/known-good.json \
                  "$(ls -t /var/lib/restoreproof/billing/reports/*.json | head -1)"
```

Exit code 1 means something regressed. Promote a new known-good deliberately,
after you have looked at why it changed — not automatically.

## Sizing and timeouts

| Setting | Start with | Raise when |
|---|---|---|
| `startup_timeout_seconds` | 180 | large images, cold pulls, slow init |
| `total_timeout_seconds` | 3× a normal run | restores measured in tens of GB |
| `defaults.retry.attempts` | enough to cover startup | a service warms up slowly |
| `security.max_command_output_bytes` | 65536 | a check legitimately prints more |

Pre-pull images on the drill host so a cold pull does not eat the startup
budget:

```bash
docker compose -f /srv/drills/billing/docker-compose.recovery.yml pull
```

Measure a real run before setting `target_rto_seconds`. An objective invented in
a meeting is not an objective.

## Upgrading the tool

Report schema version 1 is stable; a future incompatible schema will bump it
rather than reinterpret old files. Exit codes are a stable contract.

Upgrade one system first, compare a report before and after with
`restoreproof diff`, then roll out. Keep the previous binary until you have.

## Before you trust a drill

A checklist worth running once per system:

- [ ] **Make it fail on purpose.** Point a check at a table that does not exist
      and confirm the drill goes red and exits 1. A suite that has never failed
      has not been tested.
- [ ] **Check the checks assert data, not liveness.** "A container started"
      proves almost nothing.
- [ ] **Confirm the RPO is real.** If the report says the timestamp is
      approximated, write a `metadata_file` from your backup job.
- [ ] **Interrupt a drill** with `Ctrl-C` and confirm nothing is left running.
- [ ] **Run `plan`** and read the safety section: are any refusals disabled?
- [ ] **Check who can read the reports.**
- [ ] **Verify a report** with `--verify`, so you know the command works before
      you need it in an audit.
- [ ] **Write down who gets paged** when `RecoveryDrillFailing` fires, and what
      they are expected to do.

## What this does not do

Worth saying out loud before someone builds a process on it:

* it does not schedule anything itself — that is your timer or your CI;
* it keeps no history — each run writes a report and forgets;
* it has no interface beyond the CLI;
* it verifies one system at a time, on one machine.

Those are deliberate limits of the open-source edition, not oversights. See
[PREMIUM.md](../PREMIUM.md).
