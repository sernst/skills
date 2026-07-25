#!/usr/bin/env bash
set -euo pipefail

version="${1:?Zig version is required}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
temporary_root="${RUNNER_TEMP:-$(mktemp -d)}"
install_root="${2:-$repo_root/.tools/zig-$version}"
archive="$temporary_root/zig-$version.tar.xz"
url="https://ziglang.org/download/$version/zig-x86_64-linux-$version.tar.xz"
curl --fail --location --retry 3 --proto '=https' --tlsv1.2 "$url" --output "$archive"
mkdir -p "$install_root"
tar -xJf "$archive" --strip-components=1 -C "$install_root"
"$install_root/zig" version | grep -Fx "$version"
if [[ -n "${GITHUB_PATH:-}" ]]; then printf '%s\n' "$install_root" >> "$GITHUB_PATH"; fi
printf 'Zig %s installed at %s\n' "$version" "$install_root"
