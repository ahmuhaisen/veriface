<#
.SYNOPSIS
    Lists the Rust crates compiled into veriface, checks their licences against the policy and
    writes their licence texts (THIRD-PARTY-RUST.txt) for the release bundle.

.DESCRIPTION
    Uses `cargo metadata` (no extra tools). Starting from the face-server package, it follows the
    normal (non-dev, non-build) dependency edges for the target platform, then for every
    third-party crate:

      - checks its SPDX licence expression against the allow-list in service/deny.toml
        (`[licenses] allow`). `OR` needs one allowed alternative, `AND` needs all, and a
        `WITH` exception counts as its own identifier (e.g. `Apache-2.0 WITH LLVM-exception`).
        A crate without a licence expression is a violation;
      - copies its licence files (LICENSE*, LICENCE*, COPYING*, NOTICE*) into the output.

    The script exits with 1 on any licence violation, so the release gate fails. It is a policy
    gate that needs no extra tools; `cargo deny check licenses` (when installed) is the stricter
    second opinion, run by release-check.

    Works with Windows PowerShell 5.1 and PowerShell 7 (also on Linux with pwsh).

.PARAMETER Out
    Path of the licence-text file to write. Default: no file, only the check.

.PARAMETER Json
    Optional path of a JSON summary (name, version, licence, licence files, allowed).

.PARAMETER Policy
    The cargo-deny configuration whose `[licenses] allow` list is the policy. Default:
    service/deny.toml.

.PARAMETER Target
    Rust target triple to resolve platform-specific dependencies for. Default: the host
    (x86_64-pc-windows-msvc on Windows, x86_64-unknown-linux-gnu elsewhere).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File service/scripts/third-party-licenses.ps1 -Out dist/THIRD-PARTY-RUST.txt
