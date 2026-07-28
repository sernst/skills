function Resolve-SmokeCanonicalExistingPath {
    param([Parameter(Mandatory = $true)] [string] $Path)

    if ($IsWindows) {
        if (-not ('SkillManagerSmoke.NativePath' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace SkillManagerSmoke
{
    public static class NativePath
    {
        private const uint FileFlagBackupSemantics = 0x02000000;

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern SafeFileHandle CreateFileW(
            string fileName,
            uint desiredAccess,
            FileShare shareMode,
            IntPtr securityAttributes,
            FileMode creationDisposition,
            uint flagsAndAttributes,
            IntPtr templateFile);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern uint GetFinalPathNameByHandleW(
            SafeFileHandle file,
            StringBuilder path,
            uint pathLength,
            uint flags);

        public static string Canonicalize(string path)
        {
            using (SafeFileHandle handle = CreateFileW(
                path,
                0,
                FileShare.Read | FileShare.Write | FileShare.Delete,
                IntPtr.Zero,
                FileMode.Open,
                FileFlagBackupSemantics,
                IntPtr.Zero))
            {
                if (handle.IsInvalid)
                {
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not open path for canonicalization.");
                }

                uint capacity = 512;
                while (true)
                {
                    StringBuilder result = new StringBuilder((int)capacity);
                    uint length = GetFinalPathNameByHandleW(handle, result, capacity, 0);
                    if (length == 0)
                    {
                        throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not canonicalize path.");
                    }
                    if (length < capacity)
                    {
                        string finalPath = result.ToString();
                        if (finalPath.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase))
                        {
                            return @"\\" + finalPath.Substring(8);
                        }
                        if (finalPath.StartsWith(@"\\?\", StringComparison.OrdinalIgnoreCase))
                        {
                            return finalPath.Substring(4);
                        }
                        return finalPath;
                    }
                    capacity = length + 1;
                }
            }
        }
    }
}
'@
        }
        return [SkillManagerSmoke.NativePath]::Canonicalize($Path)
    }

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    $realpath = Get-Command realpath -CommandType Application -ErrorAction SilentlyContinue
    if (-not $realpath) {
        throw 'realpath is required to canonicalize live-smoke cleanup paths on this platform.'
    }
    $canonical = & $realpath.Source $resolved
    if ($LASTEXITCODE -ne 0 -or -not $canonical) {
        throw "Could not canonicalize live-smoke path: $Path"
    }
    [IO.Path]::GetFullPath(([string]$canonical).Trim())
}

function Assert-SmokePathContained {
    param(
        [Parameter(Mandatory = $true)] [string] $CanonicalTempRoot,
        [Parameter(Mandatory = $true)] [string] $CanonicalSmokeRoot
    )

    $tempRoot = [IO.Path]::GetFullPath($CanonicalTempRoot)
    $smokeRoot = [IO.Path]::GetFullPath($CanonicalSmokeRoot)
    $comparison = if ($IsWindows) {
        [StringComparison]::OrdinalIgnoreCase
    } else {
        [StringComparison]::Ordinal
    }
    $rootPrefix = if (
        $tempRoot.EndsWith([IO.Path]::DirectorySeparatorChar) -or
        $tempRoot.EndsWith([IO.Path]::AltDirectorySeparatorChar)
    ) {
        $tempRoot
    } else {
        $tempRoot + [IO.Path]::DirectorySeparatorChar
    }
    $smokeName = [IO.Path]::GetFileName(
        $smokeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    )
    $smokeParent = [IO.Path]::GetDirectoryName(
        $smokeRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar)
    )
    if (
        -not $smokeRoot.StartsWith($rootPrefix, $comparison) -or
        -not $tempRoot.Equals($smokeParent, $comparison) -or
        $smokeName -notlike 'skill-manager-live-smoke-*'
    ) {
        throw "[SMOKE_PATH_ESCAPE] Refusing to remove unexpected smoke path: $smokeRoot"
    }
}
