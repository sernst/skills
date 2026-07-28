#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <absolute-skill-manager-binary> <github-skills-url>" >&2
  exit 64
fi

skill_manager=$1
source_url=$2

if [[ $skill_manager != /* ]]; then
  echo "skill-manager binary path must be absolute" >&2
  exit 64
fi
if [[ ! -f $skill_manager || ! -x $skill_manager ]]; then
  echo "skill-manager binary is not an executable file: $skill_manager" >&2
  exit 66
fi
if [[ ! $source_url =~ ^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/tree/[0-9A-Fa-f]{40}/skills/?$ ]]; then
  echo "source URL must be a public GitHub skills path at an exact commit" >&2
  exit 64
fi

smoke_root=$(mktemp -d /tmp/skill-manager-live-smoke.XXXXXXXX)
cleanup() {
  cd /
  rm -rf -- "${smoke_root:?}"
}
trap cleanup EXIT
trap 'exit 130' HUP INT TERM

mkdir -p "$smoke_root/home" "$smoke_root/tmp" "$smoke_root/work"
export HOME="$smoke_root/home"
export SKILL_MANAGER_HOME="$HOME"
export XDG_CACHE_HOME="$HOME/.cache"
export TMPDIR="$smoke_root/tmp"
export GIT_TERMINAL_PROMPT=0
cd "$smoke_root/work"

"$skill_manager" --json source add "$source_url" live-github-smoke
"$skill_manager" --json status --refresh
