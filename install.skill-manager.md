# Install skill-manager and the managing-skills skill

This is an imperative playbook for an agent. A user may paste this entire file
into an agent session and ask you to install it.

## Agent instructions

1. Determine the current harness from explicit system/application identity, not
   from the working directory or files you happen to see:
   - Codex or another OpenAI harness: `codex`
   - Claude Code: `claude`
   - Google Antigravity: `antigravity`
2. If system identity does not make the harness clear, ask the user before
   running anything.
3. Run exactly one platform block below with the harness value. The blocks
   install or safely upgrade the latest stable release, configure `PATH`, add or
   reuse the repository source, dry-run and deploy only `managing-skills`,
   verify every target, and print the installed `SKILL.md`.
4. Read the printed `SKILL.md` into the current session and follow it for later
   skill-management requests. Tell the user that harnesses which scan skills
   only at startup require a new session before automatic skill discovery.
5. Do not use the user's real skill-manager state for testing the playbook. A
   test must set `SKILL_MANAGER_HOME` and use a temporary CWD.

The managed binary locations are:

- Windows: `%LOCALAPPDATA%\skill-manager\bin`
- macOS/Linux: `$HOME/.local/share/skill-manager/bin`

Each contains `install-provenance.json`. An existing file at the managed
destination without valid provenance is foreign: stop instead of overwriting
it. The binary and provenance must be a matched pair of ordinary files;
directories, links/reparse points, special files, and one-sided pairs are
rejected before staging or rollback. A marked same/newer stable binary is
reused; an older marked binary is upgraded; a newer one is never downgraded.
Candidates are fully validated in a temporary stage before a guarded
same-filesystem replacement; every move is state-tracked so any failure
restores and revalidates the prior pair or reports a fatal recovery error.
If another executable is already on `PATH`, the managed directory is prepended
and the shadowed path is reported.

The deployment recipes select target names through the explicit `targets`
array: `shared`, plus `claude` or `antigravity` for those native harnesses.
Explicit name selection deliberately installs to a disabled built-in target,
as the CLI permits, but does not enable or otherwise alter target
configuration.

Release asset mapping is exact:

| Platform | CPU | libc | Target and archive |
| --- | --- | --- | --- |
| Windows | x86-64 | — | `x86_64-pc-windows-msvc.zip` |
| Windows | ARM64 | — | `aarch64-pc-windows-msvc.zip` |
| macOS | Intel | — | `x86_64-apple-darwin.tar.gz` |
| macOS | Apple silicon | — | `aarch64-apple-darwin.tar.gz` |
| Linux | x86-64 | glibc | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux | ARM64 | glibc | `aarch64-unknown-linux-gnu.tar.gz` |
| Linux | x86-64 | musl | `x86_64-unknown-linux-musl.tar.gz` |
| Linux | ARM64 | musl | `aarch64-unknown-linux-musl.tar.gz` |

The full asset name is `skill-manager-v<VERSION>-<TARGET>.<ARCHIVE>`.

## Windows PowerShell

Run this complete block in PowerShell 7 or Windows PowerShell 5.1. Set
`$Harness` from explicit application identity before execution.

```powershell
$ErrorActionPreference = 'Stop'
$Harness = $env:SKILL_MANAGER_HARNESS # Agent: set explicitly from application identity.
if ($Harness -notin @('codex', 'claude', 'antigravity')) {
    throw 'Set SKILL_MANAGER_HARNESS to codex, claude, or antigravity from explicit application identity.'
}

$Repository = 'sernst/skills'
$SourceIdentity = 'sernst/skills/skills'
$RequestedSourceName = 'sernst-skills'
$ManagedDirectory = Join-Path $env:LOCALAPPDATA 'skill-manager\bin'
$ManagedBinary = Join-Path $ManagedDirectory 'skill-manager.exe'
$ProvenancePath = Join-Path $ManagedDirectory 'install-provenance.json'
$PreviousPathCommand = Get-Command skill-manager -ErrorAction SilentlyContinue
$PreviousPath = if ($PreviousPathCommand) { $PreviousPathCommand.Source } else { $null }

function ConvertTo-SemVer {
    param([Parameter(Mandatory = $true)][string]$Value)
    if ($Value -notmatch '^v?(?<major>0|[1-9][0-9]*)\.(?<minor>0|[1-9][0-9]*)\.(?<patch>0|[1-9][0-9]*)$') {
        throw "Not a stable semantic version: $Value"
    }
    [pscustomobject]@{
        Text = "$($Matches.major).$($Matches.minor).$($Matches.patch)"
        Major = [uint64]$Matches.major
        Minor = [uint64]$Matches.minor
        Patch = [uint64]$Matches.patch
    }
}

function Compare-SemVer {
    param($Left, $Right)
    foreach ($Field in @('Major', 'Minor', 'Patch')) {
        if ($Left.$Field -lt $Right.$Field) { return -1 }
        if ($Left.$Field -gt $Right.$Field) { return 1 }
    }
    return 0
}

function Get-NormalizedPathEntry {
    param([AllowEmptyString()][string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    $Expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim().Trim('"'))
    try {
        return [IO.Path]::GetFullPath($Expanded).TrimEnd(
            [IO.Path]::DirectorySeparatorChar,
            [IO.Path]::AltDirectorySeparatorChar
        )
    } catch {
        return $Expanded.TrimEnd('\', '/')
    }
}

function Get-ManagedPathItem {
    param([Parameter(Mandatory = $true)][string]$Path)
    try { return Get-Item -LiteralPath $Path -Force -ErrorAction Stop }
    catch [System.Management.Automation.ItemNotFoundException] { return $null }
    catch { throw "Cannot safely inspect managed path: $Path" }
}

function Assert-OrdinaryFile {
    param(
        [Parameter(Mandatory = $true)]$Item,
        [Parameter(Mandatory = $true)][string]$Context
    )
    if ($Item -isnot [IO.FileInfo] -or
        ($Item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "$Context must be an ordinary regular file, not a directory or reparse point: $($Item.FullName)"
    }
}

function Reset-InstallerFilePair {
    param(
        [Parameter(Mandatory = $true)][string]$First,
        [Parameter(Mandatory = $true)][string]$Second,
        [Parameter(Mandatory = $true)][string]$Context
    )
    $FirstItem = Get-ManagedPathItem $First
    $SecondItem = Get-ManagedPathItem $Second
    if (($null -eq $FirstItem) -ne ($null -eq $SecondItem)) {
        throw "$Context paths must both exist or both be absent; refusing one-sided cleanup."
    }
    if ($null -eq $FirstItem) { return }
    Assert-OrdinaryFile $FirstItem $Context
    Assert-OrdinaryFile $SecondItem $Context
    Remove-Item -LiteralPath $First -Force
    Remove-Item -LiteralPath $Second -Force
}

function Read-ManagedInstall {
    param(
        [Parameter(Mandatory = $true)][string]$Binary,
        [Parameter(Mandatory = $true)][string]$Provenance,
        [Parameter(Mandatory = $true)][string]$ExpectedRepository,
        [Parameter(Mandatory = $true)][string]$ExpectedTarget
    )
    try { $Record = Get-Content -LiteralPath $Provenance -Raw | ConvertFrom-Json }
    catch { throw "Managed destination has unreadable provenance: $Provenance" }
    $ExpectedFields = @(
        'schema_version', 'repository', 'tag', 'version', 'asset', 'sha256', 'installed_at'
    )
    $ActualFields = @($Record.PSObject.Properties.Name)
    if (@(Compare-Object ($ExpectedFields | Sort-Object) ($ActualFields | Sort-Object)).Count) {
        throw "Managed provenance fields do not exactly match schema 1: $Provenance"
    }
    if (($Record.schema_version -isnot [int] -and $Record.schema_version -isnot [long]) -or
        $Record.schema_version -ne 1) {
        throw "Managed provenance schema_version is invalid: $Provenance"
    }
    foreach ($Field in $ExpectedFields | Where-Object { $_ -ne 'schema_version' }) {
        if ($Record.$Field -isnot [string] -or [string]::IsNullOrWhiteSpace($Record.$Field)) {
            throw "Managed provenance field '$Field' must be a nonblank string."
        }
    }
    if ($Record.repository -cne $ExpectedRepository) {
        throw "Managed provenance repository does not match $ExpectedRepository."
    }
    $Version = ConvertTo-SemVer $Record.version
    if ($Record.tag -cne "v$($Version.Text)") {
        throw 'Managed provenance tag and version are inconsistent.'
    }
    $ExpectedAsset = "skill-manager-v$($Version.Text)-$ExpectedTarget.zip"
    if ($Record.asset -cne $ExpectedAsset) {
        throw "Managed provenance asset does not match this platform: $ExpectedAsset"
    }
    if ($Record.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Managed provenance sha256 must be exactly 64 lowercase hexadecimal characters.'
    }
    if ($Record.installed_at -cnotmatch (
        '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}' +
        '(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})$'
    )) {
        throw 'Managed provenance installed_at must be an RFC 3339 timestamp with an offset.'
    }
    $ParsedTimestamp = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse(
        $Record.installed_at,
        [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::RoundtripKind,
        [ref]$ParsedTimestamp
    )) {
        throw 'Managed provenance installed_at must be a valid round-trip timestamp.'
    }
    $VersionOutput = (& $Binary --version 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $VersionOutput -cne "skill-manager $($Version.Text)") {
        throw "Managed binary version does not match provenance: $VersionOutput"
    }
    [pscustomobject]@{ Record = $Record; Version = $Version }
}

function Assert-ExactObjectFields {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string[]]$Expected,
        [Parameter(Mandatory = $true)][string]$Context
    )
    if ($Value -isnot [pscustomobject]) {
        throw "$Context must be a JSON object."
    }
    $Actual = @($Value.PSObject.Properties.Name)
    if (@(Compare-Object ($Expected | Sort-Object) ($Actual | Sort-Object)).Count) {
        throw "$Context fields do not exactly match the expected schema."
    }
}

function Assert-SourceIdentityFields {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string]$Context
    )
    foreach ($Field in @('source', 'source_id', 'source_name', 'source_label', 'source_type', 'mode')) {
        if ($Value.$Field -isnot [string]) {
            throw "$Context field '$Field' must be a string."
        }
    }
    if ($Value.source_id -cnotmatch '^src_[0-9a-f]+$') {
        throw "$Context source_id is invalid."
    }
    if ($null -ne $Value.alternate) {
        Assert-ExactObjectFields $Value.alternate @('source', 'source_type') "$Context alternate"
        if ($Value.alternate.source -isnot [string] -or
            $Value.alternate.source_type -isnot [string]) {
            throw "$Context alternate fields must be strings."
        }
    }
}

function Assert-SkillActionData {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string]$Context
    )
    Assert-ExactObjectFields $Value @(
        'source', 'source_id', 'source_name', 'source_label', 'source_type',
        'mode', 'alternate', 'skill', 'path', 'target', 'scope',
        'target_path', 'destination', 'action', 'dry_run'
    ) $Context
    Assert-SourceIdentityFields $Value $Context
    foreach ($Field in @('skill', 'path', 'target', 'scope', 'target_path', 'destination', 'action')) {
        if ($Value.$Field -isnot [string] -or [string]::IsNullOrWhiteSpace($Value.$Field)) {
            throw "$Context field '$Field' must be a nonblank string."
        }
    }
    if ($Value.dry_run -isnot [bool]) {
        throw "$Context dry_run must be boolean."
    }
}

function Invoke-Recipe {
    param(
        [Parameter(Mandatory = $true)][hashtable]$Recipe,
        [switch]$AllowFailure
    )
    $Json = $Recipe | ConvertTo-Json -Depth 20 -Compress
    $Lines = @($Json | & $ManagedBinary --json-input)
    $ExitCode = $LASTEXITCODE
    $Events = @()
    foreach ($Line in $Lines) {
        if ([string]::IsNullOrWhiteSpace($Line)) { continue }
        try { $Event = $Line | ConvertFrom-Json }
        catch { throw "skill-manager emitted a non-NDJSON stdout line: $Line" }
        Assert-ExactObjectFields $Event @('version', 'event', 'level', 'data') 'NDJSON envelope'
        if (($Event.version -isnot [int] -and $Event.version -isnot [long]) -or
            $Event.version -ne 1) {
            throw 'skill-manager emitted an unsupported NDJSON version.'
        }
        if ($Event.event -isnot [string] -or [string]::IsNullOrWhiteSpace($Event.event)) {
            throw 'skill-manager emitted an invalid NDJSON event name.'
        }
        if ($Event.level -isnot [string] -or
            $Event.level -cnotin @('info', 'warning', 'error')) {
            throw 'skill-manager emitted an invalid NDJSON level.'
        }
        if ($Event.data -isnot [pscustomobject]) {
            throw 'skill-manager emitted non-object NDJSON data.'
        }
        $Events += $Event
    }
    if ($Events.Count -eq 0) {
        throw 'skill-manager emitted no NDJSON events.'
    }
    if ($ExitCode -ne 0 -and -not $AllowFailure) {
        $Failure = @($Events | Where-Object event -eq 'command.failed')
        $Message = if ($Failure.Count -eq 1 -and $Failure[0].data.message -is [string]) {
            $Failure[0].data.message
        } else {
            "exit $ExitCode without exactly one structured command.failed event"
        }
        throw "skill-manager recipe failed: $Message"
    }
    [pscustomobject]@{ ExitCode = $ExitCode; Events = $Events; Lines = $Lines }
}

$Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$Target = switch ($Architecture) {
    'X64' { 'x86_64-pc-windows-msvc' }
    'Arm64' { 'aarch64-pc-windows-msvc' }
    default { throw "Unsupported Windows architecture: $Architecture" }
}

$MachinePath = [Environment]::GetEnvironmentVariable('PATH', 'Machine')
foreach ($MachineEntry in @($MachinePath -split ';')) {
    $NormalizedMachineEntry = Get-NormalizedPathEntry $MachineEntry
    if (-not $NormalizedMachineEntry -or
        $NormalizedMachineEntry.Equals(
            (Get-NormalizedPathEntry $ManagedDirectory),
            [StringComparison]::OrdinalIgnoreCase
        )) {
        continue
    }
    $MachineCandidate = Join-Path $NormalizedMachineEntry 'skill-manager.exe'
    if (Test-Path -LiteralPath $MachineCandidate) {
        throw "A system-level skill-manager precedes User PATH: $MachineCandidate. Remove or upgrade that system install, or rerun with an administrator-approved system-level PATH/install change. This installer will not mutate Machine PATH."
    }
}

$Release = Invoke-RestMethod -Headers @{ 'User-Agent' = 'skill-manager-installer' } `
    -Uri "https://api.github.com/repos/$Repository/releases/latest"
