#!/usr/bin/env bash
# Installer for skill-manager (macOS / Linux).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh | sh
#
# Portability note: this script is invoked verbatim as `sh` by the one-liner
# above, and `sh` is dash (not bash) on Debian/Ubuntu and many other systems.
# A shebang line has no effect when a script is explicitly run as `sh script`,
# and re-executing under bash from inside a piped `sh` process is not
# reliable (stdin has already been streamed into the running interpreter and
# there is no seekable `$0` to re-read). So rather than re-exec into bash,
# this script is written to be strict POSIX sh: no arrays, no `[[ ]]`, no
# `set -o pipefail` (dash rejects that option). It is still fully valid bash,
# so it also runs correctly when invoked directly as `bash install.sh` or
# `./install.sh` under the repo's usual bash shebang convention.
set -eu

REPO="sernst/skills"
BINARY_NAME="skill-manager"
GITHUB_API="https://api.github.com/repos/${REPO}/releases/latest"
GITHUB_DOWNLOAD="https://github.com/${REPO}/releases/download"
invocation_cwd="$(pwd -L)"

version_arg=""
dir_arg=""
dir_arg_supplied=0
yes_flag=0
force_flag=0
no_modify_path_flag=0

log() { printf '==> %s\n' "$1"; }
warn() { printf 'WARNING: %s\n' "$1" >&2; }
die() { printf 'error: %s\n' "$1" >&2; exit 1; }

have_cmd() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<'EOF'
skill-manager installer

Usage:
  install.sh [options]

Options:
  --version <tag>       Install a specific release (accepts "0.1.3" or "v0.1.3").
                         Defaults to the latest release.
  --dir <path>           Install destination directory.
                         Defaults to $HOME/.local/bin.
                         ~ uses $HOME; other relative paths use the invocation
                         directory. Paths are lexically normalized.
  --yes                  Skip the confirmation prompt and proceed with the
                         plan as resolved (does not force a same-version
                         reinstall; see --force).
  --force                Reinstall even if the target version is already
                         installed at the destination.
  --no-modify-path       Never edit a shell profile or PATH; just print the
                         manual export line.
  -h, --help             Show this help and exit.

Environment variables:
  SKILL_MANAGER_VERSION           Same as --version.
  SKILL_MANAGER_INSTALL_DIR       Same as --dir.
  SKILL_MANAGER_INSTALL_YES       Set to 1 to behave like --yes.
  SKILL_MANAGER_INSTALL_FORCE     Set to 1 to behave like --force.
  SKILL_MANAGER_NO_MODIFY_PATH    Set to 1 to behave like --no-modify-path.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      case "${2-}" in
        '') die "--version requires a non-empty value" ;;
      esac
      version_arg="$2"
      shift 2
      ;;
    --version=*)
      version_arg="${1#*=}"
      [ -n "$version_arg" ] || die "--version requires a non-empty value"
      shift
      ;;
    --dir)
      [ $# -ge 2 ] || die "--dir requires a non-empty value"
      dir_arg="$2"
      dir_arg_supplied=1
      shift 2
      ;;
    --dir=*)
      dir_arg="${1#*=}"
      dir_arg_supplied=1
      shift
      ;;
    --yes)
      yes_flag=1
      shift
      ;;
    --force)
      force_flag=1
      shift
      ;;
    --no-modify-path)
      no_modify_path_flag=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [ "${SKILL_MANAGER_INSTALL_YES:-0}" = "1" ]; then yes_flag=1; fi
if [ "${SKILL_MANAGER_INSTALL_FORCE:-0}" = "1" ]; then force_flag=1; fi
if [ "${SKILL_MANAGER_NO_MODIFY_PATH:-0}" = "1" ]; then no_modify_path_flag=1; fi

