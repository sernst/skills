# Installer for skill-manager (Windows).
#
# Usage:
#   powershell -c "irm https://raw.githubusercontent.com/sernst/skills/main/clis/skill-manager/install.ps1 | iex"
#
# Write-Host is used deliberately throughout: this is an interactive console
# installer, not a function meant to be composed in a pipeline, so narration
# belongs on the console rather than the output stream. The suppression below
# documents that as a decision, not an oversight.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'Interactive installer narrates progress directly to the console; Write-Output would pollute the success-value pipeline and gives no control over when text appears relative to prompts.')]
param(
    [string] $Version,
    [string] $Dir,
    [switch] $Yes,
    [switch] $Force,
    [switch] $NoModifyPath,
    [switch] $Help
)

$ErrorActionPreference = 'Stop'

function Show-Usage {
    @'
skill-manager installer

Usage:
  install.ps1 [-Version <tag>] [-Dir <path>] [-Yes] [-Force] [-NoModifyPath] [-Help]

Options:
  -Version <tag>    Install a specific release (accepts "0.1.3" or "v0.1.3").
                    Defaults to the latest release.
  -Dir <path>       Install destination directory.
                    Defaults to $env:LOCALAPPDATA\Programs\skill-manager.
  -Yes              Skip the confirmation prompt and proceed with the plan as
                    resolved (does not force a same-version reinstall; see
                    -Force).
  -Force            Reinstall even if the target version is already installed
                    at the destination.
  -NoModifyPath     Never edit the user PATH; just print the manual command.
  -Help             Show this help and exit.

Environment variables:
  SKILL_MANAGER_VERSION           Same as -Version.
  SKILL_MANAGER_INSTALL_DIR       Same as -Dir.
  SKILL_MANAGER_INSTALL_YES       Set to 1 to behave like -Yes.
  SKILL_MANAGER_INSTALL_FORCE     Set to 1 to behave like -Force.
  SKILL_MANAGER_NO_MODIFY_PATH    Set to 1 to behave like -NoModifyPath.
'@ | Write-Output
}

if ($Help) {
    Show-Usage
    return
}

# Older Windows PowerShell 5.1 hosts may default to TLS 1.0, which GitHub's
# endpoints reject.
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    Write-Verbose "could not force TLS 1.2 (older .NET?): $($_.Exception.Message)"
}

$repo = 'sernst/skills'
$binaryName = 'skill-manager'
$githubApi = "https://api.github.com/repos/$repo/releases/latest"
$githubDownload = "https://github.com/$repo/releases/download"

function Write-Step {
    param([string] $Message)
    Write-Host "==> $Message"
}

function Write-Warn {
    param([string] $Message)
    Write-Host "WARNING: $Message" -ForegroundColor Yellow
}

function Fail {
    param([string] $Message)
    Write-Host "error: $Message" -ForegroundColor Red
    exit 1
}

function Restore-PreviousBinary {
    # Move the backup back over the destination and confirm it actually landed
    # before claiming the previous binary was restored. A silent failure here
    # would otherwise tell the user they are safe while the broken upgrade is
    # still installed and their working binary survives only as a hidden backup.
    param(
        [string] $BackupPath,
        [string] $BinaryPath,
        [string] $Reason
    )
    try {
        Move-Item -LiteralPath $BackupPath -Destination $BinaryPath -Force -ErrorAction Stop
    } catch {
        Fail "$Reason; restoring the previous binary FAILED ($($_.Exception.Message)). Your previous working binary is preserved at $BackupPath -- move it back to $BinaryPath manually to recover. PATH was not modified"
    }
    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        Fail "$Reason; restoring the previous binary FAILED (no file is present at $BinaryPath afterward). Your previous working binary is preserved at $BackupPath -- move it back to $BinaryPath manually to recover. PATH was not modified"
    }
    Fail "$Reason; the previous binary has been restored and PATH was not modified"
}