if ($Release.draft -or $Release.prerelease) { throw 'GitHub latest release was not stable.' }
$Latest = ConvertTo-SemVer ([string]$Release.tag_name)
$AssetName = "skill-manager-v$($Latest.Text)-$Target.zip"
$AssetMatches = @($Release.assets | Where-Object name -ceq $AssetName)
$ChecksumMatches = @($Release.assets | Where-Object name -ceq 'SHA256SUMS')
if ($AssetMatches.Count -ne 1) { throw "Expected exactly one release asset named $AssetName." }
if ($ChecksumMatches.Count -ne 1) { throw 'Expected exactly one SHA256SUMS release asset.' }

$ManagedBinaryItem = Get-ManagedPathItem $ManagedBinary
$ProvenanceItem = Get-ManagedPathItem $ProvenancePath
$ManagedBinaryExists = $null -ne $ManagedBinaryItem
$ProvenanceExists = $null -ne $ProvenanceItem
if ($ManagedBinaryExists -ne $ProvenanceExists) {
    throw 'Managed binary and provenance must either both exist or both be absent; refusing repair-by-guess.'
}
$InstallRequired = $true
if ($ManagedBinaryExists) {
    Assert-OrdinaryFile $ManagedBinaryItem 'Managed binary'
    Assert-OrdinaryFile $ProvenanceItem 'Managed provenance'
    $ManagedInstall = Read-ManagedInstall `
        -Binary $ManagedBinary `
        -Provenance $ProvenancePath `
        -ExpectedRepository $Repository `
        -ExpectedTarget $Target
    $Existing = $ManagedInstall.Version
    $Comparison = Compare-SemVer $Existing $Latest
    if ($Comparison -ge 0) {
        $InstallRequired = $false
        Write-Host "Reusing managed skill-manager $($Existing.Text); latest is $($Latest.Text)."
    }
}