normalize_install_dir() {
  normalize_input="$1"
  if ! printf '%s' "$normalize_input" | grep -q '[^[:space:]]'; then
    die "install directory must not be empty"
  fi

  # Case patterns undergo tilde expansion themselves, so quote the literal
  # tilde patterns. Only exact ~ and ~/... are home-relative.
  # shellcheck disable=SC2088
  case "$normalize_input" in
    "~") normalize_input="$HOME" ;;
    "~/"*) normalize_input="${HOME}/${normalize_input#\~/}" ;;
  esac
  case "$normalize_input" in
    /*) : ;;
    *) normalize_input="${invocation_cwd}/${normalize_input}" ;;
  esac

  normalize_output=""
  normalize_old_ifs="$IFS"
  IFS='/'
  set -f
  # Intentional field splitting on slash implements lexical path traversal
  # without requiring the path to exist or resolving symlinks.
  # shellcheck disable=SC2086
  set -- $normalize_input
  set +f
  IFS="$normalize_old_ifs"
  for normalize_component do
    case "$normalize_component" in
      ''|.) ;;
      ..) normalize_output="${normalize_output%/*}" ;;
      *) normalize_output="${normalize_output}/${normalize_component}" ;;
    esac
  done
  printf '%s\n' "${normalize_output:-/}"
}

default_dest="${HOME}/.local/bin"

is_tty_available() {
  # Resolver tests must not consult the developer's controlling terminal.
  # Forced prompt tests still exercise the same fd 3 reader.
  if [ "${SKILL_MANAGER_TEST_RESOLVE_DIR:-0}" = "1" ]; then
    if [ "${SKILL_MANAGER_TEST_FORCE_INTERACTIVE:-0}" = "1" ]; then
      exec 3<&0
      return 0
    fi
    return 1
  fi

  # `[ -t 0 ]` is not sufficient: under `curl ... | sh`, fd 0 is the pipe
  # carrying the script itself, not a terminal, and may already be closed by
  # the time this runs. Test /dev/tty directly instead.
  #
  # A redirection failure on a POSIX "special built-in" (`:`, `exec`, ...) is
  # fatal to a non-interactive shell -- it terminates the whole script,
  # bypassing normal error handling -- so the probe must never redirect a
  # special built-in in the current shell. In a true headless environment (no
  # controlling terminal, e.g. CI/Docker without a pty) opening /dev/tty fails
  # with ENXIO even though `[ -r ]` reports it as permission-readable.
  # Contain that failure inside a subshell first (a subshell exiting on this
  # rule only ends the subshell, not the script), and only `exec` for real in
  # the current shell once openability is already confirmed.
  if ! [ -r /dev/tty ]; then return 1; fi
  if ! (: < /dev/tty) 2>/dev/null; then return 1; fi
  exec 3</dev/tty
}
interactive=0
if is_tty_available; then interactive=1; fi

select_install_dir() {
  if [ "$dir_arg_supplied" = "1" ]; then
    dest_dir="$dir_arg"
    dest_source="--dir"
  elif [ -n "${SKILL_MANAGER_INSTALL_DIR:-}" ]; then
    dest_dir="$SKILL_MANAGER_INSTALL_DIR"
    dest_source="\$SKILL_MANAGER_INSTALL_DIR"
  elif [ "$interactive" = "1" ]; then
    printf 'Install directory [%s]: ' "$default_dest" >&2
    IFS= read -r reply <&3 || reply=""
    dest_dir="${reply:-$default_dest}"
    dest_source="prompted value"
  else
    dest_dir="$default_dest"
    dest_source="default (no TTY detected)"
  fi
  dest_dir="$(normalize_install_dir "$dest_dir")"
}

normalize_path_entry() {
  normalize_path_input="$1"
  case "$normalize_path_input" in
    /*) : ;;
    *) printf '%s\n' "$normalize_path_input"; return ;;
  esac

  normalize_path_output=""
  normalize_path_old_ifs="$IFS"
  IFS='/'
  set -f
  # shellcheck disable=SC2086
  set -- $normalize_path_input
  set +f
  IFS="$normalize_path_old_ifs"
  for normalize_path_component do
    case "$normalize_path_component" in
      ''|.) ;;
      ..) normalize_path_output="${normalize_path_output%/*}" ;;
      *) normalize_path_output="${normalize_path_output}/${normalize_path_component}" ;;
    esac
  done
  printf '%s\n' "${normalize_path_output:-/}"
}

path_dir_matches() {
  # path_dir_matches <candidate-dir> <target-dir>
  # PATH entries are shell data, not installer inputs: do not expand a literal
  # tilde or reinterpret a relative entry through install-input normalization.
  # Only trust canonical comparison when both realpath calls succeed. If
  # either path does not exist, lexically clean absolute entries without
  # expanding or anchoring shell spellings such as "~" or relative entries.
  candidate="$1"
  target_dir="$2"
  [ "$candidate" = "$target_dir" ] && return 0
  if have_cmd realpath; then
    resolved_candidate="$(realpath "$candidate" 2>/dev/null)" || resolved_candidate=""
    resolved_target="$(realpath "$target_dir" 2>/dev/null)" || resolved_target=""
    if [ -n "$resolved_candidate" ] && [ -n "$resolved_target" ]; then
      [ "$resolved_candidate" = "$resolved_target" ]
      return $?
    fi
  fi
  normalized_candidate="$(normalize_path_entry "$candidate")"
  normalized_target="$(normalize_path_entry "$target_dir")"
  [ "$normalized_candidate" = "$normalized_target" ]
}

# Undocumented process-test hook. It deliberately runs before curl, release
# resolution, temporary files, profile checks, or any installation work.
if [ "${SKILL_MANAGER_TEST_RESOLVE_DIR:-0}" = "1" ]; then
  select_install_dir
  if [ "${SKILL_MANAGER_TEST_PATH_ENTRY+x}" = "x" ]; then
    if path_dir_matches "$SKILL_MANAGER_TEST_PATH_ENTRY" "$dest_dir"; then
      printf 'match\n'
    else
      printf 'no-match\n'
    fi
  else
    printf '%s\n' "$dest_dir"
  fi
  exit 0
fi

tmp_dir=""
cleanup() {
  if [ -n "$tmp_dir" ] && [ -d "$tmp_dir" ]; then
    rm -rf "$tmp_dir"
  fi
}
# `cleanup` runs exactly once, on EXIT, however the script ends. INT/TERM must
# not run cleanup directly and then fall back into the script: on a signal
# they only translate it into the conventional exit status (130/143) and
# `exit`, which itself triggers the EXIT trap. If INT/TERM also invoked
# cleanup directly, an interrupt during cleanup-sensitive work would run it
# twice and (more importantly) previously let execution continue past the
# trap instead of terminating, which is the defect being fixed here.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if ! have_cmd curl; then
  die "curl is required but was not found on PATH"
fi

fetch() {
  # fetch <url> <output-file>
  curl --fail --silent --show-error --location --retry 3 --proto '=https' --tlsv1.2 "$1" --output "$2"
}

fetch_text() {
  # fetch_text <url>  (prints body to stdout)
  curl --fail --silent --show-error --location --retry 3 --proto '=https' --tlsv1.2 "$1"
}

json_field() {
  # json_field <field> <json-text> -- crude but dependency-free extractor for
  # a top-level string field, sufficient for the GitHub releases response.
  printf '%s' "$2" | grep -o "\"$1\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | head -n1 | sed -E "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"([^\"]*)\"/\1/"
}

# extract_binary_version <captured --version output>
# Requires an anchored "skill-manager <x.y.z>" match so foreign or garbled
# output (a different tool that happens to be named skill-manager, truncated
# output, ...) is never mistaken for a real version.
extract_binary_version() {
  printf '%s\n' "$1" | grep -Eo "^${BINARY_NAME}[[:space:]]+[0-9]+\\.[0-9]+\\.[0-9]+" | grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' | head -n1
}

# shell_quote <string> -- prints a POSIX-sh single-quoted token safe to embed
# in a profile file. The '\'' idiom (close, escaped literal quote, reopen)
# also parses correctly inside fish single-quoted strings, so this is safe to
# reuse for the fish_add_path line too.
shell_quote() {
  quoted="$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
  printf "'%s'" "$quoted"
}

# --- 2. Resolve the release -------------------------------------------------

raw_version="${version_arg:-${SKILL_MANAGER_VERSION:-}}"
if [ -n "$raw_version" ]; then
  case "$raw_version" in
    v*) tag="$raw_version" ;;
    *) tag="v${raw_version}" ;;
  esac
  log "resolving release: using requested version ${tag}"
else
  log "resolving release: querying latest from GitHub API"
  api_response="$(fetch_text "$GITHUB_API")" || die "failed to query ${GITHUB_API}"
  tag="$(json_field tag_name "$api_response")"
  [ -n "$tag" ] || die "could not determine latest release tag from GitHub API response"
  log "resolving release: latest is ${tag}"
fi
version="${tag#v}"

# --- 3. Detect the platform --------------------------------------------------

uname_s="$(uname -s)"
uname_m="$(uname -m)"

case "$uname_m" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) die "unsupported CPU architecture: ${uname_m} (skill-manager ships x86_64/aarch64 builds only)" ;;
esac

case "$uname_s" in
  Darwin)
    platform="apple-darwin"
    archive_ext="tar.gz"
    ;;
  Linux)
    platform="unknown-linux-musl"
    archive_ext="tar.gz"
    ;;
  *)
    die "unsupported operating system: ${uname_s}. On Windows, use install.ps1 instead."
    ;;
esac

target="${arch}-${platform}"
log "detected platform: ${uname_s}/${uname_m} -> target ${target}"

asset="${BINARY_NAME}-${tag}-${target}.${archive_ext}"
asset_url="${GITHUB_DOWNLOAD}/${tag}/${asset}"
sums_url="${GITHUB_DOWNLOAD}/${tag}/SHA256SUMS"
log "resolved asset: ${asset}"

# --- 4. Download and verify --------------------------------------------------

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/skill-manager-install.XXXXXX")"
archive_path="${tmp_dir}/${asset}"
sums_path="${tmp_dir}/SHA256SUMS"

log "downloading ${asset_url}"
fetch "$asset_url" "$archive_path" || die "download failed: ${asset_url}"

log "downloading checksums"
fetch "$sums_url" "$sums_path" || die "download failed: ${sums_url}"

# find_checksum_entries <sums-file> <asset-filename>
# Parses SHA256SUMS into hash/name entries, tolerating the standard `*name`
# binary-mode marker and any leading path component, and prints one matching
# hash per line for an EXACT basename match (never a substring match).
find_checksum_entries() {
  match_count=0
  matched_hashes=""
  while IFS= read -r sums_line || [ -n "$sums_line" ]; do
    case "$sums_line" in
      ''|'#'*) continue ;;
    esac
    hash="${sums_line%%[[:space:]]*}"
    rest="${sums_line#"$hash"}"
    # strip the whitespace run separating hash and filename (one or two
    # spaces per the sha256sum text/binary-mode conventions)
    while [ -n "$rest" ]; do
      case "$rest" in
        [[:space:]]*) rest="${rest#?}" ;;
        *) break ;;
      esac
    done
    # strip the binary-mode "*" marker, if present
    case "$rest" in
      "*"*) rest="${rest#\*}" ;;
    esac
    # keep only the basename, discarding any leading path component
    name="${rest##*/}"
    if [ "$name" = "$2" ]; then
      match_count=$((match_count + 1))
      matched_hashes="${matched_hashes}${hash}