function Invoke-DownloadWithRetry {
    # `-MaximumRetryCount` on Invoke-WebRequest requires PowerShell 6+; this
    # script must also run on Windows PowerShell 5.1, so retry manually.
    param(
        [string] $Uri,
        [string] $OutFile,
        [int] $MaxAttempts = 3
    )
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing
            return
        } catch {
            if ($attempt -ge $MaxAttempts) { throw }
            Start-Sleep -Seconds ([Math]::Min(2 * $attempt, 5))
        }
    }
}

function Test-InteractiveHost {
    # A piped `irm url | iex` invocation, or a CI runner, may not have a real
    # console to prompt against even though a "host" object exists. Guard
    # every probe in try/catch: some hosts throw rather than return false.
    try {
        if (-not [Environment]::UserInteractive) { return $false }
    } catch { return $false }
    try {
        if ([Console]::IsInputRedirected) { return $false }
    } catch { return $false }
    try {
        if ($Host.Name -eq 'Default Host' -or $null -eq $Host.UI -or $null -eq $Host.UI.RawUI) { return $false }
    } catch { return $false }
    return $true
}

$yesFlag = [bool]$Yes -or ($env:SKILL_MANAGER_INSTALL_YES -eq '1')
$forceFlag = [bool]$Force -or ($env:SKILL_MANAGER_INSTALL_FORCE -eq '1')
$noModifyPathFlag = [bool]$NoModifyPath -or ($env:SKILL_MANAGER_NO_MODIFY_PATH -eq '1')
$interactive = Test-InteractiveHost

$tempDir = $null

