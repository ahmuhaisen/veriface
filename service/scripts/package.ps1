<#
.SYNOPSIS
    Builds veriface in release mode and assembles a versioned, self-contained release bundle.

.DESCRIPTION
    1. cargo build --release --locked -p face-server --bin veriface (in service/).
    2. Optionally signs the executable (Authenticode; see deploy/windows/SIGNING.md).
    3. Assembles the bundle directory veriface-<version>-<platform>/:

         veriface.exe              (veriface on Linux)
         onnxruntime.dll, ...         when scripts/fetch-onnxruntime.* has fetched them
         config.example.toml
         profiles/*.yaml              from the repository's profiles/
         models/manifest.json         plus README.txt; with -IncludeModels also the model and
                                      asset files, each checked against the manifest's SHA-256
         install/                     deploy/windows/* (Windows) or deploy/linux/* (Linux):
                                      install scripts, service unit, config template
         licenses/                    ONNX Runtime LICENSE + ThirdPartyNotices, Apache-2.0 text,
                                      THIRD-PARTY-RUST.txt (every compiled-in crate)
         sbom/models.cdx.json         CycloneDX BOM of the model pack (scripts/model-bom.ps1),
                                      plus any -Sbom files (e.g. the cargo-cyclonedx SBOM)
         NOTICE.txt                   what the bundle contains and under which licences
         BUILDINFO.json               version, platform, commit, toolchain, ORT and manifest hashes
         logs/                        empty (for running from the folder)
         SHA256SUMS                   every file above, in `sha256sum -c` format

    4. With -Archive: veriface-<version>-<platform>.zip (Windows) or .tar.gz (Linux) next to
       the bundle directory, plus <archive>.sha256.

    The bundle directory is replaced. It must not exist, be empty, or be a previous package (one
    that contains SHA256SUMS); anything else is refused.

    Works with Windows PowerShell 5.1 and PowerShell 7 (also on Linux with pwsh).

.PARAMETER OutDir
    Bundle directory. Default: service/dist/veriface-<version>-<platform>.

.PARAMETER Version
    Release version (SemVer). Default: [workspace.package] version in service/Cargo.toml.

.PARAMETER IncludeModels
    Also copy the model and asset files listed in models/manifest.json (the "full" bundle).

.PARAMETER ModelPack
    Take the models from a lab model pack (veriface-models-<version>.zip from
    `python -m facelab models pack`, or its unpacked folder) instead of the repository's models/.
    Checked against <zip>.sha256 when present, its SHA256SUMS and PACK.json; implies
    -IncludeModels. The pack's licence texts, NOTICE and THIRD-PARTY-LICENSES.txt go into the
    bundle; its name and version into BUILDINFO.json.

.PARAMETER Archive
    Also write the .zip / .tar.gz and its .sha256.

.PARAMETER RequireApprovedModels
    Fail unless every model in the manifest is legally approved for commercial use (use for
    production releases).

.PARAMETER SkipBuild
    Package the existing release binary without running cargo.

.PARAMETER CertThumbprint
    Sign veriface.exe with this code-signing certificate from Cert:\CurrentUser\My or
    Cert:\LocalMachine\My (signtool /sha1).

.PARAMETER PfxPath
    Sign with a .pfx file instead (signtool /f); the password comes from -PfxPassword.

.PARAMETER PfxPassword
    Password of -PfxPath, as a SecureString.

.PARAMETER TimestampUrl
    RFC 3161 timestamp server. Default: http://timestamp.digicert.com.

.PARAMETER SignTool
    Path to signtool.exe. Default: the newest one in the Windows 10/11 SDK.

.PARAMETER Sbom
    Extra SBOM files to place in sbom/ (e.g. the output of `cargo cyclonedx`).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File service/scripts/package.ps1 -IncludeModels -Archive

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File service/scripts/package.ps1 -IncludeModels -Archive -RequireApprovedModels -CertThumbprint 0123...
#>
[CmdletBinding()]
param(
    [string] $OutDir,
    [string] $Version,
    [switch] $IncludeModels,
    [string] $ModelPack,
    [switch] $Archive,
    [switch] $RequireApprovedModels,
    [switch] $SkipBuild,
    [string] $CertThumbprint,
    [string] $PfxPath,
    [Security.SecureString] $PfxPassword,
    [string] $TimestampUrl = 'http://timestamp.digicert.com',
    [string] $SignTool,
    [string[]] $Sbom = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ServiceDir = Split-Path -Parent $PSScriptRoot
$RepoDir = Split-Path -Parent $ServiceDir
$DeployDir = Join-Path $RepoDir 'deploy'
$OnWindows = [IO.Path]::DirectorySeparatorChar -eq '\'
$Utf8NoBom = New-Object Text.UTF8Encoding $false

function Get-Sha256([string] $Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Write-TextFile([string] $Path, [string[]] $Lines) {
    [IO.File]::WriteAllText($Path, (($Lines -join "`n") + "`n"), $Utf8NoBom)
}

function Invoke-Native([string] $What, [scriptblock] $Command) {
    $output = & $Command 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code ${LASTEXITCODE}: $($output -join "`n")"
    }
    return $output
}

# ----------------------------------------------------------------------- version, platform

if (-not $Version) {
    $cargoToml = Get-Content -Raw -LiteralPath (Join-Path $ServiceDir 'Cargo.toml')
    $match = [regex]::Match($cargoToml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
    if (-not $match.Success) {
        throw 'cannot read [workspace.package] version from service/Cargo.toml; pass -Version'
    }
    $Version = $match.Groups[1].Value
}
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') {
    throw "version '$Version' is not SemVer (e.g. 1.2.0 or 1.2.0-rc.1)"
}
$platform = 'linux-x64'
$exeName = 'veriface'
$libraryPattern = 'libonnxruntime*.so*'
$installSource = Join-Path $DeployDir 'linux'
if ($OnWindows) {
    $platform = 'win-x64'
    $exeName = 'veriface.exe'
    $libraryPattern = 'onnxruntime*.dll'
    $installSource = Join-Path $DeployDir 'windows'
}
$bundleName = "veriface-$Version-$platform"
if (-not $OutDir) {
    $OutDir = Join-Path (Join-Path $ServiceDir 'dist') $bundleName
}
$OutDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)

$signing = [bool] ($CertThumbprint -or $PfxPath)
if ($signing -and -not $OnWindows) {
    throw 'Authenticode signing (-CertThumbprint / -PfxPath) is only available on Windows'
}

# ------------------------------------------------------------------------------- preflight

# Fail early on the model pack and the licence policy, before spending minutes on a build.
$modelsDir = Join-Path $RepoDir 'models'
$manifestPath = Join-Path $modelsDir 'manifest.json'
$pack = $null
$packTemp = $null
if ($ModelPack) {
    $packPath = (Resolve-Path -LiteralPath $ModelPack).ProviderPath
    $packDir = $packPath
    if (-not (Get-Item -LiteralPath $packPath).PSIsContainer) {
        $sidecar = "$packPath.sha256"
        if (Test-Path -LiteralPath $sidecar) {
            $expected = ((Get-Content -LiteralPath $sidecar -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
            if ($expected -ne (Get-Sha256 $packPath)) {
                throw "model pack SHA-256 does not match $sidecar"
            }
        } else {
            Write-Warning "no $(Split-Path -Leaf $sidecar): the pack archive's own hash is not checked (its SHA256SUMS still is)"
        }
        $packTemp = Join-Path ([IO.Path]::GetTempPath()) ('veriface-modelpack-' + [guid]::NewGuid().ToString('N'))
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::ExtractToDirectory($packPath, $packTemp)
        $top = @(Get-ChildItem -LiteralPath $packTemp -Directory)
        $packDir = $packTemp
        if ($top.Count -eq 1) {
            $packDir = $top[0].FullName
        }
    }
    foreach ($line in Get-Content -LiteralPath (Join-Path $packDir 'SHA256SUMS')) {
        if (-not $line.Trim()) {
            continue
        }
        $hash, $file = $line -split '  ', 2
        $full = Join-Path $packDir $file
        if (-not (Test-Path -LiteralPath $full) -or (Get-Sha256 $full) -ne $hash.ToLowerInvariant()) {
            throw "model pack integrity check failed: $file"
        }
    }
    $pack = Get-Content -Raw -LiteralPath (Join-Path $packDir 'PACK.json') | ConvertFrom-Json
    $modelsDir = Join-Path $packDir 'models'
    $manifestPath = Join-Path $modelsDir 'manifest.json'
    if ((Get-Sha256 $manifestPath) -ne $pack.manifestSha256.ToLowerInvariant()) {
        throw 'model pack: PACK.json manifestSha256 does not match models/manifest.json'
    }
    $IncludeModels = $true
    Write-Host "Model pack $($pack.name) $($pack.version) verified ($packDir)."
}
if ($RequireApprovedModels -and -not (Test-Path -LiteralPath $manifestPath)) {
    throw 'models/manifest.json not found; -RequireApprovedModels needs it'
}

# ------------------------------------------------------------------------------------- build

$targetDir = $env:CARGO_TARGET_DIR
if (-not $targetDir) {
    $targetDir = Join-Path $ServiceDir 'target'
} elseif (-not [IO.Path]::IsPathRooted($targetDir)) {
    $targetDir = Join-Path $ServiceDir $targetDir
}
$exe = Join-Path (Join-Path $targetDir 'release') $exeName

if (-not $SkipBuild) {
    Write-Host "Building $exeName $Version (release) ..."
    Push-Location $ServiceDir
    try {
        & cargo build --release --locked -p face-server --bin veriface
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path -LiteralPath $exe)) {
    throw "release binary not found: $exe"
}

# ---------------------------------------------------------------------------- output folder

if (Test-Path -LiteralPath $OutDir) {
    $existing = @(Get-ChildItem -LiteralPath $OutDir -Force)
    if ($existing.Count -gt 0 -and -not (Test-Path -LiteralPath (Join-Path $OutDir 'SHA256SUMS'))) {
        throw "refusing to replace '$OutDir': it is not empty and is not a previous package (no SHA256SUMS)"
    }
    Remove-Item -LiteralPath $OutDir -Recurse -Force
}
$outProfiles = Join-Path $OutDir 'profiles'
$outModels = Join-Path $OutDir 'models'
$outLicenses = Join-Path $OutDir 'licenses'
$outSbom = Join-Path $OutDir 'sbom'
foreach ($dir in @($OutDir, $outProfiles, $outModels, $outLicenses, $outSbom, (Join-Path $OutDir 'logs'))) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

$bundledExe = Join-Path $OutDir $exeName
Copy-Item -LiteralPath $exe -Destination $bundledExe
Copy-Item -LiteralPath (Join-Path $ServiceDir 'config.example.toml') -Destination $OutDir

$profiles = @(Get-ChildItem -LiteralPath (Join-Path $RepoDir 'profiles') -Filter '*.yaml' -File)
if ($profiles.Count -eq 0) {
    throw "no profiles found in $(Join-Path $RepoDir 'profiles')"
}
foreach ($profileFile in $profiles) {
    Copy-Item -LiteralPath $profileFile.FullName -Destination $outProfiles
}

# ----------------------------------------------------------------------------------- signing

if ($signing) {
    if (-not $SignTool) {
        $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        $SignTool = Get-ChildItem -LiteralPath $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1 -ExpandProperty FullName
        if (-not $SignTool) {
            throw 'signtool.exe not found; install the Windows SDK or pass -SignTool'
        }
    }
    $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/tr', $TimestampUrl, '/d', 'Veriface')
    if ($CertThumbprint) {
        $signArgs += @('/sha1', $CertThumbprint)
    } else {
        $signArgs += @('/f', $PfxPath)
        if ($PfxPassword) {
            $plain = [Runtime.InteropServices.Marshal]::PtrToStringBSTR(
                [Runtime.InteropServices.Marshal]::SecureStringToBSTR($PfxPassword))
            $signArgs += @('/p', $plain)
        }
    }
    Write-Host "Signing $exeName with $SignTool ..."
    & $SignTool @signArgs $bundledExe | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed with exit code $LASTEXITCODE"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $bundledExe
    if ($signature.Status -ne 'Valid') {
        throw "signature check failed after signing: $($signature.Status) $($signature.StatusMessage)"
    }
    Write-Host "Signed: $($signature.SignerCertificate.Subject) (timestamped: $([bool] $signature.TimeStamperCertificate))"
}

# ------------------------------------------------------------------------------------ models

$modelCount = 0
if (Test-Path -LiteralPath $manifestPath) {
    Copy-Item -LiteralPath $manifestPath -Destination $outModels
    if ($IncludeModels) {
        $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
        $entries = @($manifest.models)
        if ($manifest.PSObject.Properties.Name -contains 'assets') {
            $entries += @($manifest.assets)
        }
        foreach ($entry in $entries) {
            $source = Join-Path $modelsDir $entry.file
            if (-not (Test-Path -LiteralPath $source)) {
                throw "model file missing: $source (manifest id $($entry.id))"
            }
            $actual = Get-Sha256 $source
            if ($actual -ne $entry.sha256) {
                throw "SHA-256 mismatch for $($entry.file): manifest $($entry.sha256), file $actual"
            }
            $destination = Join-Path $outModels $entry.file
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
            Copy-Item -LiteralPath $source -Destination $destination
        }
        $modelCount = $entries.Count
        Write-Host "Copied $modelCount model and asset files (SHA-256 verified against manifest.json)."
    }
    $bomArgs = @{ Manifest = $manifestPath; Out = (Join-Path $outSbom 'models.cdx.json'); Version = $Version }
    if ($RequireApprovedModels) {
        $bomArgs.RequireApproved = $true
    }
    & (Join-Path $PSScriptRoot 'model-bom.ps1') @bomArgs
    if ($RequireApprovedModels -and $LASTEXITCODE -ne 0) {
        throw 'the model pack is not approved for production (see the errors above)'
    }
} else {
    if ($IncludeModels) {
        throw "models/manifest.json not found; the lab produces it (python -m facelab models build)"
    }
    Write-Warning 'models/manifest.json not found; the lab produces it (python -m facelab models build).'
}
if ($pack) {
    # The pack's licence texts and notices (its README.txt replaces the generic one below).
    $packLicenses = Join-Path $packDir 'licenses'
    if (Test-Path -LiteralPath $packLicenses) {
        Copy-Item -Path (Join-Path $packLicenses '*') -Destination $outLicenses -Recurse -Force
    }
    $packThirdParty = Join-Path $packDir 'THIRD-PARTY-LICENSES.txt'
    if (Test-Path -LiteralPath $packThirdParty) {
        Copy-Item -LiteralPath $packThirdParty -Destination (Join-Path $outLicenses 'THIRD-PARTY-MODELS.txt')
    }
    $packJson = Join-Path $packDir 'third-party-models.json'
    if (Test-Path -LiteralPath $packJson) {
        Copy-Item -LiteralPath $packJson -Destination $outSbom
    }
    Copy-Item -LiteralPath (Join-Path $packDir 'PACK.json') -Destination (Join-Path $outModels 'PACK.json')
}
if (-not (Test-Path -LiteralPath (Join-Path $modelsDir 'README.txt'))) {
Write-TextFile (Join-Path $outModels 'README.txt') @(
    'Model pack for veriface.',
    '',
    'manifest.json lists every model file with its source, licence and SHA-256. The files it lists',
    'belong in this directory. The service checks each SHA-256 at startup and refuses to start on',
    'a mismatch. Point paths.models in the configuration at this directory.'
)
} else {
    Copy-Item -LiteralPath (Join-Path $modelsDir 'README.txt') -Destination $outModels -Force
}

# ------------------------------------------------------------------------------ ONNX Runtime

$ortRoot = Join-Path (Join-Path $ServiceDir 'third_party') 'onnxruntime'
$ortDir = $null
$ortVersion = $null
$ortSha = $null
if (Test-Path -LiteralPath $ortRoot) {
    $ortDir = Get-ChildItem -LiteralPath $ortRoot -Directory |
        Where-Object { $_.Name -match '^\d+(\.\d+)+$' } |
        Sort-Object { [version] $_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName $platform } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Select-Object -First 1
}
if ($ortDir) {
    $ortVersion = Split-Path -Leaf (Split-Path -Parent $ortDir)
    # Real files only: the Linux symlinks (libonnxruntime.so -> .so.1 -> .so.1.28.2) would be
    # copied as duplicates; the service loads the fully versioned file.
    $libraries = @(Get-ChildItem -LiteralPath $ortDir -Filter $libraryPattern -File |
        Where-Object { -not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) })
    foreach ($library in $libraries) {
        Copy-Item -LiteralPath $library.FullName -Destination $OutDir
    }
    $mainLibrary = 'libonnxruntime.so.' + $ortVersion
    if ($OnWindows) {
        $mainLibrary = 'onnxruntime.dll'
    }
    if (Test-Path -LiteralPath (Join-Path $OutDir $mainLibrary)) {
        $ortSha = Get-Sha256 (Join-Path $OutDir $mainLibrary)
    }
    $ortLicenses = Join-Path $outLicenses 'onnxruntime'
    New-Item -ItemType Directory -Force -Path $ortLicenses | Out-Null
    foreach ($name in @('LICENSE', 'ThirdPartyNotices.txt', 'VERSION_NUMBER', 'GIT_COMMIT_ID')) {
        $file = Join-Path $ortDir $name
        if (Test-Path -LiteralPath $file) {
            Copy-Item -LiteralPath $file -Destination $ortLicenses
        }
    }
    Write-Host "Included ONNX Runtime $ortVersion from $ortDir ($($libraries.Count) libraries)."
} else {
    Write-Warning "ONNX Runtime not found under $ortRoot. Run scripts/fetch-onnxruntime.ps1 and package again, or copy the library next to $exeName."
}

# ------------------------------------------------------------------ install files, licences

if (Test-Path -LiteralPath $installSource) {
    $outInstall = Join-Path $OutDir 'install'
    Copy-Item -LiteralPath $installSource -Destination $outInstall -Recurse
}
if (-not $OnWindows) {
    # The tarball keeps these modes; nothing else in the bundle is executable.
    & chmod 0755 $bundledExe
    foreach ($script in @(Get-ChildItem -LiteralPath (Join-Path $OutDir 'install') -Filter '*.sh' -File -ErrorAction SilentlyContinue)) {
        & chmod 0755 $script.FullName
    }
}
Copy-Item -LiteralPath (Join-Path (Join-Path $DeployDir 'licenses') 'Apache-2.0.txt') -Destination $outLicenses

$rustTarget = 'x86_64-unknown-linux-gnu'
if ($OnWindows) {
    $rustTarget = 'x86_64-pc-windows-msvc'
}
& (Join-Path $PSScriptRoot 'third-party-licenses.ps1') -Target $rustTarget `
    -Out (Join-Path $outLicenses 'THIRD-PARTY-RUST.txt') -Json (Join-Path $outSbom 'rust-crates.json')
if ($LASTEXITCODE -ne 0) {
    throw 'licence policy violation in the Rust dependencies (see above)'
}
foreach ($file in $Sbom) {
    Copy-Item -LiteralPath $file -Destination $outSbom
}

# ------------------------------------------------------------------------ NOTICE, BUILDINFO

$gitCommit = $null
$gitDirty = $null
try {
    $gitCommit = (& git -C $RepoDir rev-parse HEAD 2>$null)
    if ($LASTEXITCODE -ne 0) {
        $gitCommit = $null
    } else {
        $gitDirty = [bool] (& git -C $RepoDir status --porcelain 2>$null)
    }
} catch {
    $gitCommit = $null
}
$rustc = $null
try {
    Push-Location $ServiceDir
    $rustc = (& rustc -V 2>$null)
} catch {
    $rustc = $null
} finally {
    Pop-Location
}
$buildInfo = [ordered]@{
    product = 'veriface'
    version = $Version
    platform = $platform
    builtAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    gitCommit = $gitCommit
    gitDirty = $gitDirty
    rustc = $rustc
    executable = [ordered]@{ file = $exeName; sha256 = (Get-Sha256 $bundledExe); signed = $signing }
    onnxRuntime = [ordered]@{ version = $ortVersion; sha256 = $ortSha }
    modelManifestSha256 = $(if (Test-Path -LiteralPath $manifestPath) { Get-Sha256 $manifestPath } else { $null })
    modelsIncluded = [bool] $IncludeModels
    modelPack = $(if ($pack) { [ordered]@{ name = $pack.name; version = $pack.version; manifestSha256 = $pack.manifestSha256 } } else { $null })
}
[IO.File]::WriteAllText((Join-Path $OutDir 'BUILDINFO.json'), ($buildInfo | ConvertTo-Json -Depth 4), $Utf8NoBom)

$notice = @(
    "Veriface $Version ($platform)",
    'Copyright (c) Ahmad Muhaisen. All rights reserved. Proprietary software: use is governed by your licence',
    'agreement with the copyright holder.',
    '',
    'This bundle also contains third-party components under their own licences:',
    '',
    '- Rust crates compiled into the executable: licenses/THIRD-PARTY-RUST.txt (MIT, Apache-2.0,',
    '  BSD, ISC, Zlib, Unicode and similar permissive licences; policy in service/deny.toml).'
)
if ($ortDir) {
    $notice += @(
        "- Microsoft ONNX Runtime $ortVersion (MIT): licenses/onnxruntime/LICENSE and",
        '  licenses/onnxruntime/ThirdPartyNotices.txt.'
    )
}
if ($pack -and (Test-Path -LiteralPath (Join-Path $packDir 'NOTICE'))) {
    $notice += "- Models: model pack $($pack.name) $($pack.version) in models/; licence texts in licenses/models/,"
    $notice += '  full notices in licenses/THIRD-PARTY-MODELS.txt, and the pack NOTICE below.'
    $notice += ''
    $notice += ('=' * 80)
    $notice += @(Get-Content -LiteralPath (Join-Path $packDir 'NOTICE'))
} elseif (Test-Path -LiteralPath $manifestPath) {
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $shipped = 'described'
    if ($IncludeModels) {
        $shipped = 'included'
    }
    $notice += "- Models ($shipped in models/; details, sources and SHA-256 in models/manifest.json and"
    $notice += '  sbom/models.cdx.json):'
    $entries = @($manifest.models)
    if ($manifest.PSObject.Properties.Name -contains 'assets') {
        $entries += @($manifest.assets)
    }
    foreach ($entry in $entries) {
        $upstreamName = ''
        $upstreamUrl = ''
        if ($entry.PSObject.Properties.Name -contains 'upstream') {
            $upstreamName = $entry.upstream.name
            $upstreamUrl = $entry.upstream.url
        }
        $notice += "    $($entry.id): $upstreamName, $($entry.licence.spdx). $upstreamUrl"
    }
    $notice += '  The Apache License 2.0 text is in licenses/Apache-2.0.txt. Model files converted from'
    $notice += '  TFLite to ONNX by the Veriface authors (see models/manifest.json "conversion"); modified files.'
}
Write-TextFile (Join-Path $OutDir 'NOTICE.txt') $notice

# ------------------------------------------------------------------------------- SHA256SUMS

$sumsPath = Join-Path $OutDir 'SHA256SUMS'
$prefix = $OutDir.TrimEnd('\', '/').Length + 1
$lines = @(Get-ChildItem -LiteralPath $OutDir -Recurse -File |
    Where-Object { $_.FullName -ne $sumsPath } |
    Sort-Object FullName |
    ForEach-Object {
        $relative = $_.FullName.Substring($prefix) -replace '\\', '/'
        '{0}  {1}' -f (Get-Sha256 $_.FullName), $relative
    })
Write-TextFile $sumsPath $lines

# ---------------------------------------------------------------------------------- archive

if ($Archive) {
    $parent = Split-Path -Parent $OutDir
    $leaf = Split-Path -Leaf $OutDir
    if ($OnWindows) {
        $archivePath = Join-Path $parent "$leaf.zip"
        if (Test-Path -LiteralPath $archivePath) {
            Remove-Item -LiteralPath $archivePath -Force
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        # includeBaseDirectory = true: the zip holds one top-level folder named like the bundle.
        [IO.Compression.ZipFile]::CreateFromDirectory($OutDir, $archivePath,
            [IO.Compression.CompressionLevel]::Optimal, $true)
    } else {
        $archivePath = Join-Path $parent "$leaf.tar.gz"
        Invoke-Native 'tar' { tar -C $parent -czf $archivePath $leaf } | Out-Null
    }
    $archiveSha = Get-Sha256 $archivePath
    Write-TextFile "$archivePath.sha256" @("$archiveSha  $(Split-Path -Leaf $archivePath)")
    Write-Host "Archive: $archivePath"
    Write-Host "SHA-256: $archiveSha"
}

if ($packTemp -and (Test-Path -LiteralPath $packTemp)) {
    Remove-Item -LiteralPath $packTemp -Recurse -Force
}

Write-Host ''
Write-Host "Bundle ready: $OutDir"
Get-ChildItem -LiteralPath $OutDir -Recurse -File |
    Sort-Object FullName |
    ForEach-Object {
        '  {0,12:N0}  {1}' -f $_.Length, ($_.FullName.Substring($prefix) -replace '\\', '/')
    } |
    Write-Host