"
    fi
  done < "$1"
}

verify_checksum() {
  find_checksum_entries "$sums_path" "$asset"
  if [ "$match_count" -eq 0 ]; then
    die "no checksum entry found for ${asset} in SHA256SUMS; refusing to install an unverified archive"
  elif [ "$match_count" -gt 1 ]; then
    die "found ${match_count} ambiguous checksum entries for ${asset} in SHA256SUMS; refusing to install an unverified archive"
  fi
  expected="${matched_hashes%%
*}"
  if have_cmd sha256sum; then
    actual="$(sha256sum "$archive_path" | awk '{print $1}')"
  elif have_cmd shasum; then
    actual="$(shasum -a 256 "$archive_path" | awk '{print $1}')"
  else
    # This is the ONLY case permitted to continue unverified: there is
    # simply no hashing tool available on this machine to check against the
    # checksum entry we did find.
    warn "*** no sha256sum or shasum available on this machine; the downloaded archive could NOT be checksum-verified ***"
    return 0
  fi
  expected_lc=$(printf '%s' "$expected" | tr '[:upper:]' '[:lower:]')
  actual_lc=$(printf '%s' "$actual" | tr '[:upper:]' '[:lower:]')
  if [ "$expected_lc" != "$actual_lc" ]; then
    rm -f "$archive_path"
    die "checksum mismatch for ${asset}: expected ${expected_lc}, got ${actual_lc}"
  fi
  log "checksum verified: ${actual_lc}"
}
verify_checksum

