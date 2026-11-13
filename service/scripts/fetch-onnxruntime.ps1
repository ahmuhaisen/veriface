<#
.SYNOPSIS
    Downloads Microsoft's official ONNX Runtime release that veriface loads at runtime.

.DESCRIPTION
    The service uses the `ort` crate 2.0.0-rc.13 with `load-dynamic`, which wraps the ONNX Runtime
    1.28 C API. This script fetches the matching official Microsoft build (v1.28.2, CPU only) from
    https://github.com/microsoft/onnxruntime/releases, verifies the archive against the SHA-256 pinned
    below (equal to the digest GitHub publishes for the release asset), and extracts only the runtime
    libraries plus the licence files into

        service/third_party/onnxruntime/1.28.2/<platform>/

    `service/third_party/` is git-ignored. Debug symbols (.pdb, ~400 MB) and headers are not extracted.

    After extraction the script prints the SHA-256 of each library; those are pinned below too and the
    script fails if they differ.

.PARAMETER Platform
    win-x64 (default), linux-x64, or all. linux-x64 can be fetched on Windows for container builds;
    on Windows only the versioned libonnxruntime.so.1.28.2 is written (no symlinks), so load it by
    that full path.

.PARAMETER Force
    Re-download and re-extract even if the libraries are already present.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File service/scripts/fetch-onnxruntime.ps1
    # -> service/third_party/onnxruntime/1.28.2/win-x64/onnxruntime.dll
    # Tests pick it up there, or set ORT_DYLIB_PATH to its full path.
#>
[CmdletBinding()]
param(
    [ValidateSet('win-x64', 'linux-x64', 'all')]
    [string] $Platform = 'win-x64',
    [switch] $Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow with the progress bar on PS 5.1

$Version = '1.28.2'
$BaseUrl = "https://github.com/microsoft/onnxruntime/releases/download/v$Version"

# Pinned: archive SHA-256 = GitHub release asset digest; library SHA-256 = extracted file.
$Releases = @{
    'win-x64'   = @{
        Archive       = "onnxruntime-win-x64-$Version.zip"
        ArchiveSha256 = 'c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35'
        Members       = [ordered]@{
            'lib/onnxruntime.dll'                  = '1becbd71adbf49609d33195e29c9214969db3c3de69c81425bebe5a4b69aef97'
            'lib/onnxruntime_providers_shared.dll' = '2e3a2a3903f1d4b1b15cd6887593ed2f269cd2b24e3d729fead45406ec9a25ac'
            'LICENSE'                              = $null
            'ThirdPartyNotices.txt'                = $null
            'VERSION_NUMBER'                       = $null
            'GIT_COMMIT_ID'                        = $null
        }
    }
    'linux-x64' = @{
        Archive       = "onnxruntime-linux-x64-$Version.tgz"
        ArchiveSha256 = 'd7209b8751b27b862b0c76332c2e20e203396edb5dab700ecf4bb485cf147415'
        Members       = [ordered]@{
            "lib/libonnxruntime.so.$Version"         = '088f24b1fc56714d3efaaeb3ac2ee486a5d7b50ccfbb3dd26fd1a612534a05fb'
            'lib/libonnxruntime_providers_shared.so' = '086ec1d5388f64153d9c63470d126693db9a182c8ce236d3a1119068471b8a0d'
            'LICENSE'                                = $null
            'ThirdPartyNotices.txt'                  = $null
            'VERSION_NUMBER'                         = $null
            'GIT_COMMIT_ID'                          = $null
        }
    }
}

function Get-Sha256([string] $Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

$ServiceDir = Split-Path -Parent $PSScriptRoot
$Root = Join-Path $ServiceDir (Join-Path 'third_party' (Join-Path 'onnxruntime' $Version))
New-Item -ItemType Directory -Force -Path $Root | Out-Null

$targets = if ($Platform -eq 'all') { @('win-x64', 'linux-x64') } else { @($Platform) }

foreach ($plat in $targets) {
    $rel = $Releases[$plat]
    $dest = Join-Path $Root $plat
    $libs = @($rel.Members.Keys | Where-Object { $rel.Members[$_] })
    $present = $true
    foreach ($m in $libs) {
        $p = Join-Path $dest (Split-Path -Leaf $m)
        if (-not (Test-Path -LiteralPath $p) -or (Get-Sha256 $p) -ne $rel.Members[$m]) { $present = $false }
    }
    if ($present -and -not $Force) {
        Write-Host "[$plat] already present and verified in $dest"
        continue
    }

    $archive = Join-Path $Root $rel.Archive
    $part = "$archive.part"
    $url = "$BaseUrl/$($rel.Archive)"
    Write-Host "[$plat] downloading $url"
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $part

    $actual = Get-Sha256 $part
    if ($actual -ne $rel.ArchiveSha256) {
        Remove-Item -LiteralPath $part -Force
        throw "[$plat] SHA-256 mismatch for $($rel.Archive): expected $($rel.ArchiveSha256), got $actual"
    }
    Move-Item -LiteralPath $part -Destination $archive -Force
    Write-Host "[$plat] archive SHA-256 verified: $actual"

    $staging = Join-Path $Root ".staging-$plat"
    if (Test-Path -LiteralPath $staging) { Remove-Item -LiteralPath $staging -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    $prefix = [IO.Path]::GetFileNameWithoutExtension($rel.Archive)   # onnxruntime-<plat>-<version>
    $memberPaths = @($rel.Members.Keys | ForEach-Object { "$prefix/$_" })

    if ($rel.Archive.EndsWith('.zip')) {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [IO.Compression.ZipFile]::OpenRead($archive)
        try {
            foreach ($member in $memberPaths) {
                $entry = $zip.GetEntry($member)
                if ($null -eq $entry) { throw "[$plat] $member not found in $($rel.Archive)" }
                $out = Join-Path $staging (Split-Path -Leaf $member)
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $out, $true)
            }
        } finally {
            $zip.Dispose()
        }
    } else {
        # Use the Windows bsdtar explicitly on Windows (Git's GNU tar may shadow it on PATH).
        $tar = if ($env:SystemRoot -and (Test-Path (Join-Path $env:SystemRoot 'System32\tar.exe'))) {
            Join-Path $env:SystemRoot 'System32\tar.exe'
        } else { 'tar' }
        $extract = Join-Path $staging 'x'
        New-Item -ItemType Directory -Force -Path $extract | Out-Null
        & $tar -xzf $archive -C $extract @memberPaths
        if ($LASTEXITCODE -ne 0) { throw "[$plat] tar failed with exit code $LASTEXITCODE" }
        foreach ($member in $memberPaths) {
            Move-Item -LiteralPath (Join-Path $extract $member) -Destination (Join-Path $staging (Split-Path -Leaf $member)) -Force
        }
        Remove-Item -LiteralPath $extract -Recurse -Force
    }

    foreach ($m in $libs) {
        $p = Join-Path $staging (Split-Path -Leaf $m)
        $h = Get-Sha256 $p
        if ($h -ne $rel.Members[$m]) {
            throw "[$plat] SHA-256 mismatch for extracted $m`: expected $($rel.Members[$m]), got $h"
        }
        Write-Host "[$plat] $(Split-Path -Leaf $m) SHA-256 $h"
    }

    if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
    Move-Item -LiteralPath $staging -Destination $dest
    Remove-Item -LiteralPath $archive -Force
    Write-Host "[$plat] ONNX Runtime $Version ready in $dest"
}
