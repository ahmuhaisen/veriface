<#
.SYNOPSIS
    The release gate for veriface: every check a release must pass, in one command.

.DESCRIPTION
    Runs, in order, and reports each step as PASS, FAIL or NOT RUN (with the reason):

      fmt           cargo fmt --all --check
      clippy        cargo clippy --workspace --all-targets --locked -- -D warnings
      test          cargo test --workspace --locked   (the real-engine and golden tests run when
                    ONNX Runtime and the models are present, and skip otherwise)
      licences      scripts/third-party-licenses.ps1 for win-x64 and linux-x64: every compiled-in
                    crate's licence against service/deny.toml. Needs no extra tools. REQUIRED.
      deny          cargo deny check licenses bans sources   (when cargo-deny is installed)
      advisories    cargo deny check advisories, else cargo audit (when installed; needs the
                    advisory database, i.e. network or a local mirror)
      sbom          cargo cyclonedx (when cargo-cyclonedx is installed): CycloneDX SBOM of the crates
      model-bom     scripts/model-bom.ps1: CycloneDX BOM of the model pack; with
                    -RequireApprovedModels a model not legally approved fails the gate
      parity        lab/scripts/release-parity.ps1 against a running service (only with
                    -ParityService): the Rust pipeline against the lab's reference outputs

    The gate fails (exit 1) on any FAIL. A licence-policy violation is always a FAIL. NOT RUN
    never counts as passed: with -Strict, any NOT RUN also fails the gate (use it for a release).

    Logs and SBOMs go to -OutDir (default service/dist/release-check-<UTC timestamp>/), with a
    summary.json for tooling.

    Works with Windows PowerShell 5.1 and PowerShell 7 (also on Linux with pwsh).

    Wiring it into a pipeline later (GitHub Actions, Azure Pipelines): one job per OS
    (windows-latest, ubuntu-22.04) that installs the pinned toolchain (rust-toolchain.toml),
    cargo-deny, cargo-audit and cargo-cyclonedx, restores models/ and
    service/third_party/onnxruntime from a cache keyed on sha256(models/manifest.json) and the ORT
    version, runs `pwsh service/scripts/release-check.ps1 -Strict -RequireApprovedModels` and
    publishes -OutDir as an artifact. The exit code is the gate. No other pipeline logic is
    needed: everything lives in this script, so a local run and a pipeline run are the same.

.PARAMETER Strict
    Treat NOT RUN as a failure (missing tools, no parity run).

.PARAMETER RequireApprovedModels
    Fail unless every model in models/manifest.json is legally approved for commercial use.