# --- 5. Resolve the destination ---------------------------------------------

# Validate the destination early: reject empty/whitespace-only values, reject
# a path that already exists as a non-directory, and normalize all spellings
# to a clean lexical absolute path before any downstream behavior uses it.
select_install_dir
log "destination: using ${dest_source} ${dest_dir}"
if [ -e "$dest_dir" ] && [ ! -d "$dest_dir" ]; then
  die "install directory ${dest_dir} already exists and is not a directory"
fi

# --- 7. Detect an existing installation --------------------------------------

existing_version=""
existing_present=0
existing_executable=0
existing_path="${dest_dir}/${BINARY_NAME}"
# Presence is detected with -e (any file, not just an executable one) so a
# non-executable file occupying the destination is never narrated as a
# fresh install -- it is a real replacement, just of something that never
# ran as skill-manager. The version probe itself is still gated on -x since
# running a non-executable file isn't meaningful.
if [ -e "$existing_path" ]; then
  existing_present=1
  if [ -x "$existing_path" ]; then
    existing_executable=1
    if existing_version_output="$("$existing_path" --version 2>&1)"; then
      existing_version="$(extract_binary_version "$existing_version_output")"
    fi
  fi
  if [ -n "$existing_version" ]; then
    log "existing installation: found ${existing_version} at ${existing_path}"
  elif [ "$existing_executable" = "1" ]; then
    log "existing installation: found a binary at ${existing_path} that does not identify itself as ${BINARY_NAME} (foreign or broken binary); it will be replaced"
  else
    log "existing installation: found a non-executable file at ${existing_path} (not a working ${BINARY_NAME} install); it will be replaced"
  fi