try {
    # --- 2. Resolve the release ------------------------------------------------

    $rawVersion = if ($Version) { $Version } elseif ($env:SKILL_MANAGER_VERSION) { $env:SKILL_MANAGER_VERSION } else { $null }
    if ($rawVersion) {
        $tag = if ($rawVersion.StartsWith('v')) { $rawVersion } else { "v$rawVersion" }
        Write-Step "resolving release: using requested version $tag"
    } else {
        Write-Step 'resolving release: querying latest from GitHub API'
        try {
            $release = Invoke-RestMethod -Uri $githubApi -UseBasicParsing -Headers @{ 'User-Agent' = 'skill-manager-installer' }
        } catch {
            Fail "failed to query $githubApi : $($_.Exception.Message)"
        }
        $tag = $release.tag_name
        if (-not $tag) { Fail 'could not determine latest release tag from GitHub API response' }
        Write-Step "resolving release: latest is $tag"
    }
    $resolvedVersion = $tag.TrimStart('v')

    # --- 3. Detect the platform -------------------------------------------------

    $archRaw = $env:PROCESSOR_ARCHITECTURE
    try {
        $archRaw = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch {
        Write-Verbose "RuntimeInformation.OSArchitecture unavailable, using `$env:PROCESSOR_ARCHITECTURE: $($_.Exception.Message)"
    }
    switch -Regex ($archRaw) {
        'ARM64|Arm64|aarch64' { $arch = 'aarch64' }
        'AMD64|X64|x86_64' { $arch = 'x86_64' }
        default { Fail "unsupported CPU architecture: $archRaw (skill-manager ships x86_64/aarch64 Windows builds only)" }
    }

    $target = "$arch-pc-windows-msvc"
    Write-Step "detected platform: Windows/$archRaw -> target $target"

    $asset = "$binaryName-$tag-$target.zip"
    $assetUrl = "$githubDownload/$tag/$asset"
    $sumsUrl = "$githubDownload/$tag/SHA256SUMS"
    Write-Step "resolved asset: $asset"

    # --- 4. Download and verify --------------------------------------------------

    $tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("skill-manager-install-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tempDir -Force | Out-Null
    $archivePath = Join-Path $tempDir $asset
    $sumsPath = Join-Path $tempDir 'SHA256SUMS'

    Write-Step "downloading $assetUrl"
    try {
        Invoke-DownloadWithRetry -Uri $assetUrl -OutFile $archivePath
    } catch {
        Fail "download failed: $assetUrl : $($_.Exception.Message)"
    }

    Write-Step 'downloading checksums'
    try {
        Invoke-DownloadWithRetry -Uri $sumsUrl -OutFile $sumsPath
    } catch {
        Fail "download failed: $sumsUrl : $($_.Exception.Message)"
    }

    # Parse SHA256SUMS into hash/name entries, tolerating the standard
    # "*name" binary-mode marker and any leading path component, and require
    # an EXACT basename match (never a substring match). A missing or
    # ambiguous entry aborts the install -- unlike the Unix installer, this
    # script always has a hashing tool available (Get-FileHash is built in),
    # so there is no "unverifiable" fallback path here.
    $sumsLines = Get-Content -Path $sumsPath
    $matchedHashes = @()
    foreach ($line in $sumsLines) {
        if (-not $line -or $line.Trim() -eq '') { continue }
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Count -lt 2) { continue }
        $hash = $parts[0]
        $name = $parts[1].TrimStart('*')
        $segments = $name -split '[\\/]'
        $base = $segments[$segments.Count - 1]
        if ($base -eq $asset) { $matchedHashes += $hash }
    }
    if ($matchedHashes.Count -eq 0) {
        Fail "no checksum entry found for $asset in SHA256SUMS; refusing to install an unverified archive"
    } elseif ($matchedHashes.Count -gt 1) {
        Fail "found $($matchedHashes.Count) ambiguous checksum entries for $asset in SHA256SUMS; refusing to install an unverified archive"
    }
    $expected = $matchedHashes[0].ToLowerInvariant()
    $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Remove-Item -Path $archivePath -Force -ErrorAction SilentlyContinue
        Fail "checksum mismatch for $asset : expected $expected, got $actual"
    }
    Write-Step "checksum verified: $actual"

    # --- 5. Resolve the destination ----------------------------------------------

    $defaultDest = Join-Path $env:LOCALAPPDATA 'Programs\skill-manager'

    if ($Dir) {
        $destDir = $Dir
        Write-Step "destination: using -Dir $destDir"
    } elseif ($env:SKILL_MANAGER_INSTALL_DIR) {
        $destDir = $env:SKILL_MANAGER_INSTALL_DIR
        Write-Step "destination: using `$env:SKILL_MANAGER_INSTALL_DIR $destDir"
    } elseif ($interactive) {
        $reply = Read-Host "Install directory [$defaultDest]"
        $destDir = if ($reply) { $reply } else { $defaultDest }
        Write-Step "destination: using prompted value $destDir"
    } else {
        $destDir = $defaultDest
        Write-Step "destination: no interactive host detected, using default $destDir"
    }

    # Validate the destination early: reject empty/whitespace-only values,
    # reject a path that already exists as a non-directory file, and
    # normalize a relative path to absolute so it is never persisted into
    # PATH as an unusable relative entry.
    if ([string]::IsNullOrWhiteSpace($destDir)) {
        Fail 'install directory must not be empty'
    }
    if (-not [System.IO.Path]::IsPathRooted($destDir)) {
        $destDir = Join-Path (Get-Location).Path $destDir
    }
    if ((Test-Path -LiteralPath $destDir) -and -not (Test-Path -LiteralPath $destDir -PathType Container)) {
        Fail "install directory $destDir already exists and is not a directory"
    }

    # --- 7. Detect an existing installation ---------------------------------------

    $binaryPath = Join-Path $destDir "$binaryName.exe"
    $existingVersion = $null
    $existingPresent = Test-Path -LiteralPath $binaryPath
    if ($existingPresent) {
        try {
            $verOut = & $binaryPath --version 2>&1
            $verExitCode = $LASTEXITCODE
            $verText = ($verOut | Out-String).Trim()
            # Require exit status zero AND an anchored "skill-manager <version>"
            # match, so a foreign binary (different tool, same file name) or a
            # broken/truncated one is never mistaken for a real installation.
            if ($verExitCode -eq 0 -and $verText -match "^$([Regex]::Escape($binaryName))\s+(\d+\.\d+\.\d+)") {
                $existingVersion = $Matches[1]
            }
        } catch {
            Write-Verbose "existing binary at $binaryPath did not run cleanly: $($_.Exception.Message)"
        }
        if ($existingVersion) {
            Write-Step "existing installation: found $existingVersion at $binaryPath"
        } else {
            Write-Step "existing installation: found a binary at $binaryPath that does not identify itself as $binaryName (foreign or broken binary); it will be replaced"
        }
    } else {
        Write-Step "existing installation: none found at $destDir"
    }

    function Get-UserPathEntry {
        $current = [Environment]::GetEnvironmentVariable('Path', 'User')
        if (-not $current) { return @() }
        # Force an array: PowerShell unwraps a single-element pipeline result
        # into a bare string, and `$entries + $destDir` would then silently
        # perform string concatenation instead of an array append.
        return @($current -split ';' | Where-Object { $_ -ne '' })
    }

    function Test-DirOnPath {
        param([string] $CandidateDir)
        $normalizedCandidate = $CandidateDir.TrimEnd('\')
        foreach ($entry in ($env:Path -split ';')) {
            if ($entry -and $entry.TrimEnd('\') -ieq $normalizedCandidate) { return $true }
        }
        return $false
    }

    $destOnPath = Test-DirOnPath -CandidateDir $destDir
    if ($destOnPath) {
        Write-Step "PATH check: $destDir is already on PATH"
    } else {
        Write-Step "PATH check: $destDir is not on PATH"
    }

    $otherCommand = Get-Command $binaryName -All -ErrorAction SilentlyContinue |
        Where-Object { (Split-Path -Parent $_.Source).TrimEnd('\') -ne $destDir.TrimEnd('\') } |
        Select-Object -First 1

    # --- 8. Resolve the PATH action (before the plan) ---------------------------

    # The PATH decision must be resolved BEFORE the plan is rendered and
    # BEFORE the single confirmation prompt below, so the plan states the
    # exact action that will be taken and no further PATH-specific prompting
    # happens after the user has already said yes once.
    $pathAction = if ($destOnPath) { 'already' } elseif ($noModifyPathFlag) { 'skip' } else { 'add' }

    # --- 9. Plan before writing ------------------------------------------------

    $scenario = if ($existingVersion -and $existingVersion -eq $resolvedVersion) { 'same-version' }
                elseif ($existingVersion) { 'replace' }
                elseif ($existingPresent) { 'foreign' }
                else { 'fresh' }

    Write-Host ''
    Write-Host 'Plan'
    Write-Host "  release:      $tag"
    Write-Host "  asset:        $asset"
    Write-Host "  destination:  $binaryPath"
    switch ($scenario) {
        'fresh' { Write-Host '  action:       new install' }
        'replace' { Write-Host "  action:       replace $existingVersion with $resolvedVersion" }
        'same-version' { Write-Host "  action:       $resolvedVersion is already installed" }
        'foreign' { Write-Host "  action:       replace unrecognized binary at destination with $resolvedVersion" }
    }
    switch ($pathAction) {
        'already' { Write-Host '  PATH:         already on PATH' }
        'add' { Write-Host "  PATH:         add $destDir to the User PATH" }
        'skip' { Write-Host '  PATH:         leave PATH unchanged (-NoModifyPath)' }
    }
    Write-Host ''

    $doInstall = $true
    switch ($scenario) {
        'same-version' {
            if ($forceFlag) {
                Write-Step 'same-version reinstall forced via -Force/SKILL_MANAGER_INSTALL_FORCE'
            } elseif ($interactive) {
                $reply = Read-Host "skill-manager $resolvedVersion is already installed; reinstall anyway? [y/N]"
                if ($reply -notmatch '^(y|yes)$') { $doInstall = $false }
            } else {
                Write-Step "no interactive host detected; skipping reinstall of already-installed $resolvedVersion (use -Force to reinstall)"
                $doInstall = $false
            }
        }
        default {
            if ($yesFlag) {
                Write-Step 'proceeding without prompt (-Yes/SKILL_MANAGER_INSTALL_YES)'
            } elseif ($interactive) {
                $reply = Read-Host 'Proceed with install? [Y/n]'
                if ($reply -match '^(n|no)$') { $doInstall = $false }
            } else {
                Write-Step 'no interactive host detected; proceeding with the plan above'
            }
        }
    }

    if (-not $doInstall) {
        if ($scenario -eq 'same-version') {
            Write-Host "skill-manager $resolvedVersion is already installed at $binaryPath; skipping."
        } else {
            Write-Host 'Cancelled.'
        }
        return
    }

    # --- 10. Install atomically --------------------------------------------------

    Write-Step "extracting $asset"
    $extractDir = Join-Path $tempDir 'extract'
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir -Force

    $binaryFile = Get-ChildItem -Path $extractDir -Recurse -Filter "$binaryName.exe" -File | Select-Object -First 1
    if (-not $binaryFile) { Fail "could not locate the $binaryName.exe binary inside $asset" }

    try {
        New-Item -ItemType Directory -Path $destDir -Force -ErrorAction Stop | Out-Null
    } catch {
        Fail "could not create install directory $destDir : $($_.Exception.Message)"
    }

    # The staging file MUST keep the .exe extension: Windows resolves how to
    # launch a file by its extension, and invoking an extension-less path
    # directly can fall back to a shell "Open With" prompt that hangs a
    # non-interactive install forever instead of failing cleanly.
    $stagingPath = Join-Path $destDir ".$binaryName.tmp.$PID.exe"
    try {
        Copy-Item -LiteralPath $binaryFile.FullName -Destination $stagingPath -Force -ErrorAction Stop
    } catch {
        Fail "could not stage $binaryName at $stagingPath : $($_.Exception.Message)"
    }

    # Verify the STAGED copy -- run it and confirm it reports the expected
    # version -- BEFORE anything about the previous install is touched. A
    # locked-down destination (execution policy / AppLocker), or a
    # mis-packaged release, fails here, before the working binary (if any)
    # is replaced or PATH is touched. This ordering is the fix: previously
    # verification only happened at the very end, after the old binary was
    # already gone and PATH may have already been updated.
    try {
        $stagedVersionOutput = & $stagingPath --version 2>&1
        $stagedExitCode = $LASTEXITCODE
    } catch {
        Remove-Item -LiteralPath $stagingPath -Force -ErrorAction SilentlyContinue
        Fail "staged binary failed to run at $stagingPath : $($_.Exception.Message); the existing install (if any) at $binaryPath was left untouched and PATH was not modified"
    }
    $stagedVersionText = ($stagedVersionOutput | Out-String).Trim()
    if ($stagedExitCode -ne 0) {
        Remove-Item -LiteralPath $stagingPath -Force -ErrorAction SilentlyContinue
        Fail "staged binary failed to run at $stagingPath (exit code $stagedExitCode); the existing install (if any) at $binaryPath was left untouched and PATH was not modified: $stagedVersionText"
    }
    $stagedVersion = $null
    if ($stagedVersionText -match "^$([Regex]::Escape($binaryName))\s+(\d+\.\d+\.\d+)") { $stagedVersion = $Matches[1] }
    if ($stagedVersion -ne $resolvedVersion) {
        Remove-Item -LiteralPath $stagingPath -Force -ErrorAction SilentlyContinue
        Fail "staged binary at $stagingPath reports version '$stagedVersion' but expected $resolvedVersion; the existing install (if any) at $binaryPath was left untouched and PATH was not modified: $stagedVersionText"
    }
    Write-Step "staged binary verified: $stagedVersionText"

    # Back up whatever currently occupies $binaryPath (working install or
    # foreign/broken binary) so it can be restored if the staged binary
    # somehow fails once it is running under its real, final name.
    $backupPath = $null
    if ($existingPresent) {
        $backupPath = Join-Path $destDir ".$binaryName.prev.$PID.exe"
        try {
            Copy-Item -LiteralPath $binaryPath -Destination $backupPath -Force -ErrorAction Stop
        } catch {
            Remove-Item -LiteralPath $stagingPath -Force -ErrorAction SilentlyContinue
            Fail "could not back up the existing file at $binaryPath before replacing it: $($_.Exception.Message)"
        }
    }

    try {
        Move-Item -LiteralPath $stagingPath -Destination $binaryPath -Force
    } catch {
        Remove-Item -LiteralPath $stagingPath -Force -ErrorAction SilentlyContinue
        # The rename failed, so $binaryPath still holds the original file
        # untouched; the backup copy is redundant and must not be left behind.
        if ($backupPath) { Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue }
        Fail "could not replace $binaryPath : it may be in use by a running skill-manager process. Close it and retry. ($($_.Exception.Message))"
    }

    try {
        $installedVersionOutput = & $binaryPath --version 2>&1
        $installedExitCode = $LASTEXITCODE
    } catch {
        if ($backupPath) {
            Restore-PreviousBinary -BackupPath $backupPath -BinaryPath $binaryPath -Reason "installation verification failed after replacing the existing binary: $binaryPath failed to run: $($_.Exception.Message)"
        }
        Remove-Item -LiteralPath $binaryPath -Force -ErrorAction SilentlyContinue
        Fail "installation verification failed: $binaryPath failed to run: $($_.Exception.Message)"
    }
    $installedVersionText = ($installedVersionOutput | Out-String).Trim()
    if ($installedExitCode -ne 0) {
        if ($backupPath) {
            Restore-PreviousBinary -BackupPath $backupPath -BinaryPath $binaryPath -Reason "installation verification failed after replacing the existing binary: $binaryPath exited with code $installedExitCode : $installedVersionText"
        }
        Remove-Item -LiteralPath $binaryPath -Force -ErrorAction SilentlyContinue
        Fail "installation verification failed: $binaryPath exited with code $installedExitCode : $installedVersionText"
    }
    $installedVersion = $null
    if ($installedVersionText -match "^$([Regex]::Escape($binaryName))\s+(\d+\.\d+\.\d+)") { $installedVersion = $Matches[1] }
    if ($installedVersion -ne $resolvedVersion) {
        if ($backupPath) {
            Restore-PreviousBinary -BackupPath $backupPath -BinaryPath $binaryPath -Reason "installation verification failed after replacing the existing binary: $binaryPath reports version '$installedVersion' but expected $resolvedVersion : $installedVersionText"
        }
        Remove-Item -LiteralPath $binaryPath -Force -ErrorAction SilentlyContinue
        Fail "installation verification failed: $binaryPath reports version '$installedVersion' but expected $resolvedVersion : $installedVersionText"
    }
    if ($backupPath) { Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue }
    Write-Step "installed $binaryName to $binaryPath"

    # --- 11. PATH ----------------------------------------------------------------

    $manualPathLine = "`$env:PATH = `"$destDir;`$env:PATH`""

    if ($pathAction -eq 'add') {
        # Read the raw User-scope value, not $env:PATH, which is the
        # process's merged Machine+User view and would duplicate the
        # Machine entries back into the User value on write-back.
        $entries = @(Get-UserPathEntry)
        $alreadyPresent = $entries | Where-Object { $_.TrimEnd('\') -ieq $destDir.TrimEnd('\') }
        if ($alreadyPresent) {
            Write-Step 'PATH block already present in the User environment; leaving it as-is'
        } else {
            $updated = (@($entries) + $destDir) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $updated, 'User')
            Write-Step "added $destDir to the User PATH (open a new terminal for this to take effect)"
        }
    }

    # The manual PATH line is printed whenever the destination is not active
    # in THIS session's PATH -- which includes the case where we just added
    # it to the User PATH (the running process has not picked it up yet) and
    # the case where PATH was intentionally left unchanged. It is skipped
    # only when the destination is already active in the current session's
    # PATH.
    if (-not $destOnPath) {
        Write-Host ''
        if ($pathAction -eq 'add') {
            Write-Step 'the User PATH change above applies to new terminals only; to use skill-manager in this session, run:'
        } else {
            Write-Step 'PATH was not modified. To use skill-manager in this session, run:'
        }
        Write-Host "  $manualPathLine"
    }

    # --- 12. Shadow warning --------------------------------------------------------

    if ($otherCommand) {
        Write-Warn "another $binaryName was found earlier on PATH at $($otherCommand.Source)"
        Write-Warn "it will shadow the version just installed at $binaryPath until PATH order is fixed"
    }

    # --- 13/14. Report --------------------------------------------------------------

    Write-Host ''
    Write-Host "skill-manager installed: $binaryPath"
    Write-Host $installedVersionText
} finally {
    if ($tempDir -and (Test-Path -LiteralPath $tempDir)) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
