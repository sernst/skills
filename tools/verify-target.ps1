param(
    [Parameter(Mandatory = $true)] [string] $Path,
    [Parameter(Mandatory = $true)] [string] $Target
)

$ErrorActionPreference = 'Stop'
$resolvedPath = (Resolve-Path -LiteralPath $Path).Path
[byte[]]$bytes = [System.IO.File]::ReadAllBytes($resolvedPath)
if ($bytes.Length -lt 64) { throw 'Binary is unexpectedly short.' }
if ($Target -match 'windows') {
    if ($bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw 'Expected a PE binary.' }
    $offset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ($offset -lt 0 -or $offset + 6 -gt $bytes.Length) { throw 'PE header offset is outside the binary.' }
    $machine = [BitConverter]::ToUInt16($bytes, $offset + 4)
    $expected = if ($Target -match '^aarch64') { 0xaa64 } else { 0x8664 }
    if ($machine -ne $expected) { throw "PE architecture $machine does not match $Target." }
} elseif ($Target -match 'apple') {
    $magic = ('{0:X2}{1:X2}{2:X2}{3:X2}' -f $bytes[0],$bytes[1],$bytes[2],$bytes[3])
    if ($magic -ne 'CFFAEDFE') { throw 'Expected a little-endian 64-bit Mach-O binary.' }
    $cpuType = [BitConverter]::ToUInt32($bytes, 4)
    $expectedCpu = if ($Target -match '^aarch64') { 0x0100000c } else { 0x01000007 }
    if ($cpuType -ne $expectedCpu) { throw "Mach-O CPU type $cpuType does not match $Target." }
} else {
    if ($bytes[0] -ne 0x7f -or $bytes[1] -ne 0x45 -or $bytes[2] -ne 0x4c -or $bytes[3] -ne 0x46) { throw 'Expected an ELF binary.' }
    $machine = [BitConverter]::ToUInt16($bytes, 18)
    $expected = if ($Target -match '^aarch64') { 183 } else { 62 }
    if ($machine -ne $expected) { throw "ELF architecture $machine does not match $Target." }
}
