#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
versions="$repo_root/TOOL_VERSIONS"
version() { sed -n "s/^$1=//p" "$versions"; }

rustup toolchain install "$(version rust)" --profile minimal
rustup component add rustfmt clippy --toolchain "$(version rust)"
cargo install just --version "$(version just)" --locked
cargo install cargo-llvm-cov --version "$(version cargo-llvm-cov)" --locked
cargo install cargo-deny --version "$(version cargo-deny)" --locked
cargo install cargo-zigbuild --version "$(version cargo-zigbuild)" --locked
cargo install zizmor --version "$(version workflow-audit)" --locked
bash "$repo_root/tools/install-zig.sh" "$(version zig)"