else
  log "existing installation: none found at ${dest_dir}"
fi

dest_on_path=0
old_ifs="$IFS"
IFS=:
for p in $PATH; do
  [ -n "$p" ] || continue
  if path_dir_matches "$p" "$dest_dir"; then
    dest_on_path=1
    break
  fi
done
IFS="$old_ifs"

if [ "$dest_on_path" = "1" ]; then
  log "PATH check: ${dest_dir} is already on PATH"
else
  log "PATH check: ${dest_dir} is not on PATH"
fi

other_binary_dir=""
old_ifs="$IFS"
IFS=:
for p in $PATH; do
  [ -n "$p" ] || continue
  if [ -x "${p}/${BINARY_NAME}" ]; then
    other_binary_dir="$p"
    break
  fi
done
IFS="$old_ifs"

# --- 8. Resolve the PATH action and shell profile (before the plan) ---------

# The profile/PATH decision must be resolved BEFORE the plan is rendered and
# BEFORE the single confirmation prompt below, so the plan states the exact
# action that will be taken and no further PATH-specific prompting happens
# after the user has already said yes once.
shell_name="$(basename "${SHELL:-sh}")"
case "$shell_name" in
  zsh) profile="${HOME}/.zshrc" ;;
  bash)
    if [ -f "${HOME}/.bashrc" ]; then
      profile="${HOME}/.bashrc"
    elif [ "$uname_s" = "Darwin" ]; then
      profile="${HOME}/.bash_profile"
    else
      profile="${HOME}/.bashrc"
    fi
    ;;
  fish) profile="${HOME}/.config/fish/config.fish" ;;
  *) profile="${HOME}/.profile" ;;