#>
[CmdletBinding()]
param(
    [string] $Out,
    [string] $Json,
    [string] $Target,
    [string] $Policy
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ServiceDir = Split-Path -Parent $PSScriptRoot
$OnWindows = [IO.Path]::DirectorySeparatorChar -eq '\'
$Utf8NoBom = New-Object Text.UTF8Encoding $false
if (-not $Target) {
    $Target = 'x86_64-unknown-linux-gnu'
    if ($OnWindows) {
        $Target = 'x86_64-pc-windows-msvc'
    }
}

# --------------------------------------------------------------------------- licence policy

if (-not $Policy) {
    $Policy = Join-Path $ServiceDir 'deny.toml'
}

function Get-AllowedLicenses {
    $text = Get-Content -Raw -LiteralPath $Policy
    $match = [regex]::Match($text, '(?ms)^\[licenses\].*?^allow\s*=\s*\[(.*?)\]')
    if (-not $match.Success) {
        throw 'service/deny.toml has no [licenses] allow = [...] list'
    }
    $allowed = @{}
    foreach ($m in [regex]::Matches($match.Groups[1].Value, '"([^"]+)"')) {
        $allowed[$m.Groups[1].Value] = $true
    }
    return $allowed
}

# Tokens of an SPDX expression: identifiers (an `X WITH Y` pair is one token), AND, OR, ( and ).
function Split-Spdx([string] $Expression) {
    $raw = @(($Expression -replace '\(', ' ( ' -replace '\)', ' ) ' -replace '/', ' OR ').Trim() -split '\s+' |
        Where-Object { $_ })
    $tokens = New-Object System.Collections.Generic.List[string]
    for ($i = 0; $i -lt $raw.Count; $i++) {
        if ($raw[$i] -eq 'WITH' -and $tokens.Count -gt 0 -and ($i + 1) -lt $raw.Count) {
            $tokens[$tokens.Count - 1] = $tokens[$tokens.Count - 1] + ' WITH ' + $raw[$i + 1]
            $i++
        } else {
            $tokens.Add($raw[$i])
        }
    }
    return , $tokens
}

# Recursive descent: or := and ('OR' and)*; and := atom ('AND' atom)*; atom := id | '(' or ')'.
function Test-Spdx([string] $Expression, [hashtable] $Allowed) {
    $script:spdxTokens = Split-Spdx $Expression
    $script:spdxPos = 0
    $result = Test-SpdxOr $Allowed
    if ($script:spdxPos -ne $script:spdxTokens.Count) {
        throw "cannot parse licence expression '$Expression'"
    }
    return $result
}

function Test-SpdxOr([hashtable] $Allowed) {
    $ok = Test-SpdxAnd $Allowed
    while ($script:spdxPos -lt $script:spdxTokens.Count -and $script:spdxTokens[$script:spdxPos] -eq 'OR') {
        $script:spdxPos++
        $next = Test-SpdxAnd $Allowed
        $ok = $ok -or $next
    }
    return $ok
}

function Test-SpdxAnd([hashtable] $Allowed) {
    $ok = Test-SpdxAtom $Allowed
    while ($script:spdxPos -lt $script:spdxTokens.Count -and $script:spdxTokens[$script:spdxPos] -eq 'AND') {
        $script:spdxPos++
        $next = Test-SpdxAtom $Allowed
        $ok = $ok -and $next
    }
    return $ok
}

function Test-SpdxAtom([hashtable] $Allowed) {
    if ($script:spdxPos -ge $script:spdxTokens.Count) {
        throw 'licence expression ends early'
    }
    $token = $script:spdxTokens[$script:spdxPos]
    $script:spdxPos++
    if ($token -eq '(') {
        $ok = Test-SpdxOr $Allowed
        if ($script:spdxPos -ge $script:spdxTokens.Count -or $script:spdxTokens[$script:spdxPos] -ne ')') {
            throw 'unbalanced parentheses in licence expression'
        }
        $script:spdxPos++
        return $ok
    }
    return $Allowed.ContainsKey($token.TrimEnd('+'))
}

# ------------------------------------------------------------------------- dependency graph

Push-Location $ServiceDir
try {
    $metadataJson = & cargo metadata --format-version 1 --locked --filter-platform $Target
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
$metadata = ($metadataJson -join "`n") | ConvertFrom-Json

$packages = @{}
foreach ($p in $metadata.packages) {
    $packages[$p.id] = $p
}
$nodes = @{}
foreach ($n in $metadata.resolve.nodes) {
    $nodes[$n.id] = $n
}
$root = @($metadata.packages | Where-Object { $_.name -eq 'face-server' -and $null -eq $_.source })
if ($root.Count -ne 1) {
    throw 'face-server package not found in cargo metadata'
}

$seen = @{}
$queue = New-Object System.Collections.Generic.Queue[string]
$queue.Enqueue($root[0].id)
while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    if ($seen.ContainsKey($id)) {
        continue
    }
    $seen[$id] = $true
    foreach ($dep in $nodes[$id].deps) {
        # Normal edges only: `kind` is null for normal dependencies.
        $normal = @($dep.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0
        if ($normal) {
            $queue.Enqueue($dep.pkg)
        }
    }
}

# --------------------------------------------------------------------------- check + texts

$allowed = Get-AllowedLicenses
$violations = New-Object System.Collections.Generic.List[string]
$summary = New-Object System.Collections.Generic.List[object]
$text = New-Object System.Text.StringBuilder
[void] $text.AppendLine('Third-party Rust crates compiled into veriface')
[void] $text.AppendLine("Target: $Target. Generated by service/scripts/third-party-licenses.ps1 from Cargo.lock.")
[void] $text.AppendLine('A crate with several licence files has one "--- <file> ---" sub-section per file.')
[void] $text.AppendLine('')

$thirdParty = @($seen.Keys | ForEach-Object { $packages[$_] } | Where-Object { $null -ne $_.source } |
    Sort-Object name, version)
foreach ($p in $thirdParty) {
    $licence = $p.license
    $ok = $false
    if ($licence) {
        try {
            $ok = Test-Spdx $licence $allowed
        } catch {
            $violations.Add("$($p.name) $($p.version): $($_.Exception.Message)")
            continue
        }
        if (-not $ok) {
            $violations.Add("$($p.name) $($p.version): '$licence' is not allowed by service/deny.toml")
        }
    } else {
        $violations.Add("$($p.name) $($p.version): no SPDX licence expression (license-file: $($p.license_file))")
    }

    $dir = Split-Path -Parent $p.manifest_path
    $files = @(Get-ChildItem -LiteralPath $dir -File |
        Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)' } |
        Sort-Object Name)
    $summary.Add([pscustomobject]@{
        name = $p.name
        version = $p.version
        license = $licence
        allowed = $ok
        licenseFiles = @($files | ForEach-Object { $_.Name })
    })

    # Same section format as the model pack's THIRD-PARTY-LICENSES.txt, so the files concatenate.
    $source = $p.repository
    if (-not $source) {
        $source = "https://crates.io/crates/$($p.name)/$($p.version)"
    }
    [void] $text.AppendLine(('=' * 80))
    [void] $text.AppendLine("$($p.name) $($p.version)")
    [void] $text.AppendLine("Source:  $source")
    [void] $text.AppendLine("License: $licence")
    [void] $text.AppendLine(('-' * 80))
    if ($files.Count -eq 0) {
        [void] $text.AppendLine("(The crate ships no licence file; its licence is $licence. The standard texts are in licenses/.)")
    }
    foreach ($f in $files) {
        [void] $text.AppendLine("--- $($f.Name) ---")
        [void] $text.AppendLine((Get-Content -Raw -LiteralPath $f.FullName).TrimEnd())
        [void] $text.AppendLine('')
    }
    [void] $text.AppendLine('')
}

if ($Out) {
    $outPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outPath) | Out-Null
    [IO.File]::WriteAllText($outPath, $text.ToString(), $Utf8NoBom)
}
if ($Json) {
    $jsonPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Json)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $jsonPath) | Out-Null
    [IO.File]::WriteAllText($jsonPath, ($summary | ConvertTo-Json -Depth 4), $Utf8NoBom)
}

Write-Host "Rust crates compiled in ($Target): $($thirdParty.Count) third-party."
if ($violations.Count -gt 0) {
    Write-Host "LICENCE POLICY VIOLATIONS ($($violations.Count)):" -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
Write-Host 'Licence policy: OK (every crate allowed by service/deny.toml).'
