[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [ValidateSet('Failure', 'Recovery')] [string] $State,
    [Parameter(Mandatory = $true)] [string] $Repository,
    [Parameter(Mandatory = $true)] [string] $RunUrl,
    [string] $PullRequestNumber,
    [string] $SnapshotPath
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
$title = '[automation] Model benchmark refresh failed'
$issues = @(gh issue list --repo $Repository --state all --limit 100 --search "`"$title`" in:title" --json number,title,state | ConvertFrom-Json)
$issue = @($issues | Where-Object title -CEQ $title | Select-Object -First 1)

if ($State -eq 'Failure') {
    $body = "@sernst the model benchmark refresh failed closed. The last-known-good snapshot was retained. Inspect: $RunUrl"
    if (-not $issue.Count) {
        gh issue create --repo $Repository --title $title --body $body | Out-Host
    } else {
        if ($issue[0].state -eq 'CLOSED') { gh issue reopen $issue[0].number --repo $Repository | Out-Host }
        gh issue comment $issue[0].number --repo $Repository --body $body | Out-Host
    }
    exit 0
}

if (-not $issue.Count -or $issue[0].state -eq 'CLOSED') { exit 0 }
$provenance = if ($SnapshotPath -and (Test-Path -LiteralPath $SnapshotPath)) {
    @(Get-Content -LiteralPath $SnapshotPath | Where-Object { $_ -match '^Source:' }) -join "`n"
} else { 'Snapshot provenance unavailable in this run.' }
$prText = if ($PullRequestNumber) { "Update PR: #$PullRequestNumber" } else { 'No update PR was needed.' }
$body = "Refresh recovered. $prText`n`n$provenance`n`nRun: $RunUrl"
gh issue comment $issue[0].number --repo $Repository --body $body | Out-Host
gh issue close $issue[0].number --repo $Repository | Out-Host
