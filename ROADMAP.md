# Roadmap

What is planned for **RestoreProof Core**, the open-source edition. Things
deliberately out of scope here are in [PREMIUM.md](PREMIUM.md).

This is a direction, not a delivery schedule. There are no dates because there
is no team behind them yet.

## Now — 0.1.x

Shipped:

* CLI: `init`, `validate`, `plan`, `run`, `check`, `report`, `version`
* backup sources: `local`, `restic`, `borg` (experimental)
* checks: `file`, `container`, `http`, `sql`, `command`, `script`
* Docker Compose recovery environments with guaranteed cleanup
* measured RTO, estimated RPO, `UNKNOWN` when it cannot be determined
* JSON, Markdown, terminal, JUnit XML and Prometheus reports with integrity
  digests
* `restoreproof diff` for comparing two runs locally
* cleanup that survives `Ctrl-C` and a cancelled CI job
* the security model: path confinement, read-only SQL, Compose auditing, secret
  redaction, argument-injection refusal

Remaining before 0.1 is "done":

* [x] a recorded terminal session for the README — `docs/demo.svg`, generated
      from real output of the passing and the deliberately broken example
* [ ] end-to-end CI coverage of the restic example (CI does not install restic
      today, so `examples/postgres-restic` is untested by automation)
* [ ] verifying the tool on a non-Linux platform, or saying clearly that it is
      Linux-only

## Next — 0.2

Making the tool usable on real systems rather than examples.

* **MySQL and MariaDB `sql` checks.** PostgreSQL-only is the most common reason
  the tool would not fit a project today. Needs the same three-layer read-only
  guarantee as PostgreSQL, not a weaker one.
* **`sql` checks against a container**, without publishing a port on the host —
  by executing the client inside the recovery network.
* **Better failure output.** When a service fails to start, put the relevant
  container logs in the report rather than only the tail.
* **Partial restore support** for large backups — restore only what the checks
  need.
* **Optional parallel check execution.** Checks run sequentially today, which is
  predictable and easy to reason about. An opt-in `--jobs` would cut drill time
  on scenarios with many independent checks.

## Later — 0.3 and beyond

* **A `wait` check**, expressing "this becomes true within N seconds" more
  directly than retry counts.
* **Compose profiles**, so one file can describe several recovery shapes.
* **Podman support.** Compose-compatible, rootless by default, a good fit for
  the security model.
* **Restore-into-an-existing-database** flows, for setups where the recovery
  target is provisioned separately.
* **A stable Rust API**, once the crate boundaries have stopped moving. They are
  documented in [docs/architecture.md](docs/architecture.md) but not yet
  promised.

## Explicitly not planned here

These are not "later", they are **out of scope** for the open-source edition:

* web interface, hosted service, multi-tenancy, SSO
* central scheduling, distributed execution, long-term history
* Kubernetes, VMware, Proxmox, managed cloud backup services
* signed PDF reports, compliance dashboards, automatic data masking
* any licence check, feature flag, telemetry, or required account

The reasoning is in [PREMIUM.md](PREMIUM.md). If you think something on this
list should be open source, open an issue and make the argument.

## Principles that will not change

1. **The open-source edition works offline, forever, with no account.**
2. **No telemetry.** Not opt-out, not anonymous, not "just crash reports".
3. **`UNKNOWN` over a guess.** A value that cannot be determined is reported as
   unknown. Ever.
4. **No claim without evidence.** No invented benchmarks, no "enterprise-ready",
   no capability in the documentation that is not in the code.
5. **Refusals stay conservative by default.** Widening the blast radius of a
   drill is always opt-in.

## Contributing to the roadmap

Open an issue. Bug reports with a reproducing scenario, and reports of the tool
being wrong about a recovery, are more valuable than feature requests right now.
