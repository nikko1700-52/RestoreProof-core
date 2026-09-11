## What this changes

<!-- One or two sentences. If it fixes an issue, "Fixes #123". -->

## Why

<!-- What problem does it solve? For a bug fix, what was going wrong? -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Tests added or updated. For a security control: both the case it refuses
      **and** a legitimate case it must still accept.
- [ ] Documentation updated in the same PR (`docs/`, and `SECURITY.md` if a
      safety switch or refusal changed)
- [ ] `CHANGELOG.md` updated under *Unreleased*, for user-visible changes

## Invariants

These are load-bearing. Tick the ones your change touches and say how it keeps
them, or explain why it should not.

- [ ] Does not start an external program outside `restoreproof_core::process`
- [ ] Does not use a configured path without `PathPolicy`
- [ ] Cannot put a secret in `argv`, a log line, or a report
- [ ] Still produces a report when the drill fails
- [ ] Still destroys the recovery environment on every path
- [ ] Does not change the meaning of an exit code
