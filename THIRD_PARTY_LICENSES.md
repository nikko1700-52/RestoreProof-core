# Third-party licences

RestoreProof Core is licensed under [Apache-2.0](LICENSE). It links against
third-party Rust crates, all of which are under permissive licences compatible
with Apache-2.0.

No copyleft licence (GPL, AGPL, LGPL) is used in the dependency tree. No code
from an incompatibly licensed project has been copied into this repository.

## Direct dependencies

| Crate | Version | Licence | Used for |
|---|---|---|---|
| `async-trait` | 0.1 | MIT OR Apache-2.0 | dyn-compatible async traits at the extension points |
| `chrono` | 0.4 | MIT OR Apache-2.0 | timestamps, RTO/RPO arithmetic |
| `clap` | 4 | MIT OR Apache-2.0 | command line parsing |
| `reqwest` | 0.12 | MIT OR Apache-2.0 | HTTP checks (rustls, no OpenSSL) |
| `serde` | 1 | MIT OR Apache-2.0 | serialisation |
| `serde_json` | 1 | MIT OR Apache-2.0 | JSON reports, tool output parsing |
| `serde_yaml_ng` | 0.10 | MIT | YAML configuration |
| `sha2` | 0.10 | MIT OR Apache-2.0 | report and file digests |
| `sqlx` | 0.8 | MIT OR Apache-2.0 | PostgreSQL checks (rustls) |
| `tempfile` | 3 | MIT OR Apache-2.0 | temporary directories, tests |
| `thiserror` | 2 | MIT OR Apache-2.0 | error types |
| `tokio` | 1 | MIT | async runtime, process handling |
| `tracing` | 0.1 | MIT | structured logging |
| `tracing-subscriber` | 0.3 | MIT | log formatting and filtering |
| `url` | 2 | MIT OR Apache-2.0 | URL validation for HTTP checks and DSNs |
| `uuid` | 1 | Apache-2.0 OR MIT | unique run identifiers |
| `zeroize` | 1 | Apache-2.0 OR MIT | wiping secrets from memory |

Development and test only:

| Crate | Version | Licence |
|---|---|---|
| `assert_cmd` | 2 | MIT OR Apache-2.0 |
| `predicates` | 3 | MIT OR Apache-2.0 |

## Transitive dependencies

291 packages in total, all permissive:

| Licence | Packages |
|---|---|
| MIT OR Apache-2.0 (in various spellings) | 246 |
| MIT only | 45 |
| Unicode-3.0 | 18 |
| ISC, BSD-2-Clause, BSD-3-Clause, Zlib, Unlicense, BSL-1.0 (as options) | the remainder |
| CDLA-Permissive-2.0 (`webpki-roots`) | 2 |

`webpki-roots` carries the Mozilla CA certificate bundle under
CDLA-Permissive-2.0, a permissive data licence.

Regenerate the exact list at any time:

```bash
cargo tree --all-features
cargo deny check licenses      # see deny.toml
```

## Licence policy

`deny.toml` in the repository root encodes the policy that CI enforces on every
push. A dependency introducing a copyleft or unknown licence fails the build.

## Reusing this project

Apache-2.0 permits commercial use, modification, distribution and private use,
provided you keep the licence and copyright notices and state significant
changes. It also grants an explicit patent licence.

If you redistribute a compiled binary, the licences above apply to the crates
linked into it. `cargo about` or `cargo bundle-licenses` will generate a
complete notice file with full licence texts, which is what you want to ship
alongside a binary.
