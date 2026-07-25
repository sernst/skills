# Development and release

The repository registers independent CLI components through `clis/registry.just`. The skill-manager module supplies format, format-check, lint, build, test, coverage, docs, deny, check, metadata, build-target, test-target, and package recipes. Run the root aggregation recipe for the same checks CI applies.

The quality gate uses `cargo fmt --check`, strict pedantic Clippy with warnings denied, rustdoc warnings denied, dependency policy checks, and an 85% line coverage threshold. Production code forbids unsafe and unchecked unwrap/expect/panic-style exits; narrow allowances must explain why they are safe. Exact auxiliary-tool versions are stored centrally and bootstrap scripts install locked versions.

Release version is the root `VERSION`, Cargo metadata, and changelog heading. The release recipe first validates a clean tree, version parity, and main ancestry; it creates an annotated tag and prints the push command without pushing.

PR CI runs the Linux GNU quality gate. Main also packages and inspects eight targets without publishing. Annotated SemVer `v…` tags reachable from `main` run the complete matrix, assemble archives, checksums, and release manifest, then publish a GitHub Release. A prerelease tag becomes a prerelease. Releases contain executable, readme, license, completions, and man page; crates.io, SBOMs, signatures, and attestations are intentionally out of scope for v0.1.
