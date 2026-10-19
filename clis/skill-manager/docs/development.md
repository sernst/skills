# Development and release

The repository registers independent CLI components through `clis/registry.just`. The skill-manager module supplies run, format, format-check, lint, build, test, coverage, docs, deny, check, metadata, build-target, test-target, and package recipes. Run the development CLI locally with `just run --version` or `just run configs` from `clis/skill-manager`; from the repository root, use `just skill-manager run --version` or `just skill-manager run configs`. Run `just skill-manager check` from the repository root for the same checks CI applies.

The quality gate uses `cargo fmt --check`, strict pedantic Clippy with warnings denied, rustdoc warnings denied, dependency policy checks, and an 85% line coverage threshold. Production code forbids unsafe and unchecked unwrap/expect/panic-style exits; narrow allowances must explain why they are safe. Exact auxiliary-tool versions are stored centrally and bootstrap scripts install locked versions.

CI uses the pinned Rust toolchain; the manifest's Rust 1.96 minimum is also
validated with `cargo +1.96.0 check --locked --all-targets --all-features`.
Dependency refreshes update the complete compatible lockfile graph and verify
new major releases of direct dependencies and build tools against upstream
registries. Transitive major versions remain bounded by their upstream
dependency requirements. Benchmark workflows
pin Python 3.15.0 and test the updater on Linux, Windows, and macOS.

The reqwest 0.13 transport explicitly configures rustls with ring and Mozilla
roots to preserve the previous `rustls-tls` trust policy. Reqwest's new default
uses platform roots and aws-lc, which would change certificate trust and native
build requirements. Certificate validity and hostname checks remain enabled;
offline TLS handshake tests cover trusted TLS 1.2/1.3 connections, untrusted
roots, wrong hostnames, and expired certificates.

Never run a built `skill-manager` binary against the real user home while validating or smoke testing. Every manual invocation must pass `--home <scratch-dir>` (or set `SKILL_MANAGER_HOME` to a scratch dir); `--home` takes precedence and is the safest choice. Automated tests already isolate the home this way — see `tests/operations_contract.rs` and `tests/home_isolation_contract.rs`.

Release version is the root `VERSION`, Cargo metadata, and changelog heading. The release recipe first validates a clean tree, version parity, and main ancestry; it creates an annotated tag and prints the push command without pushing.

PR CI runs the Linux GNU quality gate. Main also packages and inspects eight targets without publishing. Annotated SemVer `v…` tags reachable from `main` run the complete matrix, assemble archives, checksums, and release manifest, then publish a GitHub Release. A prerelease tag becomes a prerelease. Releases contain executable, readme, license, completions, and man page; crates.io, SBOMs, signatures, and attestations are intentionally out of scope for v0.1.
