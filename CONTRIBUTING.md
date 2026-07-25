# Contributing

Each executable CLI is an independent component under `clis/`, registered in
`clis/registry.just`. Its folder, registry ID, executable, and Cargo package
use the same lowercase kebab-case name.

Install the pinned developer tooling with `tools/bootstrap.ps1` on Windows or
`tools/bootstrap.sh` on macOS/Linux. Run `just check` before opening a pull
request. `just format` is the only repository recipe that rewrites source.

The quality gate includes formatting, strict Clippy, tests, rustdoc, dependency
policy, and 85% line coverage. Keep the code safe Rust and document any narrow
lint allowance next to the code that needs it.
