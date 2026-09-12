# RestoreProof Premium

RestoreProof Core, in this repository, is open source under Apache-2.0 and
stays that way. A commercial edition is planned on top of it. This file says
exactly where the line is, so that nobody has to guess.

## The rule

**The proof engine is open source. The organisation around it is commercial.**

Everything needed to prove that one application can be restored and restarted —
restore, start, check, measure, report — is in this repository, works offline,
needs no account, and is not limited in any way.

What the commercial edition adds is what an *organisation* needs once it runs
many drills, for many systems, over a long period: central scheduling, shared
history, access control, audit evidence and environments the open-source
edition deliberately does not touch.

## What is in RestoreProof Core, permanently

* the CLI and every check type
* `local`, `restic` and `borg` backup sources
* Docker Compose recovery environments
* RTO measurement and RPO estimation
* JSON, Markdown and terminal reports with integrity digests
* the whole security model: path confinement, read-only SQL, Compose auditing,
  secret redaction, guaranteed cleanup

There is **no** licence check, **no** feature flag, **no** telemetry, **no**
"contact us to unlock", **no** required account and **no** call home anywhere in
this repository. There never will be. If you ever find one, it is a bug — please
report it.

## What the commercial edition is for

### Running drills for an organisation, not a laptop

| | |
|---|---|
| **Web console** | See the state of every system at a glance instead of reading JSON. |
| **Central scheduling** | Nightly drills across many systems, without a cron job per repository. |
| **Distributed execution** | Run drills on agents close to the data, driven centrally. |
| **Parallel scenarios** | Many drills at once, with queueing and resource limits. |
| **Long-term history** | Is our recovery getting slower? Which drill first failed? Core writes one report per run; comparing them over months is the commercial feature. |

### Multiple teams, multiple customers

| | |
|---|---|
| **Organisations, users and roles** | Who may see which system's reports. |
| **SSO and SCIM** | Identity integration for companies that require it. |
| **MSP multi-tenancy** | One console, many customers, strictly separated. |
| **Centralised audit trail** | Who ran what, when, and what it produced. |

### Evidence that satisfies an auditor

| | |
|---|---|
| **Signed PDF reports** | Core's SHA-256 digest detects accidental modification; it is not a signature. Cryptographically signed, timestamped reports are commercial. |
| **Compliance dashboards** | Mapping drills to the controls an auditor asks about. |
| **Data masking** | Automatically redacting personal data in restored environments, so drills on real data are defensible. |

### Environments Core does not touch

| | |
|---|---|
| **Kubernetes** | Restore into an ephemeral namespace. |
| **VMware, Proxmox** | Hypervisor-level snapshot recovery. |
| **Managed cloud backups** | AWS Backup, Azure Backup, Google Cloud Backup. |
| **Ephemeral cloud environments** | Recovery environments provisioned on demand. |
| **Managed object storage connectors** | S3, Azure Blob, GCS with platform-managed credentials. |

### Commercial arrangements

Support with response times, SLAs, private installation, and professional
integration work.

## How the boundary is enforced

Not by good intentions. `cargo test -p restoreproof-cli --test open_source_guarantees`
runs on every push and fails the build if:

* any licence, entitlement, activation or telemetry identifier appears in the
  code — documentation may discuss them freely, comments are stripped before
  scanning;
* any crate here gains a dependency on a commercial crate, which would make this
  edition unbuildable for everyone else;
* `restoreproof --help` ever mentions an account, an activation or a
  subscription;
* the promises above disappear from this file.

The test is deliberately annoying to work around. If one of those checks fails,
the question is not how to silence it — it is whether the change belongs in the
commercial repository instead.

## Why this split

Three honest reasons.

**The value is different.** Proving *one* recovery works is a technical problem,
and a solved one — it should be free, auditable, and something you can run
before you trust anyone. Proving that *forty* recoveries work every night, for
three customers, with evidence an auditor accepts, is an operational problem,
and it costs real money to run.

**The costs are different.** A CLI on your machine costs nothing to operate. A
hosted console with long-term history, distributed agents and signed evidence
has continuous costs. Charging for the second is what keeps the first
maintained.

**Trust runs one way.** A tool that tells you your disaster recovery is fine had
better be inspectable. The engine that produces the verdict is open source
precisely so you can check that it is not lying to you.

## Growing from Core to the commercial edition

The split is architectural, not artificial. The commercial edition is built on
the same crates published here:

```
restoreproof-core · restoreproof-config · restoreproof-storage
restoreproof-checks · restoreproof-runner · restoreproof-report
```

The CLI in this repository is a thin front-end over them, and nothing in those
crates depends on it. A new backup source implements
`restoreproof_storage::source::BackupSource`. A new check type implements
`restoreproof_checks::executor::CheckExecutor`. A different orchestrator
implements `restoreproof_checks::context::EnvironmentProbe`. See
[docs/architecture.md](docs/architecture.md).

Practically, that means:

* your `restoreproof.yaml` and `checks.yaml` keep working;
* your reports keep the same schema, so history is continuous;
* your exit codes do not change, so your CI keeps working;
* and if you stop paying, the drills you already have keep running, because the
  engine is the one in this repository.

## Status

**Nothing here is for sale, and none of the features above are implemented in
this repository.** That second half is the part that affects you, and it is not
a promise in prose: `open_source_guarantees` fails the build if commercial
machinery ever appears in this code.

Work on a commercial edition has started, in a separate private repository: a
control plane that stores drill reports, shows them per system, keeps
organisations apart and records an audit trail. It is early and it is not for
sale. It changes nothing here — it depends on these crates exactly as published,
never a fork or a patch, and if it were abandoned tomorrow everything in this
repository would keep working unchanged.

Most of the list above is not built anywhere yet: no central scheduling, no
Kubernetes or hypervisor environments, no signed PDFs, no SSO, no data masking.
This file remains a statement of scope for contributors, not a product
announcement.

If a feature you need is on the commercial list and you would rather it were
open source, say so in an issue. The list is a plan, not a promise to keep
anything from you.
