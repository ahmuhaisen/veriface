<#
.SYNOPSIS
    Builds the veriface container images (slim and full) and exports them for offline sites.

.DESCRIPTION
    Windows counterpart of build.sh, for Docker Desktop (Linux containers). Output in -OutDir
    (default service/dist):

      veriface-<version>-slim.docker.tar    docker load -i ...
      veriface-<version>-full.docker.tar
      veriface-<version>-models.zip         the model pack for the slim image
      *.sha256

    Needs: Docker Desktop with the Linux engine running, the ONNX Runtime linux-x64 library
    (service/scripts/fetch-onnxruntime.ps1 -Platform linux-x64) and the model files in models/.
#>
[CmdletBinding()]
param(
    [string] $Version,
    [string] $OutDir,
    [switch] $NoExport
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $OutDir) {
    $OutDir = Join-Path (Join-Path $Root 'service') 'dist'
}
if (-not $Version) {
    $toml = Get-Content -Raw -LiteralPath (Join-Path $Root 'service\Cargo.toml')
    $Version = [regex]::Match($toml, '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
}
$ortLib = Join-Path $Root 'service\third_party\onnxruntime\1.28.2\linux-x64\libonnxruntime.so.1.28.2'
if (-not (Test-Path -LiteralPath $ortLib)) {
    throw "missing ${ortLib}: run service\scripts\fetch-onnxruntime.ps1 -Platform linux-x64"
}
$revision = 'unknown'
$git = & git -C $Root rev-parse HEAD 2>$null
if ($LASTEXITCODE -eq 0) {
    $revision = $git
}

function Write-Sha([string] $Path) {
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText("$Path.sha256", "$hash  $(Split-Path -Leaf $Path)`n", (New-Object Text.UTF8Encoding $false))
}

foreach ($target in @('slim', 'full')) {
    $tag = "veriface:$Version-$target"
    Write-Host "==> building $tag"
    & docker build -f (Join-Path $Root 'deploy\docker\Dockerfile') --target $target `
        --build-arg "VERSION=$Version" --build-arg "REVISION=$revision" -t $tag $Root
    if ($LASTEXITCODE -ne 0) {
        throw "docker build ($target) failed with exit code $LASTEXITCODE"
    }
}

if (-not $NoExport) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    foreach ($target in @('slim', 'full')) {
        $file = Join-Path $OutDir "veriface-$Version-$target.docker.tar"
        Write-Host "==> exporting $file"
        & docker save -o $file "veriface:$Version-$target"
        if ($LASTEXITCODE -ne 0) {
            throw "docker save ($target) failed"
        }
        Write-Sha $file
    }
    $manifestPath = Join-Path $Root 'models\manifest.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    $staging = Join-Path ([IO.Path]::GetTempPath()) ("veriface-models-" + [guid]::NewGuid().ToString('N'))
    $packDir = Join-Path $staging 'models'
    New-Item -ItemType Directory -Force -Path $packDir | Out-Null
    try {
        Copy-Item -LiteralPath $manifestPath -Destination $packDir
        # The same files the full image bakes in.
        foreach ($entry in @($manifest.models) + @($manifest.assets)) {
            Copy-Item -LiteralPath (Join-Path $Root "models\$($entry.file)") -Destination $packDir
        }
        $pack = Join-Path $OutDir "veriface-$Version-models.zip"
        if (Test-Path -LiteralPath $pack) {
            Remove-Item -LiteralPath $pack -Force
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::CreateFromDirectory($packDir, $pack, [IO.Compression.CompressionLevel]::Optimal, $true)
        Write-Sha $pack
    } finally {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    Get-ChildItem -LiteralPath $OutDir -Filter "veriface-$Version-*" | Format-Table Name, Length -AutoSize
}
