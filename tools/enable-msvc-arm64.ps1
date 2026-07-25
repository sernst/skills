$ErrorActionPreference = 'Stop'
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { throw 'vswhere.exe was not found.' }
$installation = [string](& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath | Select-Object -First 1)
$installation = $installation.Trim()
if (-not $installation) {
    $installation = [string](& $vswhere -latest -products '*' -property installationPath | Select-Object -First 1)
    $installation = $installation.Trim()
    $installer = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\setup.exe"
    if (-not $installation -or -not (Test-Path $installer)) { throw 'Visual Studio Build Tools are not installed on this runner.' }
    $process = Start-Process -FilePath $installer -Wait -PassThru -ArgumentList @(
        'modify', '--installPath', $installation,
        '--add', 'Microsoft.VisualStudio.Component.VC.Tools.ARM64',
        '--quiet', '--norestart'
    )
    if ($process.ExitCode -ne 0) { throw "Visual Studio ARM64 component installation failed with $($process.ExitCode)." }
}
$installation = [string](& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath | Select-Object -First 1)
$installation = $installation.Trim()
if (-not $installation) { throw 'The Visual C++ ARM64 tools are not installed on this runner.' }
$developerCommand = Join-Path $installation 'Common7\Tools\VsDevCmd.bat'
$environment = cmd /s /c "`"$developerCommand`" -no_logo -arch=arm64 -host_arch=x64 && set"
if ($LASTEXITCODE -ne 0) { throw 'Failed to initialize the MSVC ARM64 environment.' }
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') { "$($Matches[1])=$($Matches[2])" | Add-Content $env:GITHUB_ENV }
}
