# Install skill-manager

This file is instructions for an AI agent acting on a user's behalf. Run the
appropriate installer below; do not ask the user to perform its prompts.

## Agent instructions

1. Detect the user's operating system. Use `install.ps1` on Windows and
   `install.sh` on macOS or Linux. Stop and report an unsupported platform.
2. Choose an explicit, user-writable install directory. The normal defaults are
   `%LOCALAPPDATA%\Programs\skill-manager` on Windows and `$HOME/.local/bin` on
   macOS and Linux. `~` and `~/...` use the active home; every other relative
   path uses the installer's invocation directory. The installer displays and
   uses a lexically normalized absolute path.
3. Use the non-interactive overrides: install directory, assume-yes, and
   suppress PATH modification. The installers detect no TTY and use defaults,
   but explicit settings let you control and report the result. Add `--force`
   (or `-Force`) only when a same-version reinstall is required.

On macOS and Linux, pass the options through the pipe:

```sh
install_dir="$HOME/.local/bin"
curl -fsSL https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.sh |
  sh -s -- --dir "$install_dir" --yes --no-modify-path
"$install_dir/skill-manager" --version
PATH="$install_dir:$PATH" skill-manager --version
```

On Windows, `irm | iex` cannot take parameters. Set the environment-variable
overrides first, then run the one-liner:

```powershell
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\skill-manager'
$env:SKILL_MANAGER_INSTALL_DIR = $installDir
$env:SKILL_MANAGER_INSTALL_YES = '1'
$env:SKILL_MANAGER_NO_MODIFY_PATH = '1'
irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex
& (Join-Path $installDir 'skill-manager.exe') --version
$env:Path = "$installDir;$env:Path"
skill-manager --version
```

The optional version override is `--version <tag>` on macOS/Linux or
`SKILL_MANAGER_VERSION` on Windows. Windows verifies release downloads against
`SHA256SUMS`. The POSIX installer does so when `sha256sum` or `shasum` is
available and warns prominently if neither is installed. Never bypass a
checksum mismatch.

## Verify and report

Run the installed binary with `--version` as shown above. Tell the user the
installed version and directory. Because PATH modification was suppressed, the
directory may not yet be on `PATH`: open a new terminal after adding it, then
confirm with `skill-manager --version` (`Get-Command skill-manager` on
Windows, or `command -v skill-manager` on macOS/Linux).

On a checksum mismatch, stop and report it. On an unsupported platform, report
that no release installer is available. If the chosen directory is not
writable, choose another user-writable directory and rerun with the explicit
directory override; if none is available, report the permission failure and
the directory that was attempted.