esac

if [ "$dest_on_path" = "1" ]; then
  path_action="already"
elif [ "$no_modify_path_flag" = "1" ]; then
  path_action="skip"
else
  path_action="add"
fi

# --- 9. Plan before writing ---------------------------------------------------

if [ -n "$existing_version" ] && [ "$existing_version" = "$version" ]; then
  scenario="same-version"
elif [ -n "$existing_version" ]; then
  scenario="replace"
elif [ "$existing_present" = "1" ] && [ "$existing_executable" = "1" ]; then
  scenario="foreign"
elif [ "$existing_present" = "1" ]; then
  scenario="nonexec"
else
  scenario="fresh"
fi

echo
echo "Plan"
echo "  release:      ${tag}"
echo "  asset:        ${asset}"
echo "  destination:  ${dest_dir}/${BINARY_NAME}"
case "$scenario" in
  fresh) echo "  action:       new install" ;;
  replace) echo "  action:       replace ${existing_version} with ${version}" ;;
  same-version) echo "  action:       ${version} is already installed" ;;
  foreign) echo "  action:       replace unrecognized binary at destination with ${version}" ;;
  nonexec) echo "  action:       replace non-executable file at destination with ${version}" ;;
esac
case "$path_action" in
  already) echo "  PATH:         already on PATH" ;;
  add) echo "  PATH:         add ${dest_dir} to ${profile}" ;;
  skip) echo "  PATH:         leave PATH unchanged (--no-modify-path)" ;;
esac
echo

do_install=1
case "$scenario" in
  same-version)
    if [ "$force_flag" = "1" ]; then
      log "same-version reinstall forced via --force"
    elif [ "$interactive" = "1" ]; then
      printf 'skill-manager %s is already installed; reinstall anyway? [y/N] ' "$version" >&2
      IFS= read -r reply <&3 || reply=""
      case "$reply" in
        y|Y|yes|YES) : ;;
        *) do_install=0 ;;
      esac
    else
      log "no TTY detected; skipping reinstall of already-installed ${version} (use --force to reinstall)"
      do_install=0
    fi
    ;;
  *)
    if [ "$yes_flag" = "1" ]; then
      log "proceeding without prompt (--yes/SKILL_MANAGER_INSTALL_YES)"
    elif [ "$interactive" = "1" ]; then
      printf 'Proceed with install? [Y/n] ' >&2
      IFS= read -r reply <&3 || reply=""
      case "$reply" in
        n|N|no|NO) do_install=0 ;;
        *) : ;;
      esac
    else
      log "no TTY detected; proceeding with the plan above"
    fi
    ;;
esac

if [ "$do_install" != "1" ]; then
  if [ "$scenario" = "same-version" ]; then
    echo "skill-manager ${version} is already installed at ${dest_dir}/${BINARY_NAME}; skipping."
  else
    echo "Cancelled."
  fi
  exit 0
fi

# --- 10. Install atomically ---------------------------------------------------

log "extracting ${asset}"
extract_dir="${tmp_dir}/extract"
mkdir -p "$extract_dir"
tar -xzf "$archive_path" -C "$extract_dir"

binary_path="$(find "$extract_dir" -type f -name "$BINARY_NAME" | head -n1)"
[ -n "$binary_path" ] || die "could not locate the ${BINARY_NAME} binary inside ${asset}"

chmod +x "$binary_path"
if ! mkdir -p "$dest_dir" 2>"${tmp_dir}/mkdir.err"; then
  die "could not create install directory ${dest_dir}: $(cat "${tmp_dir}/mkdir.err" 2>/dev/null)"
fi