.PARAMETER ParityService
    URL of a running veriface (e.g. http://127.0.0.1:18080) to run the lab parity gate against.

.PARAMETER ParityKeyEnv
    Environment variable holding an API key for -ParityService (default VERIFACE_API_KEY).

.PARAMETER ParityProfile
    Profile for the parity run (default fixtures; see lab/README.md).

.PARAMETER Skip
    Steps to skip (reported as NOT RUN), e.g. -Skip test,clippy for a quick licence check.

.PARAMETER OutDir
    Where logs, SBOMs and summary.json go.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File service/scripts/release-check.ps1

.EXAMPLE
    pwsh service/scripts/release-check.ps1 -Strict -RequireApprovedModels -ParityService http://127.0.0.1:18080
#>
[CmdletBinding()]
param(
    [switch] $Strict,
    [switch] $RequireApprovedModels,
    [string] $ParityService,
    [string] $ParityKeyEnv = 'VERIFACE_API_KEY',
    [string] $ParityProfile = 'fixtures',
    [string[]] $Skip = @(),
    [string] $OutDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ServiceDir = Split-Path -Parent $PSScriptRoot
$RepoDir = Split-Path -Parent $ServiceDir
if (-not $OutDir) {
    $OutDir = Join-Path (Join-Path $ServiceDir 'dist') ('release-check-' + (Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss'))
}
$OutDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (-not $env:CARGO_TARGET_DIR) {
    # Its own target dir: a release check must not wait on (or disturb) a developer's build.
    $env:CARGO_TARGET_DIR = Join-Path $ServiceDir 'target-release-check'
}
$Shell = 'powershell'
if ($PSVersionTable.PSEdition -eq 'Core') {
    $Shell = 'pwsh'
}

$results = New-Object System.Collections.Generic.List[object]

function Add-Result([string] $Step, [string] $Status, [string] $Detail, [double] $Seconds) {
    $results.Add([pscustomobject]@{ step = $Step; status = $Status; detail = $Detail; seconds = [math]::Round($Seconds, 1) })
    $color = @{ PASS = 'Green'; FAIL = 'Red'; 'NOT RUN' = 'Yellow' }[$Status]
    Write-Host ('{0,-8} {1,-11} {2}' -f $Status, $Step, $Detail) -ForegroundColor $color
}

function Test-Tool([string] $Name) {
    return [bool] (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Test-CargoSubcommand([string] $Name) {
    return (Test-Tool "cargo-$Name")
}

# Runs a command in service/, logging to <OutDir>/<step>.log. Returns the exit code.
function Invoke-Logged([string] $Step, [string] $Exe, [string[]] $Arguments) {
    $log = Join-Path $OutDir "$Step.log"
    $ErrorActionPreference = 'Continue'
    Push-Location $ServiceDir
    try {
        & $Exe @Arguments *>&1 | ForEach-Object { "$_" } | Out-File -FilePath $log -Encoding utf8
        return $LASTEXITCODE
    } finally {
        Pop-Location
    }
}

function Invoke-Step([string] $Step, [scriptblock] $Body) {
    if ($Skip -contains $Step) {
        Add-Result $Step 'NOT RUN' 'skipped (-Skip)' 0
        return
    }
    Write-Host "--- $Step" -ForegroundColor Cyan
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $outcome = & $Body
        Add-Result $Step $outcome[0] $outcome[1] $watch.Elapsed.TotalSeconds
    } catch {
        Add-Result $Step 'FAIL' "error: $($_.Exception.Message)" $watch.Elapsed.TotalSeconds
    }
}

function Get-LogTail([string] $Step, [int] $Lines = 15) {
    $log = Join-Path $OutDir "$Step.log"
    if (Test-Path -LiteralPath $log) {
        return (Get-Content -LiteralPath $log -Tail $Lines) -join "`n"
    }
    return ''
}

Write-Host "veriface release check -> $OutDir" -ForegroundColor Cyan
Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR"

# ------------------------------------------------------------------------------------ steps

Invoke-Step 'fmt' {
    $code = Invoke-Logged 'fmt' 'cargo' @('fmt', '--all', '--check')
    if ($code -eq 0) { , @('PASS', 'formatted') } else { , @('FAIL', "cargo fmt --check: see fmt.log`n$(Get-LogTail 'fmt')") }
}

Invoke-Step 'clippy' {
    $code = Invoke-Logged 'clippy' 'cargo' @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
    if ($code -eq 0) { , @('PASS', 'no warnings') } else { , @('FAIL', "see clippy.log`n$(Get-LogTail 'clippy')") }
}

Invoke-Step 'test' {
    $code = Invoke-Logged 'test' 'cargo' @('test', '--workspace', '--locked')
    $log = Get-Content -LiteralPath (Join-Path $OutDir 'test.log')
    $passed = 0
    $failed = 0
    $ignored = 0
    foreach ($m in ($log | Select-String -Pattern 'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored')) {
        $passed += [int] $m.Matches[0].Groups[1].Value
        $failed += [int] $m.Matches[0].Groups[2].Value
        $ignored += [int] $m.Matches[0].Groups[3].Value
    }
    $skips = @($log | Select-String -Pattern '(?i)skipp').Count
    $detail = "$passed passed, $failed failed, $ignored ignored; $skips 'skipping' lines (real-engine tests skip without ONNX Runtime/models)"
    if ($code -eq 0) { , @('PASS', $detail) } else { , @('FAIL', "$detail; see test.log") }
}

Invoke-Step 'licences' {
    $script = Join-Path $PSScriptRoot 'third-party-licenses.ps1'
    $problems = @()
    foreach ($target in @('x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu')) {
        $log = Join-Path $OutDir "licences-$target.log"
        $ErrorActionPreference = 'Continue'
        & $Shell -NoProfile -ExecutionPolicy Bypass -File $script -Target $target `
            -Out (Join-Path $OutDir "THIRD-PARTY-RUST-$target.txt") -Json (Join-Path $OutDir "rust-crates-$target.json") *>&1 |
            ForEach-Object { "$_" } | Out-File -FilePath $log -Encoding utf8
        $code = $LASTEXITCODE
        $ErrorActionPreference = 'Stop'
        if ($code -ne 0) {
            $problems += "${target}: $((Get-Content -LiteralPath $log | Select-String '^\s+- ') -join '; ')"
        }
    }
    if ($problems.Count -eq 0) { , @('PASS', 'every crate allowed by deny.toml (windows + linux targets)') } else { , @('FAIL', "LICENCE POLICY VIOLATION: $($problems -join ' | ')") }
}

Invoke-Step 'deny' {
    if (-not (Test-CargoSubcommand 'deny')) {
        , @('NOT RUN', 'cargo-deny not installed (cargo install --locked cargo-deny)')
        return
    }
    $code = Invoke-Logged 'deny' 'cargo' @('deny', '--locked', 'check', 'licenses', 'bans', 'sources')
    if ($code -eq 0) { , @('PASS', 'licenses, bans, sources') } else { , @('FAIL', "see deny.log`n$(Get-LogTail 'deny')") }
}

Invoke-Step 'advisories' {
    if (Test-CargoSubcommand 'deny') {
        $code = Invoke-Logged 'advisories' 'cargo' @('deny', '--locked', 'check', 'advisories')
        $tool = 'cargo deny check advisories'
    } elseif (Test-CargoSubcommand 'audit') {
        $code = Invoke-Logged 'advisories' 'cargo' @('audit')
        $tool = 'cargo audit'
    } else {
        , @('NOT RUN', 'neither cargo-deny nor cargo-audit installed')
        return
    }
    if ($code -eq 0) { , @('PASS', $tool) } else { , @('FAIL', "$tool reported advisories or could not fetch the database; see advisories.log`n$(Get-LogTail 'advisories')") }
}

Invoke-Step 'sbom' {
    if (-not (Test-CargoSubcommand 'cyclonedx')) {
        , @('NOT RUN', 'cargo-cyclonedx not installed (cargo install --locked cargo-cyclonedx)')
        return
    }
    $code = Invoke-Logged 'sbom' 'cargo' @('cyclonedx', '--format', 'json', '--spec-version', '1.5', '--no-build-deps')
    if ($code -ne 0) {
        , @('FAIL', "see sbom.log`n$(Get-LogTail 'sbom')")
        return
    }
    $boms = @(Get-ChildItem -LiteralPath (Join-Path $ServiceDir 'crates') -Recurse -Filter '*.cdx.json' -File)
    $sbomDir = Join-Path $OutDir 'sbom'
    New-Item -ItemType Directory -Force -Path $sbomDir | Out-Null
    foreach ($bom in $boms) {
        Move-Item -LiteralPath $bom.FullName -Destination (Join-Path $sbomDir $bom.Name) -Force
    }
    , @('PASS', "$($boms.Count) CycloneDX files in sbom/")
}

Invoke-Step 'model-bom' {
    $log = Join-Path $OutDir 'model-bom.log'
    $bomArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $PSScriptRoot 'model-bom.ps1'),
        '-Out', (Join-Path (Join-Path $OutDir 'sbom') 'models.cdx.json'))
    if ($RequireApprovedModels) {
        $bomArgs += '-RequireApproved'
    }
    $ErrorActionPreference = 'Continue'
    & $Shell @bomArgs *>&1 | ForEach-Object { "$_" } | Out-File -FilePath $log -Encoding utf8
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $warnings = @(Get-Content -LiteralPath $log | Select-String -Pattern '(WARNING|ERROR): model pack: (.*)' |
        ForEach-Object { $_.Matches[0].Groups[2].Value })
    if ($code -ne 0) {
        , @('FAIL', "models not approved for production: $($warnings -join '; ')")
    } elseif ($warnings.Count -gt 0) {
        , @('PASS', "BOM written; NOT production-approved: $($warnings.Count) findings (use -RequireApprovedModels for a release): $($warnings -join '; ')")
    } else {
        , @('PASS', 'BOM written; every model approved')
    }
}

Invoke-Step 'parity' {
    if (-not $ParityService) {
        , @('NOT RUN', 'no -ParityService (needs a running service with the fixtures profile; see lab/README.md)')
        return
    }
    $script = Join-Path (Join-Path (Join-Path $RepoDir 'lab') 'scripts') 'release-parity.ps1'
    if (-not (Test-Path -LiteralPath $script)) {
        , @('NOT RUN', "missing $script")
        return
    }
    $log = Join-Path $OutDir 'parity.log'
    $ErrorActionPreference = 'Continue'
    & $Shell -NoProfile -ExecutionPolicy Bypass -File $script -Service $ParityService -ApiKeyEnv $ParityKeyEnv `
        -Profile $ParityProfile -Out (Join-Path $OutDir 'parity') *>&1 | ForEach-Object { "$_" } |
        Out-File -FilePath $log -Encoding utf8
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    switch ($code) {
        0 { , @('PASS', 'lab parity gate passed') }
        3 { , @('NOT RUN', "parity prerequisites missing: $(Get-LogTail 'parity' 3)") }
        default { , @('FAIL', "parity gate failed (exit $code); see parity/ and parity.log") }
    }
}

# ---------------------------------------------------------------------------------- summary

$failed = @($results | Where-Object { $_.status -eq 'FAIL' })
$notRun = @($results | Where-Object { $_.status -eq 'NOT RUN' })
$gate = 'PASS'
if ($failed.Count -gt 0 -or ($Strict -and $notRun.Count -gt 0)) {
    $gate = 'FAIL'
}
$summary = [ordered]@{
    gate = $gate
    strict = [bool] $Strict
    requireApprovedModels = [bool] $RequireApprovedModels
    at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    host = [Environment]::MachineName
    steps = $results
}
[IO.File]::WriteAllText((Join-Path $OutDir 'summary.json'), ($summary | ConvertTo-Json -Depth 4), (New-Object Text.UTF8Encoding $false))

Write-Host ''
$results | Format-Table step, status, seconds -AutoSize | Out-String | Write-Host
$color = 'Green'
if ($gate -eq 'FAIL') {
    $color = 'Red'
}
Write-Host "RELEASE GATE: $gate ($($failed.Count) failed, $($notRun.Count) not run$(if ($Strict) { ', strict' }))  -> $OutDir" -ForegroundColor $color
if ($gate -eq 'FAIL') {
    exit 1
}
