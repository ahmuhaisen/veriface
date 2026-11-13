<#
.SYNOPSIS
    Writes a CycloneDX 1.6 bill of materials for the model pack (models/manifest.json).

.DESCRIPTION
    One component per model (type `machine-learning-model`) and per asset (type `data`), with the
    SHA-256, the upstream source and its SHA-256, the licence, the training-data statement, the
    conversion and the legal review status from the manifest. No extra tools needed.

    With -RequireApproved the script fails (exit 1) when any model's review status is not
    `approved`, or its training data does not allow commercial use, so a production release
    cannot ship an unreviewed model by accident. Without it, those are warnings.

    Works with Windows PowerShell 5.1 and PowerShell 7.

.PARAMETER Manifest
    Path to manifest.json. Default: models/manifest.json in the repository.

.PARAMETER Out
    Output path. Default: print to stdout.

.PARAMETER Version
    The veriface release the pack ships with (metadata.component.version).

.PARAMETER RequireApproved
    Fail unless every model is legally approved for commercial use.
#>
[CmdletBinding()]
param(
    [string] $Manifest,
    [string] $Out,
    [string] $Version = '0.0.0',
    [switch] $RequireApproved
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepoDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Manifest) {
    $Manifest = Join-Path (Join-Path $RepoDir 'models') 'manifest.json'
}
$manifestRaw = Get-Content -Raw -LiteralPath $Manifest
$m = $manifestRaw | ConvertFrom-Json
$manifestSha = (Get-FileHash -Algorithm SHA256 -LiteralPath $Manifest).Hash.ToLowerInvariant()

function Get-Prop($Object, [string] $Name) {
    if ($null -ne $Object -and $Object.PSObject.Properties.Name -contains $Name) {
        return $Object.$Name
    }
    return $null
}

$problems = New-Object System.Collections.Generic.List[string]
$components = New-Object System.Collections.Generic.List[object]

$entries = @()
foreach ($model in @($m.models)) {
    $entries += , @('machine-learning-model', $model)
}
if ($m.PSObject.Properties.Name -contains 'assets') {
    foreach ($asset in @($m.assets)) {
        $entries += , @('data', $asset)
    }
}

foreach ($pair in $entries) {
    $type = $pair[0]
    $e = $pair[1]
    $licence = Get-Prop $e 'licence'
    $upstream = Get-Prop $e 'upstream'
    $review = Get-Prop $e 'review'
    $training = Get-Prop $licence 'trainingData'
    $conversion = Get-Prop $e 'conversion'

    $properties = New-Object System.Collections.Generic.List[object]
    $add = {
        param($name, $value)
        if ($null -ne $value -and "$value" -ne '') {
            $properties.Add([ordered]@{ name = $name; value = "$value" })
        }
    }
    & $add 'veriface:role' (Get-Prop $e 'role')
    & $add 'veriface:file' (Get-Prop $e 'file')
    & $add 'veriface:bytes' (Get-Prop $e 'bytes')
    & $add 'veriface:format' (Get-Prop $e 'format')
    & $add 'veriface:precision' (Get-Prop $e 'precision')
    & $add 'veriface:review.status' (Get-Prop $review 'status')
    & $add 'veriface:review.notes' (Get-Prop $review 'notes')
    & $add 'veriface:trainingData.commercialUse' (Get-Prop $training 'commercialUse')
    & $add 'veriface:trainingData.statement' (Get-Prop $training 'statement')
    & $add 'veriface:trainingData.source' (Get-Prop $training 'source')
    & $add 'veriface:upstream.sha256' (Get-Prop $upstream 'sha256')
    & $add 'veriface:conversion.tool' (Get-Prop $conversion 'toolVersion')
    & $add 'veriface:conversion.fidelity' (Get-Prop $conversion 'fidelity')

    $component = [ordered]@{
        type = $type
        'bom-ref' = "model:$($e.id)"
        name = $e.id
        version = "$(Get-Prop $upstream 'version')"
        description = "$(Get-Prop $upstream 'name')"
        hashes = @([ordered]@{ alg = 'SHA-256'; content = $e.sha256 })
        properties = $properties
    }
    $spdx = Get-Prop $licence 'spdx'
    if ($spdx) {
        $component.licenses = @([ordered]@{ license = [ordered]@{ id = $spdx; url = "$(Get-Prop $licence 'url')" } })
    }
    $url = Get-Prop $upstream 'url'
    if ($url) {
        $component.externalReferences = @([ordered]@{ type = 'distribution'; url = $url })
    }
    $components.Add($component)

    if ($type -eq 'machine-learning-model') {
        $status = Get-Prop $review 'status'
        if ($status -ne 'approved') {
            $problems.Add("$($e.id): legal review status is '$status', not 'approved'")
        }
        $commercial = Get-Prop $training 'commercialUse'
        if ($commercial -ne 'allowed') {
            $problems.Add("$($e.id): training data commercialUse is '$commercial', not 'allowed'")
        }
    }
}

$bom = [ordered]@{
    bomFormat = 'CycloneDX'
    specVersion = '1.6'
    serialNumber = "urn:uuid:$([guid]::NewGuid())"
    version = 1
    metadata = [ordered]@{
        timestamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        tools = [ordered]@{ components = @([ordered]@{ type = 'application'; name = 'veriface model-bom.ps1' }) }
        component = [ordered]@{
            type = 'data'
            'bom-ref' = 'veriface-model-pack'
            name = 'veriface-model-pack'
            version = $Version
            hashes = @([ordered]@{ alg = 'SHA-256'; content = $manifestSha })
            properties = @(
                [ordered]@{ name = 'veriface:manifest.generatedAt'; value = "$(Get-Prop $m 'generatedAt')" },
                [ordered]@{ name = 'veriface:manifest.generator'; value = "$(Get-Prop $m 'generator')" }
            )
        }
    }
    components = $components
}
$json = $bom | ConvertTo-Json -Depth 10

if ($Out) {
    $outPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outPath) | Out-Null
    [IO.File]::WriteAllText($outPath, $json, (New-Object Text.UTF8Encoding $false))
    Write-Host "Model BOM: $outPath ($($components.Count) components)"
} else {
    $json
}

if ($problems.Count -gt 0) {
    $level = 'WARNING'
    if ($RequireApproved) {
        $level = 'ERROR'
    }
    foreach ($p in $problems) {
        Write-Host "${level}: model pack: $p" -ForegroundColor Yellow
    }
    if ($RequireApproved) {
        exit 1
    }
}