# Stage the new binary under its final directory (required so the later `mv`
# onto $existing_path is an atomic same-filesystem rename) and make it
# executable, then run and version-check THIS staged copy before anything
# about the previous install is touched. A no-exec mount, an AppLocker rule
# scoped to $dest_dir, or a mis-packaged release all fail here, before the
# working binary (if any) is replaced or PATH is touched -- that ordering is
# the fix: previously verification only happened at the very end, after the
# old binary was already gone and PATH may have already been updated.
staging_path="${dest_dir}/.${BINARY_NAME}.tmp.$$"
cp "$binary_path" "$staging_path"
chmod +x "$staging_path"

if ! staged_version_output="$("$staging_path" --version 2>&1)"; then
  rm -f "$staging_path"
  die "staged binary failed to run at ${staging_path}; the existing install (if any) at ${existing_path} was left untouched and PATH was not modified: ${staged_version_output}"
fi
staged_version="$(extract_binary_version "$staged_version_output")"
if [ "$staged_version" != "$version" ]; then
  rm -f "$staging_path"
  die "staged binary at ${staging_path} reports version '${staged_version:-unknown}', expected ${version}; the existing install (if any) at ${existing_path} was left untouched and PATH was not modified: ${staged_version_output}"
fi
log "staged binary verified: ${staged_version_output}"

# Back up whatever currently occupies $existing_path (working install,
# foreign file, or non-executable file) so it can be restored if the staged
# binary somehow fails once it is running under its real, final name.
backup_path=""
if [ "$existing_present" = "1" ]; then
  backup_path="${dest_dir}/.${BINARY_NAME}.prev.$$"
  cp "$existing_path" "$backup_path" || die "could not back up the existing file at ${existing_path} before replacing it"
fi

if ! mv -f "$staging_path" "$existing_path"; then
  rm -f "$staging_path"
  # The rename failed, so $existing_path still holds the original file
  # untouched; the backup copy is therefore redundant and must not be left
  # behind as a stray file.
  [ -z "$backup_path" ] || rm -f "$backup_path"
  die "could not move the verified staged binary into place at ${existing_path}; the existing install (if any) was left untouched and PATH was not modified"
fi

if ! installed_version_output="$("$existing_path" --version 2>&1)"; then
  if [ -n "$backup_path" ]; then
    mv -f "$backup_path" "$existing_path"
    die "installation verification failed after replacing the existing binary: ${existing_path} exited with an error; the previous binary has been restored and PATH was not modified: ${installed_version_output}"
  fi
  rm -f "$existing_path"
  die "installation verification failed: ${existing_path} exited with an error: ${installed_version_output}"
fi
installed_version="$(extract_binary_version "$installed_version_output")"
if [ "$installed_version" != "$version" ]; then
  if [ -n "$backup_path" ]; then
    mv -f "$backup_path" "$existing_path"
    die "installation verification failed after replacing the existing binary: ${existing_path} reports version '${installed_version:-unknown}', expected ${version}; the previous binary has been restored and PATH was not modified: ${installed_version_output}"
  fi
  rm -f "$existing_path"
  die "installation verification failed: ${existing_path} reports version '${installed_version:-unknown}', expected ${version}: ${installed_version_output}"
fi
[ -z "$backup_path" ] || rm -f "$backup_path"
log "installed ${BINARY_NAME} to ${existing_path}"

# --- 11. PATH -----------------------------------------------------------------

quoted_dest_dir="$(shell_quote "$dest_dir")"
path_export_line="export PATH=${quoted_dest_dir}:\"\$PATH\""

marker_start="# >>> skill-manager >>>"
marker_end="# <<< skill-manager <<<"

# build_path_block -- prints the exact marker block (start marker, PATH line,
# end marker) this run would write for the CURRENTLY resolved destination.
build_path_block() {
  block="${marker_start}
"
  if [ "$shell_name" = "fish" ]; then
    block="${block}fish_add_path ${quoted_dest_dir}
"
  else
    # shellcheck disable=SC2016 # $PATH is meant literally: it is expanded
    # later when the profile is sourced, not now.
    block="${block}export PATH=${quoted_dest_dir}:\"\$PATH\"
"
  fi
  block="${block}${marker_end}
"
  printf '%s' "$block"
}