New-Item -ItemType Directory -Force -Path $ManagedDirectory | Out-Null
if ($InstallRequired) {
    $StageRoot = Join-Path ([IO.Path]::GetTempPath()) ("skill-manager-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $StageRoot | Out-Null
    try {
        $Archive = Join-Path $StageRoot $AssetName
        $Sums = Join-Path $StageRoot 'SHA256SUMS'
        Invoke-WebRequest -UseBasicParsing -Uri $AssetMatches[0].browser_download_url -OutFile $Archive
        Invoke-WebRequest -UseBasicParsing -Uri $ChecksumMatches[0].browser_download_url -OutFile $Sums
        $EscapedAsset = [regex]::Escape($AssetName)
        $ChecksumLines = @(Get-Content -LiteralPath $Sums | Where-Object {
            $_ -match "^(?<hash>[0-9a-fA-F]{64})  $EscapedAsset$"
        })
        if ($ChecksumLines.Count -ne 1) {
            throw "SHA256SUMS must contain exactly one checksum for $AssetName."
        }
        $null = $ChecksumLines[0] -match '^(?<hash>[0-9a-fA-F]{64})  '
        $ExpectedHash = $Matches.hash.ToLowerInvariant()
        $ActualHash = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($ActualHash -cne $ExpectedHash) { throw "Checksum mismatch for $AssetName." }

        $Expanded = Join-Path $StageRoot 'expanded'
        Expand-Archive -LiteralPath $Archive -DestinationPath $Expanded
        $Candidates = @(Get-ChildItem -LiteralPath $Expanded -Recurse -File -Filter 'skill-manager.exe')
        if ($Candidates.Count -ne 1) { throw 'Release archive must contain exactly one skill-manager.exe.' }
        $CandidateBinary = Join-Path $StageRoot 'candidate-skill-manager.exe'
        Copy-Item -LiteralPath $Candidates[0].FullName -Destination $CandidateBinary
        $StagedVersionOutput = (& $CandidateBinary --version 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $StagedVersionOutput -notmatch "(^| )$([regex]::Escape($Latest.Text))$") {
            throw "Staged binary failed version verification: $StagedVersionOutput"
        }

        $NewProvenance = [ordered]@{
            schema_version = 1
            repository = $Repository
            tag = [string]$Release.tag_name
            version = $Latest.Text
            asset = $AssetName
            sha256 = $ActualHash
            installed_at = [DateTimeOffset]::UtcNow.ToString('o')
        }
        $CandidateProvenance = Join-Path $StageRoot 'candidate-provenance.json'
        $NewProvenance | ConvertTo-Json | Set-Content -LiteralPath $CandidateProvenance -Encoding UTF8
        $null = Read-ManagedInstall -Binary $CandidateBinary -Provenance $CandidateProvenance `
            -ExpectedRepository $Repository -ExpectedTarget $Target

        $StagedBinary = Join-Path $ManagedDirectory 'skill-manager.exe.new'
        $StagedProvenance = Join-Path $ManagedDirectory 'install-provenance.json.new'
        Reset-InstallerFilePair $StagedBinary $StagedProvenance 'Installer staged pair'
        $NewBinaryCreated = $false
        $NewProvenanceCreated = $false
        try {
            Copy-Item -LiteralPath $CandidateBinary -Destination $StagedBinary
            $NewBinaryCreated = $true
            Copy-Item -LiteralPath $CandidateProvenance -Destination $StagedProvenance
            $NewProvenanceCreated = $true
            Assert-OrdinaryFile (Get-ManagedPathItem $StagedBinary) 'Installer staged binary'
            Assert-OrdinaryFile (Get-ManagedPathItem $StagedProvenance) 'Installer staged provenance'
            $null = Read-ManagedInstall -Binary $StagedBinary -Provenance $StagedProvenance `
                -ExpectedRepository $Repository -ExpectedTarget $Target
        } catch {
            if ($NewBinaryCreated -and (Test-Path -LiteralPath $StagedBinary)) {
                Remove-Item -LiteralPath $StagedBinary -Force
            }
            if ($NewProvenanceCreated -and (Test-Path -LiteralPath $StagedProvenance)) {
                Remove-Item -LiteralPath $StagedProvenance -Force
            }
            throw
        }

        $RollbackBinary = Join-Path $ManagedDirectory 'skill-manager.exe.rollback'
        $RollbackProvenance = Join-Path $ManagedDirectory 'install-provenance.json.rollback'
        Reset-InstallerFilePair $RollbackBinary $RollbackProvenance 'Installer rollback pair'
        $OldBinaryMoved = $false
        $OldProvenanceMoved = $false
        $NewBinaryInstalled = $false
        $NewProvenanceInstalled = $false
        try {
            if ($ManagedBinaryExists) {
                Move-Item -LiteralPath $ManagedBinary -Destination $RollbackBinary
                $OldBinaryMoved = $true
                Move-Item -LiteralPath $ProvenancePath -Destination $RollbackProvenance
                $OldProvenanceMoved = $true
            }
            Move-Item -LiteralPath $StagedBinary -Destination $ManagedBinary
            $NewBinaryInstalled = $true
            Move-Item -LiteralPath $StagedProvenance -Destination $ProvenancePath
            $NewProvenanceInstalled = $true
            $null = Read-ManagedInstall `
                -Binary $ManagedBinary `
                -Provenance $ProvenancePath `
                -ExpectedRepository $Repository `
                -ExpectedTarget $Target
        } catch {
            $OriginalFailure = $_
            $RecoveryErrors = @()
            if ($NewBinaryInstalled) {
                try { Remove-Item -LiteralPath $ManagedBinary -Force }
                catch { $RecoveryErrors += "remove new binary: $($_.Exception.Message)" }
            }
            if ($NewProvenanceInstalled) {
                try { Remove-Item -LiteralPath $ProvenancePath -Force }
                catch { $RecoveryErrors += "remove new provenance: $($_.Exception.Message)" }
            }
            if ($OldBinaryMoved) {
                try {
                    Move-Item -LiteralPath $RollbackBinary -Destination $ManagedBinary
                    $OldBinaryMoved = $false
                } catch { $RecoveryErrors += "restore old binary: $($_.Exception.Message)" }
            }
            if ($OldProvenanceMoved) {
                try {
                    Move-Item -LiteralPath $RollbackProvenance -Destination $ProvenancePath
                    $OldProvenanceMoved = $false
                } catch { $RecoveryErrors += "restore old provenance: $($_.Exception.Message)" }
            }
            if ($ManagedBinaryExists -and $RecoveryErrors.Count -eq 0) {
                try {
                    $null = Read-ManagedInstall -Binary $ManagedBinary -Provenance $ProvenancePath `
                        -ExpectedRepository $Repository -ExpectedTarget $Target
                } catch { $RecoveryErrors += "validate restored pair: $($_.Exception.Message)" }
            }
            if ($RecoveryErrors.Count -gt 0) {
                throw "FATAL: replacement failed and restoration failed: $($RecoveryErrors -join '; ')"
            }
            throw $OriginalFailure
        } finally {
            if (-not $NewBinaryInstalled -and $NewBinaryCreated -and
                (Test-Path -LiteralPath $StagedBinary)) {
                Remove-Item -LiteralPath $StagedBinary -Force
            }
            if (-not $NewProvenanceInstalled -and $NewProvenanceCreated -and
                (Test-Path -LiteralPath $StagedProvenance)) {
                Remove-Item -LiteralPath $StagedProvenance -Force
            }
        }
        Reset-InstallerFilePair $RollbackBinary $RollbackProvenance 'Completed rollback pair'
    } finally {
        if (Test-Path -LiteralPath $StageRoot) {
            Remove-Item -LiteralPath $StageRoot -Recurse -Force
        }
    }
}

$ManagedPathKey = Get-NormalizedPathEntry $ManagedDirectory
$PathParts = @($env:PATH -split ';' | Where-Object {
    $EntryKey = Get-NormalizedPathEntry $_
    $EntryKey -and -not $EntryKey.Equals($ManagedPathKey, [StringComparison]::OrdinalIgnoreCase)
})
$env:PATH = (@($ManagedDirectory) + $PathParts) -join ';'
$UserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
$UserParts = @($UserPath -split ';' | Where-Object {
    $EntryKey = Get-NormalizedPathEntry $_
    $EntryKey -and -not $EntryKey.Equals($ManagedPathKey, [StringComparison]::OrdinalIgnoreCase)
})
[Environment]::SetEnvironmentVariable(
    'PATH',
    ((@($ManagedDirectory) + $UserParts) -join ';'),
    'User'
)
if ($PreviousPath -and
    -not [IO.Path]::GetFullPath($PreviousPath).Equals(
        [IO.Path]::GetFullPath($ManagedBinary),
        [StringComparison]::OrdinalIgnoreCase
    )) {
    Write-Warning "The validated managed installation now shadows the external PATH executable: $PreviousPath"
}

$SourceList = Invoke-Recipe @{ command = 'source.list' }
$SourceEvents = @($SourceList.Events | Where-Object event -eq 'source.listed')
foreach ($SourceEvent in $SourceEvents) {
    Assert-ExactObjectFields $SourceEvent.data @(
        'source', 'source_id', 'source_name', 'source_label',
        'source_type', 'mode', 'alternate'
    ) 'source.listed data'
    Assert-SourceIdentityFields $SourceEvent.data 'source.listed data'
}
$ExactSources = @($SourceEvents | Where-Object { $_.data.source -ceq $SourceIdentity })
$AlternateSources = @($SourceEvents | Where-Object {
    $null -ne $_.data.alternate -and $_.data.alternate.source -ceq $SourceIdentity
})
$NamedSources = @($SourceEvents | Where-Object { $_.data.source_name -ceq $RequestedSourceName })
if (($ExactSources.Count + $AlternateSources.Count) -gt 1) {
    throw "Multiple active/alternate source slots match $SourceIdentity."
}
if ($ExactSources.Count -eq 1) {
    $SourceSelector = [string]$ExactSources[0].data.source_id
    $SourceName = [string]$ExactSources[0].data.source_name
} elseif ($AlternateSources.Count -eq 1) {
    $SourceSelector = [string]$AlternateSources[0].data.source_id
    $Swapped = Invoke-Recipe @{ command = 'source.swap'; source = $SourceSelector }
    $SwapEvents = @($Swapped.Events | Where-Object event -eq 'source.locations-swapped')
    if ($SwapEvents.Count -ne 1) { throw 'source.swap must emit exactly one source.locations-swapped.' }
    Assert-ExactObjectFields $SwapEvents[0].data @(
        'source', 'source_id', 'source_name', 'source_label', 'source_type',
        'mode', 'alternate', 'changed', 'previous'
    ) 'source.locations-swapped data'
    Assert-SourceIdentityFields $SwapEvents[0].data 'source.locations-swapped data'
    Assert-ExactObjectFields $SwapEvents[0].data.previous @(
        'source', 'source_type', 'alternate'
    ) 'source.locations-swapped previous'
    if ($SwapEvents[0].data.source -cne $SourceIdentity -or
        $SwapEvents[0].data.changed -ne $true) {
        throw 'source.swap did not activate the requested remote.'
    }
    $Confirmed = Invoke-Recipe @{ command = 'source.list' }
    $ConfirmedActive = @($Confirmed.Events | Where-Object {
        $_.event -eq 'source.listed' -and
        $_.data.source_id -ceq $SourceSelector -and
        $_.data.source -ceq $SourceIdentity
    })
    if ($ConfirmedActive.Count -ne 1) { throw 'source.list did not confirm the swapped active remote.' }
    $SourceName = [string]$ConfirmedActive[0].data.source_name
} else {
    if ($NamedSources.Count -gt 0) {
        throw "Source name $RequestedSourceName already identifies a different active source."
    }
    $Added = Invoke-Recipe @{
        command = 'source.add'
        source = $SourceIdentity
        name = $RequestedSourceName
        label = 'sernst skills'
    }
    $AddedEvent = @($Added.Events | Where-Object event -eq 'source.added')
    if ($AddedEvent.Count -ne 1) { throw 'source.add must emit exactly one source.added event.' }
    Assert-ExactObjectFields $AddedEvent[0].data @(
        'source', 'source_id', 'source_name', 'source_label',
        'source_type', 'mode', 'alternate'
    ) 'source.added data'
    Assert-SourceIdentityFields $AddedEvent[0].data 'source.added data'
    if ($AddedEvent[0].data.source -cne $SourceIdentity -or
        $AddedEvent[0].data.source_name -cne $RequestedSourceName -or
        $AddedEvent[0].data.source_id -isnot [string] -or
        $AddedEvent[0].data.source_id -cnotmatch '^src_[0-9a-f]+$') {
        throw 'source.added does not match the requested source identity.'
    }
    $SourceName = [string]$AddedEvent[0].data.source_name
    $SourceSelector = [string]$AddedEvent[0].data.source_id
}

$ExpectedTargets = @('shared')
if ($Harness -eq 'claude') { $ExpectedTargets += 'claude' }
if ($Harness -eq 'antigravity') { $ExpectedTargets += 'antigravity' }
$ExpectedTargets = @($ExpectedTargets | Sort-Object -Unique)
$BaseLoad = @{
    command = 'load'
    sources = @($SourceSelector)
    filters = @('managing-skills')
    targets = $ExpectedTargets
    global = $true
}

$DryRecipe = @{} + $BaseLoad
$DryRecipe.dry_run = $true
$DryRun = Invoke-Recipe $DryRecipe
$DryActions = @($DryRun.Events | Where-Object {
    $_.event -in @('skill.loaded', 'skill.updated', 'skill.skipped', 'skill.copied', 'skill.removed')
})
if ($DryActions.Count -ne $ExpectedTargets.Count) {
    throw 'Dry-run action-event set does not equal the expected skill/target tuples.'
}
foreach ($TargetName in $ExpectedTargets) {
    $Planned = @($DryActions | Where-Object {
        $_.event -in @('skill.loaded', 'skill.skipped') -and
        $_.data.skill -ceq 'managing-skills' -and
        $_.data.target -ceq $TargetName -and
        $_.data.scope -ceq 'global' -and
        $_.data.dry_run -eq $true
    })
    if ($Planned.Count -ne 1) {
        throw "Dry-run must emit exactly one correlated action for $TargetName."
    }
    Assert-SkillActionData $Planned[0].data "dry-run action data for $TargetName"
}

$Loaded = Invoke-Recipe $BaseLoad
$CommittedActions = @($Loaded.Events | Where-Object {
    $_.event -in @('skill.loaded', 'skill.updated', 'skill.skipped', 'skill.copied', 'skill.removed')
})
if ($CommittedActions.Count -ne $ExpectedTargets.Count) {
    throw 'Committed action-event set does not equal the expected skill/target tuples.'
}
foreach ($TargetName in $ExpectedTargets) {
    $Committed = @($CommittedActions | Where-Object {
        $_.event -in @('skill.loaded', 'skill.skipped') -and
        $_.data.skill -ceq 'managing-skills' -and
        $_.data.target -ceq $TargetName -and
        $_.data.scope -ceq 'global' -and
        $_.data.dry_run -eq $false
    })
    if ($Committed.Count -ne 1) {
        throw "Load must emit exactly one correlated committed action for $TargetName."
    }
    Assert-SkillActionData $Committed[0].data "committed action data for $TargetName"
}

$StatusRecipe = @{
    command = 'status'
    filters = @('managing-skills')
    targets = $ExpectedTargets
    global = $true
}
$Status = Invoke-Recipe $StatusRecipe
$Row = @($Status.Events | Where-Object {
    $_.event -eq 'status.row' -and $_.data.skill -ceq 'managing-skills'
})
if ($Row.Count -ne 1) { throw 'Status must return exactly one managing-skills row.' }
Assert-ExactObjectFields $Row[0].data @(
    'skill', 'source', 'targets', 'location', 'mixed',
    'shadowed_global_divergent', 'deployments'
) 'status.row data'
if ($Row[0].data.skill -isnot [string] -or $Row[0].data.location -isnot [string] -or
    $Row[0].data.mixed -isnot [bool] -or
    $Row[0].data.shadowed_global_divergent -isnot [bool] -or
    $Row[0].data.targets -isnot [pscustomobject]) {
    throw 'status.row aggregate fields have invalid types.'
}
if ($null -ne $Row[0].data.source) {
    Assert-ExactObjectFields $Row[0].data.source @(
        'source', 'source_id', 'source_name', 'source_label',
        'source_type', 'mode', 'alternate'
    ) 'status.row source'
    Assert-SourceIdentityFields $Row[0].data.source 'status.row source'
}
foreach ($DeploymentItem in @($Row[0].data.deployments)) {
    Assert-ExactObjectFields $DeploymentItem @(
        'target', 'scope', 'path', 'installed', 'state', 'effective'
    ) 'status deployment'
    foreach ($Field in @('target', 'scope', 'path', 'state')) {
        if ($DeploymentItem.$Field -isnot [string] -or
            [string]::IsNullOrWhiteSpace($DeploymentItem.$Field)) {
            throw "Status deployment field '$Field' must be a nonblank string."
        }
    }
    if ($DeploymentItem.installed -isnot [bool] -or
        $DeploymentItem.effective -isnot [bool]) {
        throw 'Status deployment installed/effective fields must be boolean.'
    }
}
if (@($Row[0].data.deployments).Count -ne $ExpectedTargets.Count) {
    throw 'Status deployment set does not equal the expected target/scope tuples.'
}

$SkillPaths = @()
foreach ($TargetName in $ExpectedTargets) {
    $Deployment = @($Row[0].data.deployments | Where-Object {
        $_.target -ceq $TargetName -and
        $_.scope -ceq 'global' -and
        $_.installed -eq $true -and
        $_.path -is [string] -and
        -not [string]::IsNullOrWhiteSpace($_.path)
    })
    if ($Deployment.Count -ne 1) {
        throw "Status must return exactly one installed global deployment for $TargetName."
    }
    $SkillPaths += (Join-Path ([string]$Deployment[0].path) 'SKILL.md')
}
$SkillPaths = @($SkillPaths | Sort-Object -Unique)
foreach ($SkillPath in $SkillPaths) {
    if (-not (Test-Path -LiteralPath $SkillPath)) { throw "Installed skill is missing: $SkillPath" }
    Write-Output "----- BEGIN INSTALLED SKILL: $SkillPath -----"
    Get-Content -LiteralPath $SkillPath -Raw
    Write-Output "----- END INSTALLED SKILL: $SkillPath -----"
}
Write-Host "Installed skill-manager and managing-skills from source '$SourceName' for: $($ExpectedTargets -join ', ')."
```

## macOS and Linux

Run this complete block in Bash. It requires Python 3 for strict provenance and
NDJSON parsing, plus `curl`, `tar`, and either `sha256sum` or `shasum`. It also
accepts Zsh as the user's persistent shell and writes Fish syntax when Fish is
explicitly identified. Set `SKILL_MANAGER_HARNESS` from application identity.
If `$SHELL` is unavailable or unrecognized, ask the user and set
`SKILL_MANAGER_SHELL` to `bash`, `zsh`, or `fish` before running.

```bash
set -euo pipefail

: "${SKILL_MANAGER_HARNESS:?Agent must set SKILL_MANAGER_HARNESS from explicit application identity.}"
case "$SKILL_MANAGER_HARNESS" in
  codex|claude|antigravity) ;;
  *) printf 'Harness must be codex, claude, or antigravity.\n' >&2; exit 1 ;;
esac

repository='sernst/skills'
source_identity='sernst/skills/skills'
requested_source_name='sernst-skills'
managed_directory="$HOME/.local/share/skill-manager/bin"
managed_binary="$managed_directory/skill-manager"
provenance_path="$managed_directory/install-provenance.json"
previous_path="$(command -v skill-manager 2>/dev/null || true)"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    printf 'Required command is missing: %s\n' "$1" >&2
    exit 1
  }
}
require_command curl
require_command tar
require_command uname
require_command python3

parse_semver() {
  case "$1" in
    v[0-9]*.[0-9]*.[0-9]*) value=${1#v} ;;
    [0-9]*.[0-9]*.[0-9]*) value=$1 ;;
    *) return 1 ;;
  esac
  old_ifs=$IFS
  IFS=.
  set -- $value
  IFS=$old_ifs
  [ "$#" -eq 3 ] || return 1
  for part in "$@"; do
    case "$part" in ''|*[!0-9]*) return 1 ;; esac
  done
  [ "$1" = 0 ] || [ "${1#0}" = "$1" ] || return 1
  [ "$2" = 0 ] || [ "${2#0}" = "$2" ] || return 1
  [ "$3" = 0 ] || [ "${3#0}" = "$3" ] || return 1
  printf '%s.%s.%s\n' "$1" "$2" "$3"
}

compare_semver() {
  left=$1
  right=$2
  old_ifs=$IFS
  IFS=.
  set -- $left
  l1=$1 l2=$2 l3=$3
  set -- $right
  r1=$1 r2=$2 r3=$3
  IFS=$old_ifs
  for pair in "$l1:$r1" "$l2:$r2" "$l3:$r3"; do
    l=${pair%%:*}
    r=${pair#*:}
    if [ "$l" -lt "$r" ]; then printf '%s\n' -1; return; fi
    if [ "$l" -gt "$r" ]; then printf '%s\n' 1; return; fi
  done
  printf '%s\n' 0
}

os="$(uname -s)"
arch="$(uname -m)"
case "$os:$arch" in
  Darwin:x86_64) target='x86_64-apple-darwin' ;;
  Darwin:arm64|Darwin:aarch64) target='aarch64-apple-darwin' ;;
  Linux:x86_64|Linux:amd64) cpu='x86_64' ;;
  Linux:aarch64|Linux:arm64) cpu='aarch64' ;;
  *) printf 'Unsupported OS/architecture: %s/%s\n' "$os" "$arch" >&2; exit 1 ;;
esac
if [ "$os" = Linux ]; then
  libc=gnu
  if (ldd --version 2>&1 || true) | grep -qi musl ||
     find /lib /usr/lib -maxdepth 2 -name 'ld-musl-*.so.1' -print -quit 2>/dev/null | grep -q .; then
    libc=musl
  fi
  target="${cpu}-unknown-linux-${libc}"
fi

latest_url="$(curl -fsSIL -o /dev/null -w '%{url_effective}' \
  "https://github.com/$repository/releases/latest")"
tag=${latest_url##*/}
latest="$(parse_semver "$tag")" || {
  printf 'Latest release tag is not stable SemVer: %s\n' "$tag" >&2
  exit 1
}
asset_name="skill-manager-v${latest}-${target}.tar.gz"
asset_url="https://github.com/$repository/releases/download/$tag/$asset_name"
sums_url="https://github.com/$repository/releases/download/$tag/SHA256SUMS"

validate_managed_install() {
  validate_provenance=${1:-$provenance_path}
  validate_binary=${2:-$managed_binary}
  python3 - "$validate_provenance" "$validate_binary" "$repository" "$target" <<'PY'
import datetime
import json
import pathlib
import re
import subprocess
import sys

provenance_path, binary, repository, target = sys.argv[1:]
required = {
    "schema_version", "repository", "tag", "version",
    "asset", "sha256", "installed_at",
}
try:
    record = json.loads(pathlib.Path(provenance_path).read_text(encoding="utf-8"))
except Exception as error:
    raise SystemExit(f"managed provenance is not valid UTF-8 JSON: {error}")
if not isinstance(record, dict) or set(record) != required:
    raise SystemExit("managed provenance fields do not exactly match schema 1")
if type(record["schema_version"]) is not int or record["schema_version"] != 1:
    raise SystemExit("managed provenance schema_version must be integer 1")
for field in required - {"schema_version"}:
    if not isinstance(record[field], str) or not record[field]:
        raise SystemExit(f"managed provenance {field} must be a nonblank string")
if record["repository"] != repository:
    raise SystemExit("managed provenance repository mismatch")
version_pattern = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
if re.fullmatch(version_pattern, record["version"]) is None:
    raise SystemExit("managed provenance version is not stable SemVer")
if record["tag"] != f'v{record["version"]}':
    raise SystemExit("managed provenance tag/version mismatch")
expected_asset = f'skill-manager-v{record["version"]}-{target}.tar.gz'
if record["asset"] != expected_asset:
    raise SystemExit(f"managed provenance asset mismatch: expected {expected_asset}")
if re.fullmatch(r"[0-9a-f]{64}", record["sha256"]) is None:
    raise SystemExit("managed provenance sha256 must be lowercase 64-digit hex")
if re.fullmatch(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
    r"(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})",
    record["installed_at"],
) is None:
    raise SystemExit("managed provenance installed_at must be RFC 3339 with an offset")
try:
    installed_at = datetime.datetime.fromisoformat(record["installed_at"].replace("Z", "+00:00"))
except ValueError as error:
    raise SystemExit(f"managed provenance installed_at is invalid: {error}")
if installed_at.utcoffset() is None:
    raise SystemExit("managed provenance installed_at must include an offset")
try:
    output = subprocess.run(
        [binary, "--version"], check=True, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    ).stdout.strip()
except Exception as error:
    raise SystemExit(f"managed binary failed version verification: {error}")
if output != f'skill-manager {record["version"]}':
    raise SystemExit("managed binary version does not match provenance")
print(record["version"])
PY
}

path_present() {
  [ -e "$1" ] || [ -L "$1" ]
}

assert_ordinary_file() {
  [ -f "$1" ] && [ ! -L "$1" ] || {
    printf 'Managed path must be an ordinary regular file, not a directory or link: %s\n' "$1" >&2
    return 1
  }
}

reset_file_pair() {
  first=$1
  second=$2
  context=$3
  first_present=false
  second_present=false
  path_present "$first" && first_present=true
  path_present "$second" && second_present=true
  if [ "$first_present" != "$second_present" ]; then
    printf '%s paths must both exist or both be absent; refusing one-sided cleanup.\n' "$context" >&2
    exit 1
  fi
  if [ "$first_present" = true ]; then
    assert_ordinary_file "$first"
    assert_ordinary_file "$second"
    rm -f "$first" "$second"
  fi
}

install_required=true
binary_present=false
provenance_present=false
path_present "$managed_binary" && binary_present=true
path_present "$provenance_path" && provenance_present=true
if [ "$binary_present" != "$provenance_present" ]; then
  printf 'Managed binary and provenance must both exist or both be absent; refusing repair-by-guess.\n' >&2
  exit 1
fi
if [ "$binary_present" = true ]; then
  assert_ordinary_file "$managed_binary"
  assert_ordinary_file "$provenance_path"
  existing="$(validate_managed_install)"
  comparison="$(compare_semver "$existing" "$latest")"
  if [ "$comparison" -ge 0 ]; then
    install_required=false
    printf 'Reusing managed skill-manager %s; latest is %s.\n' "$existing" "$latest"
  fi
fi

mkdir -p "$managed_directory"
if [ "$install_required" = true ]; then
  stage_root="$(mktemp -d "${TMPDIR:-/tmp}/skill-manager-install.XXXXXXXX")"
  cleanup_stage() { rm -rf "$stage_root"; }
  trap cleanup_stage EXIT HUP INT TERM
  archive="$stage_root/$asset_name"
  sums="$stage_root/SHA256SUMS"
  curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$archive" "$asset_url"
  curl -fL --retry 3 --proto '=https' --tlsv1.2 -o "$sums" "$sums_url"
  checksum_lines="$(awk -v asset="$asset_name" \
    'NF == 2 && $2 == asset && length($1) == 64 && $1 ~ /^[0-9a-fA-F]+$/ { print }' \
    "$sums")"
  checksum_count="$(printf '%s\n' "$checksum_lines" | grep -c . || true)"
  [ "$checksum_count" -eq 1 ] || {
    printf 'SHA256SUMS must contain exactly one checksum for %s.\n' "$asset_name" >&2
    exit 1
  }
  expected_hash=${checksum_lines%% *}
  if command -v sha256sum >/dev/null 2>&1; then
    actual_hash="$(sha256sum "$archive" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual_hash="$(shasum -a 256 "$archive" | awk '{print $1}')"
  else
    printf 'Need sha256sum or shasum for checksum verification.\n' >&2
    exit 1
  fi
  [ "$(printf '%s' "$actual_hash" | tr A-F a-f)" =
    "$(printf '%s' "$expected_hash" | tr A-F a-f)" ] || {
      printf 'Checksum mismatch for %s.\n' "$asset_name" >&2
      exit 1
    }

  expanded="$stage_root/expanded"
  mkdir "$expanded"
  tar -xzf "$archive" -C "$expanded"
  candidates="$(find "$expanded" -type f -name skill-manager -print)"
  candidate_count="$(printf '%s\n' "$candidates" | grep -c . || true)"
  [ "$candidate_count" -eq 1 ] || {
    printf 'Release archive must contain exactly one skill-manager.\n' >&2
    exit 1
  }
  candidate_binary="$stage_root/candidate-skill-manager"
  candidate_provenance="$stage_root/candidate-provenance.json"
  cp "$candidates" "$candidate_binary"
  chmod 0755 "$candidate_binary"
  staged_output="$("$candidate_binary" --version 2>&1)" || {
    printf 'Staged binary failed: %s\n' "$staged_output" >&2
    exit 1
  }
  [ "${staged_output##* }" = "$latest" ] || {
    printf 'Staged binary version mismatch: %s\n' "$staged_output" >&2
    exit 1
  }
  actual_hash="$(printf '%s' "$actual_hash" | tr A-F a-f)"
  python3 - "$candidate_provenance" "$repository" "$tag" "$latest" \
    "$asset_name" "$actual_hash" <<'PY'
import datetime
import json
import pathlib
import sys

path, repository, tag, version, asset, sha256 = sys.argv[1:]
record = {
    "schema_version": 1,
    "repository": repository,
    "tag": tag,
    "version": version,
    "asset": asset,
    "sha256": sha256,
    "installed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
}
pathlib.Path(path).write_text(
    json.dumps(record, separators=(",", ":")) + "\n", encoding="utf-8"
)
PY
  candidate_version="$(validate_managed_install "$candidate_provenance" "$candidate_binary")"
  [ "$candidate_version" = "$latest" ] || {
    printf 'Candidate pair failed validation.\n' >&2
    exit 1
  }

  staged_binary="$managed_directory/skill-manager.new"
  staged_provenance="$managed_directory/install-provenance.json.new"
  reset_file_pair "$staged_binary" "$staged_provenance" 'Installer staged pair'
  new_binary_created=false
  new_provenance_created=false
  if cp "$candidate_binary" "$staged_binary"; then
    new_binary_created=true
  else
    printf 'Failed to stage managed binary.\n' >&2
    exit 1
  fi
  if cp "$candidate_provenance" "$staged_provenance"; then
    new_provenance_created=true
  else
    [ "$new_binary_created" = false ] || rm -f "$staged_binary"
    printf 'Failed to stage managed provenance; staged residue removed.\n' >&2
    exit 1
  fi
  if ! assert_ordinary_file "$staged_binary" ||
     ! assert_ordinary_file "$staged_provenance" ||
     ! staged_version="$(validate_managed_install "$staged_provenance" "$staged_binary")" ||
     [ "$staged_version" != "$latest" ]; then
    rm -f "$staged_binary" "$staged_provenance"
    printf 'Managed .new pair failed validation; staged residue removed.\n' >&2
    exit 1
  fi

  rollback_binary="$managed_directory/skill-manager.rollback"
  rollback_provenance="$managed_directory/install-provenance.json.rollback"
  reset_file_pair "$rollback_binary" "$rollback_provenance" 'Installer rollback pair'
  old_binary_moved=false
  old_provenance_moved=false
  new_binary_installed=false
  new_provenance_installed=false
  transaction_failure=''
  set +e
  if [ "$binary_present" = true ]; then
    mv "$managed_binary" "$rollback_binary"
    [ "$?" -eq 0 ] && old_binary_moved=true || transaction_failure='old binary move failed'
    if [ -z "$transaction_failure" ]; then
      mv "$provenance_path" "$rollback_provenance"
      [ "$?" -eq 0 ] && old_provenance_moved=true || transaction_failure='old provenance move failed'
    fi
  fi
  if [ -z "$transaction_failure" ]; then
    mv "$staged_binary" "$managed_binary"
    [ "$?" -eq 0 ] && new_binary_installed=true || transaction_failure='new binary install failed'
  fi
  if [ -z "$transaction_failure" ]; then
    mv "$staged_provenance" "$provenance_path"
    [ "$?" -eq 0 ] && new_provenance_installed=true || transaction_failure='new provenance install failed'
  fi
  if [ -z "$transaction_failure" ]; then
    installed_version="$(validate_managed_install)"
    [ "$?" -eq 0 ] && [ "$installed_version" = "$latest" ] ||
      transaction_failure='installed pair validation failed'
  fi
  set -e
  if [ -n "$transaction_failure" ]; then
    recovery_failure=''
    set +e
    if [ "$new_binary_installed" = true ]; then rm -f "$managed_binary" || recovery_failure='remove new binary'; fi
    if [ "$new_provenance_installed" = true ]; then rm -f "$provenance_path" || recovery_failure='remove new provenance'; fi
    if [ "$old_binary_moved" = true ]; then mv "$rollback_binary" "$managed_binary" || recovery_failure='restore old binary'; fi
    if [ "$old_provenance_moved" = true ]; then mv "$rollback_provenance" "$provenance_path" || recovery_failure='restore old provenance'; fi
    if [ "$binary_present" = true ] && [ -z "$recovery_failure" ]; then
      validate_managed_install >/dev/null || recovery_failure='validate restored pair'
    fi
    rm -f "$staged_binary" "$staged_provenance"
    set -e
    if [ -n "$recovery_failure" ]; then
      printf 'FATAL: replacement failed (%s) and recovery failed (%s).\n' \
        "$transaction_failure" "$recovery_failure" >&2
      exit 1
    fi
    printf 'Replacement failed (%s); previous live pair restored.\n' "$transaction_failure" >&2
    exit 1
  fi
  reset_file_pair "$rollback_binary" "$rollback_provenance" 'Completed rollback pair'
  cleanup_stage
  trap - EXIT HUP INT TERM
fi

export PATH="$(python3 - "$managed_directory" "$PATH" <<'PY'
import os
import sys

managed, current = sys.argv[1:]
key = os.path.normcase(os.path.abspath(os.path.expandvars(os.path.expanduser(managed))))
kept = []
for entry in current.split(os.pathsep):
    if not entry:
        continue
    candidate = os.path.normcase(os.path.abspath(os.path.expandvars(os.path.expanduser(entry))))
    if candidate != key:
        kept.append(entry)
print(os.pathsep.join([managed, *kept]))
PY
)"
detected_shell=${SHELL:-}
shell_name="${SKILL_MANAGER_SHELL:-${detected_shell##*/}}"
case "$shell_name" in
  bash)
    profile="$HOME/.bashrc"
    path_line='export PATH="$HOME/.local/share/skill-manager/bin:$PATH"'
    ;;
  zsh)
    profile="$HOME/.zshrc"
    path_line='export PATH="$HOME/.local/share/skill-manager/bin:$PATH"'
    ;;
  fish)
    profile="$HOME/.config/fish/config.fish"
    path_line='fish_add_path --prepend "$HOME/.local/share/skill-manager/bin"'
    mkdir -p "$(dirname "$profile")"
    ;;
  *)
    printf 'Unknown shell %s; ask the user and set SKILL_MANAGER_SHELL to bash, zsh, or fish.\n' \
      "$shell_name" >&2
    exit 1
    ;;
esac
touch "$profile"
python3 - "$profile" "$path_line" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path_line = sys.argv[2]
begin = "# >>> skill-manager managed PATH >>>"
end = "# <<< skill-manager managed PATH <<<"
lines = path.read_text(encoding="utf-8").splitlines()
kept = []
inside = False
for line in lines:
    if line == begin:
        inside = True
        continue
    if inside:
        if line == end:
            inside = False
        continue
    if line.strip() == path_line:
        continue
    kept.append(line)
if inside:
    raise SystemExit("unterminated skill-manager PATH marker in shell profile")
while kept and not kept[-1]:
    kept.pop()
kept.extend(["", begin, path_line, end])
path.write_text("\n".join(kept) + "\n", encoding="utf-8")
PY
if [ -n "$previous_path" ] && [ "$previous_path" != "$managed_binary" ]; then
  printf 'Warning: the external PATH executable is now shadowed by the validated managed install: %s\n' \
    "$previous_path" >&2
fi

invoke_recipe() {
  recipe=$1
  output_file=$2
  set +e
  printf '%s\n' "$recipe" | "$managed_binary" --json-input >"$output_file"
  exit_code=$?
  set -e
  python3 - "$output_file" "$exit_code" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
exit_code = int(sys.argv[2])
lines = path.read_text(encoding="utf-8").splitlines()
if not lines:
    raise SystemExit("skill-manager emitted no NDJSON events")
events = []
for number, line in enumerate(lines, 1):
    try:
        event = json.loads(line)
    except json.JSONDecodeError as error:
        raise SystemExit(f"NDJSON line {number} is invalid: {error}")
    if not isinstance(event, dict) or set(event) != {"version", "event", "level", "data"}:
        raise SystemExit(f"NDJSON line {number} has an invalid envelope")
    if type(event["version"]) is not int or event["version"] != 1:
        raise SystemExit(f"NDJSON line {number} has an unsupported version")
    if not isinstance(event["event"], str) or not event["event"]:
        raise SystemExit(f"NDJSON line {number} has an invalid event name")
    if event["level"] not in {"info", "warning", "error"}:
        raise SystemExit(f"NDJSON line {number} has an invalid level")
    if not isinstance(event["data"], dict):
        raise SystemExit(f"NDJSON line {number} data is not an object")
    events.append(event)
if exit_code:
    failures = [event for event in events if event["event"] == "command.failed"]
    if len(failures) != 1 or set(failures[0]["data"]) != {"message"} \
       or not isinstance(failures[0]["data"]["message"], str):
        raise SystemExit("failed command did not emit exactly one structured command.failed event")
PY
  if [ "$exit_code" -ne 0 ]; then
    command cat "$output_file"
    printf 'skill-manager recipe failed with exit %s.\n' "$exit_code" >&2
    return "$exit_code"
  fi
}

work_root="$(mktemp -d "${TMPDIR:-/tmp}/skill-manager-configure.XXXXXXXX")"
cleanup_work() { rm -rf "$work_root"; }
trap cleanup_work EXIT HUP INT TERM
source_output="$work_root/source-list.ndjson"
invoke_recipe '{"command":"source.list"}' "$source_output"
source_context="$work_root/source-context.json"
python3 - "$source_output" "$source_identity" "$requested_source_name" "$source_context" <<'PY'
import json
import pathlib
import re
import sys

events_path, identity, requested_name, context_path = sys.argv[1:]
events = [json.loads(line) for line in pathlib.Path(events_path).read_text(encoding="utf-8").splitlines()]
sources = []
for event in events:
    if event["event"] != "source.listed":
        continue
    data = event["data"]
    required = {
        "source", "source_id", "source_name", "source_label",
        "source_type", "mode", "alternate",
    }
    if set(data) != required:
        raise SystemExit("source.listed payload has an unexpected shape")
    if not all(isinstance(data[field], str) for field in (
        "source", "source_id", "source_name", "source_label", "source_type", "mode"
    )):
        raise SystemExit("source.listed identity fields must be strings")
    if re.fullmatch(r"src_[0-9a-f]+", data["source_id"]) is None:
        raise SystemExit("source.listed source_id is invalid")
    if data["alternate"] is not None and (
        not isinstance(data["alternate"], dict)
        or set(data["alternate"]) != {"source", "source_type"}
    ):
        raise SystemExit("source.listed alternate has an unexpected shape")
    sources.append(data)
exact = [source for source in sources if source["source"] == identity]
alternate_exact = [
    source for source in sources
    if isinstance(source["alternate"], dict) and source["alternate"]["source"] == identity
]
named = [source for source in sources if source["source_name"] == requested_name]
if len(exact) + len(alternate_exact) > 1:
    raise SystemExit(f"multiple active/alternate source slots match {identity}")
if exact:
    context = {"add": False, "swap": False, "source_id": exact[0]["source_id"], "source_name": exact[0]["source_name"]}
elif alternate_exact:
    source = alternate_exact[0]
    context = {"add": False, "swap": True, "source_id": source["source_id"], "source_name": source["source_name"]}
elif named:
    raise SystemExit(f"source name {requested_name} identifies a different active source")
else:
    context = {"add": True, "swap": False}
pathlib.Path(context_path).write_text(json.dumps(context), encoding="utf-8")
PY
needs_add="$(python3 - "$source_context" <<'PY'
import json, pathlib, sys
print("true" if json.loads(pathlib.Path(sys.argv[1]).read_text())["add"] else "false")
PY
)"
if [ "$needs_add" = true ]; then
  add_output="$work_root/source-add.ndjson"
  invoke_recipe \
    '{"command":"source.add","source":"sernst/skills/skills","name":"sernst-skills","label":"sernst skills"}' \
    "$add_output"
  python3 - "$add_output" "$source_identity" "$requested_source_name" "$source_context" <<'PY'
import json
import pathlib
import re
import sys

events_path, identity, name, context_path = sys.argv[1:]
events = [json.loads(line) for line in pathlib.Path(events_path).read_text(encoding="utf-8").splitlines()]
added = [event["data"] for event in events if event["event"] == "source.added"]
if len(added) != 1:
    raise SystemExit("source.add must emit exactly one source.added event")
data = added[0]
required = {
    "source", "source_id", "source_name", "source_label",
    "source_type", "mode", "alternate",
}
if set(data) != required:
    raise SystemExit("source.added payload has an unexpected shape")
if data.get("source") != identity or data.get("source_name") != name:
    raise SystemExit("source.added does not match the requested source")
if not isinstance(data.get("source_id"), str) or re.fullmatch(r"src_[0-9a-f]+", data["source_id"]) is None:
    raise SystemExit("source.added source_id is invalid")
if not all(isinstance(data[field], str) for field in (
    "source", "source_id", "source_name", "source_label", "source_type", "mode"
)):
    raise SystemExit("source.added identity fields must be strings")
if data["alternate"] is not None and (
    not isinstance(data["alternate"], dict)
    or set(data["alternate"]) != {"source", "source_type"}
):
    raise SystemExit("source.added alternate has an unexpected shape")
pathlib.Path(context_path).write_text(json.dumps(
    {"add": False, "swap": False, "source_id": data["source_id"], "source_name": data["source_name"]}
), encoding="utf-8")
PY
fi
needs_swap="$(python3 - "$source_context" <<'PY'
import json, pathlib, sys
print("true" if json.loads(pathlib.Path(sys.argv[1]).read_text())["swap"] else "false")
PY
)"
if [ "$needs_swap" = true ]; then
  swap_selector="$(python3 - "$source_context" <<'PY'
import json, pathlib, sys
print(json.loads(pathlib.Path(sys.argv[1]).read_text())["source_id"])
PY
)"
  swap_output="$work_root/source-swap.ndjson"
  invoke_recipe "{\"command\":\"source.swap\",\"source\":\"$swap_selector\"}" "$swap_output"
  confirm_output="$work_root/source-confirm.ndjson"
  invoke_recipe '{"command":"source.list"}' "$confirm_output"
  python3 - "$swap_output" "$confirm_output" "$source_identity" "$swap_selector" <<'PY'
import json, pathlib, sys
swap_path, list_path, identity, selector = sys.argv[1:]
swap_events = [json.loads(line) for line in pathlib.Path(swap_path).read_text(encoding="utf-8").splitlines()]
matches = [event["data"] for event in swap_events if event["event"] == "source.locations-swapped"]
required = {"source", "source_id", "source_name", "source_label", "source_type",
            "mode", "alternate", "changed", "previous"}
if len(matches) != 1 or set(matches[0]) != required:
    raise SystemExit("source.swap must emit one exact source.locations-swapped payload")
if matches[0]["source"] != identity or matches[0]["source_id"] != selector or matches[0]["changed"] is not True:
    raise SystemExit("source.swap did not activate the requested remote")
if not isinstance(matches[0]["previous"], dict) or set(matches[0]["previous"]) != {"source", "source_type", "alternate"}:
    raise SystemExit("source.locations-swapped previous payload is invalid")
listed = [json.loads(line) for line in pathlib.Path(list_path).read_text(encoding="utf-8").splitlines()]
confirmed = [event for event in listed if event["event"] == "source.listed"
             and event["data"].get("source_id") == selector
             and event["data"].get("source") == identity]
if len(confirmed) != 1:
    raise SystemExit("source.list did not confirm the swapped active remote")
PY
fi
source_selector="$(python3 - "$source_context" <<'PY'
import json, pathlib, sys
print(json.loads(pathlib.Path(sys.argv[1]).read_text())["source_id"])
PY
)"

expected_targets='shared'
if [ "$SKILL_MANAGER_HARNESS" = claude ]; then
  expected_targets="$expected_targets claude"
elif [ "$SKILL_MANAGER_HARNESS" = antigravity ]; then
  expected_targets="$expected_targets antigravity"
fi
targets_json="$(python3 - $expected_targets <<'PY'
import json
import sys
print(json.dumps(list(dict.fromkeys(sys.argv[1:]))))
PY
)"

dry_output="$work_root/load-dry.ndjson"
invoke_recipe \
  "{\"command\":\"load\",\"sources\":[\"$source_selector\"],\"filters\":[\"managing-skills\"],\"targets\":$targets_json,\"global\":true,\"dry_run\":true}" \
  "$dry_output"
python3 - "$dry_output" true $expected_targets <<'PY'
import json, pathlib, re, sys
events = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()]
dry_run = sys.argv[2] == "true"
required = {
    "source", "source_id", "source_name", "source_label", "source_type",
    "mode", "alternate", "skill", "path", "target", "scope",
    "target_path", "destination", "action", "dry_run",
}
actions = [event for event in events if event["event"] in {
    "skill.loaded", "skill.updated", "skill.skipped", "skill.copied", "skill.removed"
}]
if len(actions) != len(sys.argv[3:]):
    raise SystemExit("dry-run action-event set does not equal expected tuples")
for target in sys.argv[3:]:
    matches = [event for event in actions if event["event"] in {"skill.loaded", "skill.skipped"}
               and event["data"].get("skill") == "managing-skills"
               and event["data"].get("target") == target
               and event["data"].get("scope") == "global"
               and event["data"].get("dry_run") is dry_run]
    if len(matches) != 1:
        raise SystemExit(f"expected exactly one correlated action event for {target}")
    data = matches[0]["data"]
    if set(data) != required:
        raise SystemExit(f"dry-run action payload has an unexpected shape for {target}")
    for field in {
        "source", "source_id", "source_name", "source_label", "source_type",
        "mode", "skill", "path", "target", "scope", "target_path",
        "destination", "action",
    }:
        if not isinstance(data[field], str) or not data[field]:
            raise SystemExit(f"dry-run action {field} is not a nonblank string for {target}")
    if re.fullmatch(r"src_[0-9a-f]+", data["source_id"]) is None:
        raise SystemExit(f"dry-run action source_id is invalid for {target}")
    if type(data["dry_run"]) is not bool:
        raise SystemExit(f"dry-run action dry_run is not boolean for {target}")
    if data["alternate"] is not None and (
        not isinstance(data["alternate"], dict)
        or set(data["alternate"]) != {"source", "source_type"}
    ):
        raise SystemExit(f"dry-run action alternate has an unexpected shape for {target}")
PY

load_output="$work_root/load.ndjson"
invoke_recipe \
  "{\"command\":\"load\",\"sources\":[\"$source_selector\"],\"filters\":[\"managing-skills\"],\"targets\":$targets_json,\"global\":true}" \
  "$load_output"
python3 - "$load_output" false $expected_targets <<'PY'
import json, pathlib, re, sys
events = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()]
dry_run = sys.argv[2] == "true"
required = {
    "source", "source_id", "source_name", "source_label", "source_type",
    "mode", "alternate", "skill", "path", "target", "scope",
    "target_path", "destination", "action", "dry_run",
}
actions = [event for event in events if event["event"] in {
    "skill.loaded", "skill.updated", "skill.skipped", "skill.copied", "skill.removed"
}]
if len(actions) != len(sys.argv[3:]):
    raise SystemExit("committed action-event set does not equal expected tuples")
for target in sys.argv[3:]:
    matches = [event for event in actions if event["event"] in {"skill.loaded", "skill.skipped"}
               and event["data"].get("skill") == "managing-skills"
               and event["data"].get("target") == target
               and event["data"].get("scope") == "global"
               and event["data"].get("dry_run") is dry_run]
    if len(matches) != 1:
        raise SystemExit(f"expected exactly one correlated committed action for {target}")
    data = matches[0]["data"]
    if set(data) != required:
        raise SystemExit(f"committed action payload has an unexpected shape for {target}")
    for field in {
        "source", "source_id", "source_name", "source_label", "source_type",
        "mode", "skill", "path", "target", "scope", "target_path",
        "destination", "action",
    }:
        if not isinstance(data[field], str) or not data[field]:
            raise SystemExit(f"committed action {field} is not a nonblank string for {target}")
    if re.fullmatch(r"src_[0-9a-f]+", data["source_id"]) is None:
        raise SystemExit(f"committed action source_id is invalid for {target}")
    if type(data["dry_run"]) is not bool:
        raise SystemExit(f"committed action dry_run is not boolean for {target}")
    if data["alternate"] is not None and (
        not isinstance(data["alternate"], dict)
        or set(data["alternate"]) != {"source", "source_type"}
    ):
        raise SystemExit(f"committed action alternate has an unexpected shape for {target}")
PY

status_output="$work_root/status.ndjson"
invoke_recipe \
  "{\"command\":\"status\",\"filters\":[\"managing-skills\"],\"targets\":$targets_json,\"global\":true}" \
  "$status_output"
python3 - "$status_output" $expected_targets <<'PY'
import json
import pathlib
import sys

events = [json.loads(line) for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()]
targets = sys.argv[2:]
rows = [event["data"] for event in events
        if event["event"] == "status.row" and event["data"].get("skill") == "managing-skills"]
if len(rows) != 1:
    raise SystemExit("status must return exactly one managing-skills row")
row = rows[0]
if set(row) != {
    "skill", "source", "targets", "location", "mixed",
    "shadowed_global_divergent", "deployments",
}:
    raise SystemExit("status.row payload has an unexpected shape")
deployments = row.get("deployments")
if not isinstance(deployments, list):
    raise SystemExit("status.row deployments must be an array")
if len(deployments) != len(targets):
    raise SystemExit("status deployment set does not equal expected target/scope tuples")
if not isinstance(row["source"], (dict, type(None))):
    raise SystemExit("status.row source must be a source object or null")
if row["source"] is not None:
    required_source = {
        "source", "source_id", "source_name", "source_label",
        "source_type", "mode", "alternate",
    }
    if set(row["source"]) != required_source:
        raise SystemExit("status.row source has an unexpected shape")
    if not all(isinstance(row["source"][field], str) for field in (
        "source", "source_id", "source_name", "source_label", "source_type", "mode"
    )):
        raise SystemExit("status.row source identity fields must be strings")
    if row["source"]["alternate"] is not None and (
        not isinstance(row["source"]["alternate"], dict)
        or set(row["source"]["alternate"]) != {"source", "source_type"}
    ):
        raise SystemExit("status.row source alternate has an unexpected shape")
if not isinstance(row["targets"], dict) or not all(
    isinstance(key, str) and isinstance(value, str)
    for key, value in row["targets"].items()
):
    raise SystemExit("status.row targets must map target strings to state strings")
if not isinstance(row["skill"], str) or not row["skill"] \
   or not isinstance(row["location"], str) or type(row["mixed"]) is not bool \
   or type(row["shadowed_global_divergent"]) is not bool:
    raise SystemExit("status.row aggregate fields have invalid types")
for deployment in deployments:
    if not isinstance(deployment, dict) or set(deployment) != {
        "target", "scope", "path", "installed", "state", "effective",
    }:
        raise SystemExit("status deployment payload has an unexpected shape")
    if not all(isinstance(deployment[field], str) and deployment[field]
               for field in {"target", "scope", "path", "state"}):
        raise SystemExit("status deployment string fields are invalid")
    if type(deployment["installed"]) is not bool or type(deployment["effective"]) is not bool:
        raise SystemExit("status deployment boolean fields are invalid")
skill_paths = []
for target in targets:
    matches = [item for item in deployments if isinstance(item, dict)
               and item.get("target") == target
               and item.get("scope") == "global"
               and item.get("installed") is True
               and isinstance(item.get("path"), str) and item["path"]]
    if len(matches) != 1:
        raise SystemExit(f"status must contain one installed global deployment for {target}")
    skill_paths.append(pathlib.Path(matches[0]["path"]) / "SKILL.md")
unique_skill_paths = list(dict.fromkeys(skill_paths))
for path in unique_skill_paths:
    if not path.is_file():
        raise SystemExit(f"installed skill is missing: {path}")
    print(f"----- BEGIN INSTALLED SKILL: {path} -----")
    print(path.read_text(encoding="utf-8"), end="")
    print(f"----- END INSTALLED SKILL: {path} -----")
PY
source_name="$(python3 - "$source_context" <<'PY'
import json, pathlib, sys
print(json.loads(pathlib.Path(sys.argv[1]).read_text())["source_name"])
PY
)"
printf "Installed skill-manager and managing-skills from source '%s' for: %s.\n" \
  "$source_name" "$expected_targets"
cleanup_work
trap - EXIT HUP INT TERM
```

## Completion criteria

Report:

- managed binary path and `skill-manager --version`;
- release tag, version, asset, verified SHA-256, and installation timestamp from
  the strictly validated `install-provenance.json`;
- whether an older PATH executable was shadowed;
- the reused/added source name for `sernst/skills/skills`;
- verified global targets (`shared`, plus the harness-specific target);
- whether the current harness needs a new session to discover the skill.

This branch can validate the new skill locally through a local source. The
remote installation path becomes externally testable only after these files are
merged into the default branch; perform one post-merge paste-playbook smoke as
final acceptance.