# extract_marker_block <profile> -- prints the first COMPLETE owned marker
# block (marker_start line through marker_end line, inclusive) found in
# <profile>, and returns 0. Returns 1 and prints nothing if no complete block
# is present -- including a block whose marker_start was written but whose
# marker_end never landed (a partial block left by a run interrupted mid
# write), so that case is treated the same as "no block yet" rather than
# being trusted just because the marker text is present.
extract_marker_block() {
  [ -f "$1" ] || return 1
  in_block=0
  found=0
  block=""
  while IFS= read -r mline || [ -n "$mline" ]; do
    if [ "$in_block" = "1" ]; then
      block="${block}${mline}
"
      if [ "$mline" = "$marker_end" ]; then
        found=1
        break
      fi
    elif [ "$mline" = "$marker_start" ]; then
      in_block=1
      block="${mline}
"
    fi
  done < "$1"
  if [ "$found" = "1" ]; then
    printf '%s' "$block"
    return 0
  fi
  return 1
}

# rewrite_path_block <profile> <new-block> -- atomically replaces any owned
# marker block in <profile> (complete, or a partial one running to EOF) with
# <new-block>. Writes a temp file in the SAME directory as the profile and
# `mv`s it into place, so a reader never observes a half-written profile.
rewrite_path_block() {
  profile_file="$1"
  new_block="$2"
  profile_dir="$(dirname "$profile_file")"
  mkdir -p "$profile_dir"
  tmp_profile="${profile_file}.skill-manager-tmp.$$"
  : > "$tmp_profile"
  if [ -f "$profile_file" ]; then
    in_block=0
    while IFS= read -r mline || [ -n "$mline" ]; do
      if [ "$in_block" = "1" ]; then
        if [ "$mline" = "$marker_end" ]; then in_block=0; fi
        continue
      fi
      if [ "$mline" = "$marker_start" ]; then
        in_block=1
        continue
      fi
      printf '%s\n' "$mline"
    done < "$profile_file" >> "$tmp_profile"
  fi
  {
    echo ""
    printf '%s\n' "$new_block"
  } >> "$tmp_profile"
  mv -f "$tmp_profile" "$profile_file"
}

if [ "$path_action" = "add" ]; then
  wanted_block="$(build_path_block)"
  existing_block=""
  if [ -f "$profile" ]; then
    existing_block="$(extract_marker_block "$profile")" || existing_block=""
  fi
  if [ "$existing_block" = "$wanted_block" ]; then
    log "PATH block already present in ${profile} and points at ${dest_dir}; leaving it as-is"
  else
    rewrite_path_block "$profile" "$wanted_block"
    log "added ${dest_dir} to PATH in ${profile} (open a new terminal for this to take effect)"
  fi
fi

# The manual export line is printed whenever the destination is not active in
# THIS session's PATH -- which includes the case where we just appended it to
# a profile (the running shell has not re-sourced it yet) and the case where
# PATH was intentionally left unchanged. It is skipped only when the
# destination is already active in the current session's PATH.
if [ "$dest_on_path" != "1" ]; then
  echo
  if [ "$path_action" = "add" ]; then
    log "the profile change above applies to new terminals only; to use ${BINARY_NAME} in this session, run:"
  else
    log "PATH was not modified. To use ${BINARY_NAME} in this session, run:"
  fi
  echo "  ${path_export_line}"
fi

# --- 12. Shadow warning --------------------------------------------------------

if [ -n "$other_binary_dir" ] && ! path_dir_matches "$other_binary_dir" "$dest_dir"; then
  warn "another ${BINARY_NAME} was found earlier on PATH at ${other_binary_dir}/${BINARY_NAME}"
  warn "it will shadow the version just installed at ${existing_path} until PATH order is fixed"
fi

# --- 13/14. Report --------------------------------------------------------------

echo
echo "skill-manager installed: ${existing_path}"
echo "${installed_version_output}"
