<#
.SYNOPSIS
    Installs, upgrades, rolls back and removes the Veriface Windows service.

.DESCRIPTION
    Layout (side by side, so an upgrade can be rolled back):

      <InstallRoot>  default C:\Program Files\Veriface
        releases\<version>\      one unpacked release bundle per version, never modified
        current  ->  releases\<version>     a directory junction; the service runs from here
      <DataRoot>     default C:\ProgramData\Veriface
        config\config.toml       written on the first install from install\config.windows.toml,
                                 never overwritten
        config\keys.toml         the API key hashes (api_keys_file)
        logs\                    the service log (writable by the service account only)
        state.json               current and previous version

    Actions:
      Verify     Check a bundle (SHA256SUMS, optional Authenticode) and run it once on a free
                 loopback port with the installed (or a temporary) configuration. Changes nothing.
      Install    First installation: unpack, write config + first API key, register the service
                 (virtual account NT SERVICE\<name>, delayed auto start, recovery actions), set
                 ACLs and the firewall rule, start, wait until /health/ready answers 200.
      Upgrade    Verify the new bundle, preflight it on a spare port while the old version keeps
                 serving, then stop, flip `current`, start and wait for ready. If the new version
                 does not become ready, flip back and start the previous version automatically.
      Rollback   Flip `current` back to the previous (or -Version) release and restart.
      Uninstall  Stop and remove the service, the firewall rule and the releases. The data
                 directory (config, keys, logs) is kept unless -RemoveData.
      Status     Service state, versions, health.
      NewKey     Generate an API key, print it once, append its SHA-256 to keys.toml.

    Every action except Status and Verify needs an elevated (Administrator) prompt.
    Works with Windows PowerShell 5.1 (Windows Server 2019, 2022, 2025) and PowerShell 7.
    Supports -WhatIf and -Confirm for every change.

.PARAMETER Action
    Verify | Install | Upgrade | Rollback | Uninstall | Status | NewKey.

.PARAMETER Bundle
    The release: veriface-<version>-win-x64.zip (checked against <zip>.sha256 when present) or
    its unpacked folder. Default for Install/Upgrade/Verify: the bundle this script is part of.

.PARAMETER ServiceName
    Service name (default veriface). Use another name, InstallRoot, DataRoot and Bind for a
    second instance, e.g. a test instance on port 18080.

.PARAMETER Bind
    Listen address written into a new config.toml (Install only). Default 0.0.0.0:8080.

.PARAMETER Account
    Service account. Default: the virtual account NT SERVICE\<ServiceName>. Also possible:
    "NT AUTHORITY\LocalService", or a gMSA "DOMAIN\name$".

.PARAMETER FirewallRemoteAddress
    Remote addresses allowed by the inbound firewall rule (default: Any). E.g. the integrating
    backend's servers: 10.0.1.10,10.0.1.11 or 10.0.1.0/24.

.PARAMETER NoFirewall
    Do not create the firewall rule (e.g. behind a local reverse proxy).

.PARAMETER Project
    Project name of the API key created by Install (default "default") or NewKey (required).

.PARAMETER Profiles
    Profiles the new key may use (default: all, "*").

.PARAMETER Version
    Rollback target (default: the previous version in state.json).

.PARAMETER HealthKey
    An API key to send with the health probe when health_requires_key = true.

.PARAMETER RequireSignature
    Refuse a bundle whose veriface.exe has no valid Authenticode signature.

.PARAMETER TrustedSignerThumbprint
    Also require the signing certificate to have this thumbprint.

.PARAMETER ReadyTimeoutSec
    How long to wait for /health/ready after a start (default 180).

.PARAMETER KeepReleases
    Releases kept after an upgrade, including current and previous (default 3).

.PARAMETER RemoveData
    Uninstall: also delete the data directory (configuration, keys and logs).

.PARAMETER Force
    Upgrade: allow re-installing the same version or a lower one.

.EXAMPLE
    .\Install-Veriface.ps1 -Action Install -Bundle .\veriface-1.0.0-win-x64.zip -FirewallRemoteAddress 10.0.1.0/24

.EXAMPLE
    .\Install-Veriface.ps1 -Action Upgrade -Bundle .\veriface-1.1.0-win-x64.zip

.EXAMPLE
    .\Install-Veriface.ps1 -Action Install -ServiceName veriface-test -Bind 127.0.0.1:18080 -InstallRoot C:\veriface-test -DataRoot C:\veriface-test-data -NoFirewall
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Verify', 'Install', 'Upgrade', 'Rollback', 'Uninstall', 'Status', 'NewKey')]
    [string] $Action,
    [string] $Bundle,
    [ValidatePattern('^[A-Za-z0-9._-]{1,64}$')]
    [string] $ServiceName = 'veriface',
    [string] $InstallRoot = (Join-Path $env:ProgramFiles 'Veriface'),
    [string] $DataRoot = (Join-Path $env:ProgramData 'Veriface'),
    [string] $Bind = '0.0.0.0:8080',
    [string] $Account,
    [string[]] $FirewallRemoteAddress = @('Any'),
    [switch] $NoFirewall,
    [string] $Project,
    [string[]] $Profiles = @('*'),
    [string] $Version,
    [string] $HealthKey,
    [switch] $RequireSignature,
    [string] $TrustedSignerThumbprint,
    [int] $ReadyTimeoutSec = 180,
    [ValidateRange(2, 50)]
    [int] $KeepReleases = 3,
    [switch] $RemoveData,
    [switch] $Force
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ExeName = 'veriface.exe'
$ReleasesDir = Join-Path $InstallRoot 'releases'
$CurrentLink = Join-Path $InstallRoot 'current'
$ConfigDir = Join-Path $DataRoot 'config'
$ConfigPath = Join-Path $ConfigDir 'config.toml'
$KeysPath = Join-Path $ConfigDir 'keys.toml'
$LogDir = Join-Path $DataRoot 'logs'
$StatePath = Join-Path $DataRoot 'state.json'
$FirewallRuleName = "Veriface-$ServiceName"
if (-not $Account) {
    $Account = "NT SERVICE\$ServiceName"
}
$Utf8NoBom = New-Object Text.UTF8Encoding $false

# ------------------------------------------------------------------------------------ helpers

function Write-Step([string] $Message) {
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Assert-Admin {
    $principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "Action $Action needs an elevated (Run as Administrator) PowerShell."
    }
}

function Invoke-Exe([string] $What, [string] $Exe, [string[]] $Arguments) {
    # Windows PowerShell 5.1 turns a native command's stderr into terminating errors under
    # 'Stop'; the exit code is what counts here.
    $ErrorActionPreference = 'Continue'
    $output = & $Exe @Arguments 2>&1 | ForEach-Object { "$_" }
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        throw "$What failed (exit code $code):`n$($output -join "`n")"
    }
    return $output
}

function Get-Sha256([string] $Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Write-Utf8([string] $Path, [string] $Text) {
    [IO.File]::WriteAllText($Path, $Text, $Utf8NoBom)
}

function Get-State {
    if (Test-Path -LiteralPath $StatePath) {
        return Get-Content -Raw -LiteralPath $StatePath | ConvertFrom-Json
    }
    return [pscustomobject]@{ current = $null; previous = $null; history = @() }
}

function Save-State([string] $Current, [string] $Previous, [string] $Event) {
    $state = Get-State
    $history = @($state.history) + @([pscustomobject]@{
        at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        event = $Event
        current = $Current
        previous = $Previous
        by = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    })
    $new = [ordered]@{ current = $Current; previous = $Previous; history = @($history | Select-Object -Last 50) }
    if ($PSCmdlet.ShouldProcess($StatePath, 'record state')) {
        Write-Utf8 $StatePath ($new | ConvertTo-Json -Depth 4)
    }
}

function Get-CurrentVersion {
    if (-not (Test-Path -LiteralPath $CurrentLink)) {
        return $null
    }
    $item = Get-Item -LiteralPath $CurrentLink -Force
    $target = @($item.Target) | Select-Object -First 1
    if (-not $target) {
        return $null
    }
    return Split-Path -Leaf $target
}

function Get-ReleaseDir([string] $Ver) {
    return Join-Path $ReleasesDir $Ver
}

# Points `current` at a release. Removes only the junction itself, never the target's contents.
function Set-Current([string] $Ver) {
    $target = Get-ReleaseDir $Ver
    if (-not (Test-Path -LiteralPath (Join-Path $target $ExeName))) {
        throw "release $Ver is not installed ($target)"
    }
    if ($PSCmdlet.ShouldProcess($CurrentLink, "point at releases\$Ver")) {
        if (Test-Path -LiteralPath $CurrentLink) {
            $item = Get-Item -LiteralPath $CurrentLink -Force
            if (-not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
                throw "$CurrentLink exists and is not a junction; refusing to replace it"
            }
            [IO.Directory]::Delete($CurrentLink, $false)
        }
        New-Item -ItemType Junction -Path $CurrentLink -Target $target | Out-Null
    }
}

function Get-Service2 {
    return Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
}

function Get-ConfigBind([string] $Path) {
    $text = Get-Content -Raw -LiteralPath $Path
    $m = [regex]::Match($text, '(?m)^\s*bind\s*=\s*["'']([^"'']+)["'']')
    if ($m.Success) {
        return $m.Groups[1].Value
    }
    return '127.0.0.1:8080'
}

# The URL a local probe uses for an `ip:port` bind address (unspecified -> loopback).
function Get-ProbeUrl([string] $BindAddress) {
    $m = [regex]::Match($BindAddress, '^(\[(?<ip6>[^\]]+)\]|(?<ip4>[^:]+)):(?<port>\d+)$')
    if (-not $m.Success) {
        throw "cannot read the bind address '$BindAddress'"
    }
    $port = $m.Groups['port'].Value
    if ($m.Groups['ip6'].Success) {
        $ip = $m.Groups['ip6'].Value
        if ($ip -eq '::') {
            $ip = '::1'
        }
        return "http://[$ip]:$port/health/ready"
    }
    $ip = $m.Groups['ip4'].Value
    if ($ip -eq '0.0.0.0') {
        $ip = '127.0.0.1'
    }
    return "http://${ip}:$port/health/ready"
}

function Test-Loopback([string] $BindAddress) {
    return $BindAddress -match '^(127\.|\[::1\]:|localhost:)'
}

function Get-BindPort([string] $BindAddress) {
    return [int] ($BindAddress -replace '^.*:', '')
}

# Polls `veriface healthcheck --url` until it succeeds; $true when ready in time.
function Wait-Ready([string] $Exe, [string] $Url, [int] $TimeoutSec, [Diagnostics.Process] $Process) {
    $ErrorActionPreference = 'Continue'
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $previousKey = $env:VERIFACE_HEALTHCHECK_KEY
    if ($HealthKey) {
        $env:VERIFACE_HEALTHCHECK_KEY = $HealthKey
    }
    try {
        while ((Get-Date) -lt $deadline) {
            if ($null -ne $Process -and $Process.HasExited) {
                return $false
            }
            & $Exe healthcheck --url $Url --timeout-ms 2000 2>&1 | Out-Null
            if ($LASTEXITCODE -eq 0) {
                return $true
            }
            Start-Sleep -Seconds 1
        }
        return $false
    } finally {
        $env:VERIFACE_HEALTHCHECK_KEY = $previousKey
    }
}

function Get-FreeLoopbackPort {
    $listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    $port = $listener.LocalEndpoint.Port
    $listener.Stop()
    return $port
}

function New-ApiKey {
    $bytes = New-Object byte[] 32
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $rng.GetBytes($bytes)
    $rng.Dispose()
    $key = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
    $sha = [Security.Cryptography.SHA256]::Create()
    $digest = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($key))
    $sha.Dispose()
    $hex = ($digest | ForEach-Object { $_.ToString('x2') }) -join ''
    return [pscustomobject]@{ Key = $key; Sha256 = $hex }
}

function Format-KeyEntry([string] $ProjectName, [string] $Sha256, [string[]] $ProfileNames) {
    if ($ProjectName -notmatch '^[A-Za-z0-9._ -]{1,64}$') {
        throw "project name '$ProjectName' must be 1-64 characters from letters, digits, space, '.', '_' and '-'"
    }
    $list = ($ProfileNames | ForEach-Object { '"' + ($_ -replace '"', '') + '"' }) -join ', '
    $created = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
    return "`n# created $created by $([Security.Principal.WindowsIdentity]::GetCurrent().Name)`n[[api_keys]]`nproject = `"$ProjectName`"`nsha256 = `"$Sha256`"`nprofiles = [$list]`n"
}

# ------------------------------------------------------------------------------ the bundle

# Returns the unpacked bundle folder (a temporary copy for a zip) and its BUILDINFO.
function Open-Bundle([string] $Path) {
    if (-not $Path) {
        # The bundle this script ships in: <bundle>\install\Install-Veriface.ps1
        $Path = Split-Path -Parent $PSScriptRoot
    }
    $Path = (Resolve-Path -LiteralPath $Path).ProviderPath
    $dir = $Path
    $temp = $null
    if ((Get-Item -LiteralPath $Path).PSIsContainer -eq $false) {
        if ($Path -notmatch '\.zip$') {
            throw "bundle '$Path' must be a .zip or a folder"
        }
        $sidecar = "$Path.sha256"
        if (Test-Path -LiteralPath $sidecar) {
            $expected = ((Get-Content -LiteralPath $sidecar -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
            $actual = Get-Sha256 $Path
            if ($expected -ne $actual) {
                throw "archive SHA-256 mismatch: $sidecar says $expected, the file is $actual"
            }
            Write-Host "    archive SHA-256 matches $([IO.Path]::GetFileName($sidecar))"
        } else {
            Write-Warning "no $([IO.Path]::GetFileName($sidecar)) next to the archive: its own hash is not checked (SHA256SUMS inside still is)"
        }
        $temp = Join-Path ([IO.Path]::GetTempPath()) ("veriface-bundle-" + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $temp | Out-Null
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [IO.Compression.ZipFile]::ExtractToDirectory($Path, $temp)
        $top = @(Get-ChildItem -LiteralPath $temp -Directory)
        if ($top.Count -eq 1 -and (Test-Path -LiteralPath (Join-Path $top[0].FullName 'BUILDINFO.json'))) {
            $dir = $top[0].FullName
        } else {
            $dir = $temp
        }
    }
    $infoPath = Join-Path $dir 'BUILDINFO.json'
    if (-not (Test-Path -LiteralPath $infoPath)) {
        throw "'$dir' is not a veriface release bundle (no BUILDINFO.json)"
    }
    $info = Get-Content -Raw -LiteralPath $infoPath | ConvertFrom-Json
    if ($info.platform -ne 'win-x64') {
        throw "bundle platform is '$($info.platform)', expected win-x64"
    }
    return [pscustomobject]@{ Dir = $dir; Temp = $temp; Info = $info }
}

function Close-Bundle($Opened) {
    if ($Opened -and $Opened.Temp -and (Test-Path -LiteralPath $Opened.Temp)) {
        Remove-Item -LiteralPath $Opened.Temp -Recurse -Force
    }
}

function Test-BundleIntegrity([string] $Dir) {
    $sums = Join-Path $Dir 'SHA256SUMS'
    if (-not (Test-Path -LiteralPath $sums)) {
        throw "no SHA256SUMS in $Dir"
    }
    $listed = @{}
    $bad = New-Object System.Collections.Generic.List[string]
    foreach ($line in Get-Content -LiteralPath $sums) {
        if (-not $line.Trim()) {
            continue
        }
        $hash, $file = $line -split '  ', 2
        $listed[$file] = $true
        $full = Join-Path $Dir ($file -replace '/', '\')
        if (-not (Test-Path -LiteralPath $full)) {
            $bad.Add("missing: $file")
        } elseif ((Get-Sha256 $full) -ne $hash.ToLowerInvariant()) {
            $bad.Add("changed: $file")
        }
    }
    if ($bad.Count -gt 0) {
        throw "bundle integrity check failed:`n  $($bad -join "`n  ")"
    }
    $exe = Join-Path $Dir $ExeName
    if (-not $listed.ContainsKey($ExeName)) {
        throw "SHA256SUMS does not list $ExeName"
    }
    $models = @(Get-ChildItem -LiteralPath (Join-Path $Dir 'models') -Filter '*.onnx' -File -ErrorAction SilentlyContinue)
    if ($models.Count -eq 0) {
        throw "the bundle has no models (models\*.onnx): use a full bundle (package.ps1 -IncludeModels)"
    }
    Write-Host "    SHA256SUMS: $($listed.Count) files verified"

    $signature = Get-AuthenticodeSignature -LiteralPath $exe
    if ($signature.Status -eq 'Valid') {
        Write-Host "    Authenticode: valid, signed by $($signature.SignerCertificate.Subject)"
        if ($TrustedSignerThumbprint -and $signature.SignerCertificate.Thumbprint -ne $TrustedSignerThumbprint.ToUpperInvariant()) {
            throw "signed by $($signature.SignerCertificate.Thumbprint), expected $TrustedSignerThumbprint"
        }
    } elseif ($RequireSignature -or $TrustedSignerThumbprint) {
        throw "$ExeName has no valid Authenticode signature ($($signature.Status)); -RequireSignature refuses it"
    } else {
        Write-Warning "$ExeName is not signed ($($signature.Status)). Production bundles should be signed: see install\SIGNING.md"
    }
}

# Runs a release once against the installed configuration, on a free loopback port, with paths
# pointed at that release. Proves the binary, ONNX Runtime, the models, the profiles and the
# configuration together, and the warm-up, without touching the running service.
function Invoke-Preflight([string] $ReleaseDir, [string] $Config) {
    $exe = Join-Path $ReleaseDir $ExeName
    $port = Get-FreeLoopbackPort
    $logBase = Join-Path ([IO.Path]::GetTempPath()) ("veriface-preflight-" + [guid]::NewGuid().ToString('N'))
    $overrides = [ordered]@{
        VERIFACE__PATHS__MODELS = (Join-Path $ReleaseDir 'models')
        VERIFACE__PATHS__PROFILES = (Join-Path $ReleaseDir 'profiles')
        VERIFACE__PATHS__ORT_LIBRARY = (Join-Path $ReleaseDir 'onnxruntime.dll')
        VERIFACE__SERVER__BIND = "127.0.0.1:$port"
        VERIFACE__SERVER__MAX_CONCURRENCY = '1'
        VERIFACE__ENGINE__SESSIONS_PER_MODEL = '1'
        VERIFACE__HEALTH_REQUIRES_KEY = 'false'
        # Never the service's own log file (an empty value would unset the variable instead).
        VERIFACE__LOGGING__FILE = "$logBase.service.log"
    }
    $saved = @{}
    foreach ($name in $overrides.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $overrides[$name], 'Process')
    }
    $process = $null
    try {
        Write-Host "    check: $exe check"
        Invoke-Exe 'veriface check' $exe @('check', '--config', $Config) | ForEach-Object { Write-Host "    $_" }
        Write-Host "    trial run on 127.0.0.1:$port ..."
        $process = Start-Process -FilePath $exe -ArgumentList @('run', '--config', "`"$Config`"") `
            -RedirectStandardOutput "$logBase.out.log" -RedirectStandardError "$logBase.err.log" `
            -PassThru -WindowStyle Hidden
        if (-not (Wait-Ready $exe "http://127.0.0.1:$port/health/ready" $ReadyTimeoutSec $process)) {
            $tail = @()
            foreach ($log in @("$logBase.err.log", "$logBase.out.log", "$logBase.service.log")) {
                if (Test-Path -LiteralPath $log) {
                    $tail += @(Get-Content -LiteralPath $log -Tail 20)
                }
            }
            throw "preflight: the release did not become ready within $ReadyTimeoutSec s:`n$($tail -join "`n")"
        }
        Write-Host '    trial run: ready (warm-up passed)'
    } finally {
        if ($null -ne $process -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
            $process.WaitForExit(10000) | Out-Null
        }
        foreach ($name in $overrides.Keys) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process')
        }
        Remove-Item -LiteralPath "$logBase.out.log", "$logBase.err.log", "$logBase.service.log" -Force -ErrorAction SilentlyContinue
    }
}

# Copies an opened bundle to releases\<version> (or reuses an identical existing copy).
function Install-Release($Opened) {
    $ver = $Opened.Info.version
    $target = Get-ReleaseDir $ver
    if (Test-Path -LiteralPath $target) {
        $sameExe = (Get-Sha256 (Join-Path $target $ExeName)) -eq (Get-Sha256 (Join-Path $Opened.Dir $ExeName))
        $sameSums = (Get-Sha256 (Join-Path $target 'SHA256SUMS')) -eq (Get-Sha256 (Join-Path $Opened.Dir 'SHA256SUMS'))
        if ($sameExe -and $sameSums) {
            Write-Host "    releases\$ver already present and identical"
            Test-BundleIntegrity $target
            return $target
        }
        if (-not $Force) {
            throw "releases\$ver exists with different contents; use -Force to replace it (only when it is not current)"
        }
        if ((Get-CurrentVersion) -eq $ver) {
            throw "releases\$ver is the current release; it cannot be replaced in place"
        }
        if ($PSCmdlet.ShouldProcess($target, 'replace release')) {
            Remove-Item -LiteralPath $target -Recurse -Force
        }
    }
    if ($PSCmdlet.ShouldProcess($target, "install release $ver")) {
        New-Item -ItemType Directory -Force -Path $ReleasesDir | Out-Null
        Copy-Item -LiteralPath $Opened.Dir -Destination $target -Recurse
        Test-BundleIntegrity $target
    }
    return $target
}

# ----------------------------------------------------------------------- service, ACL, firewall

function Set-ServiceOptions {
    # Delayed auto start (models load after the boot rush), restart on failure (also on a
    # non-crash failure exit), and the binary path through the `current` junction.
    $exe = Join-Path $CurrentLink $ExeName
    $imagePath = "`"$exe`" service dispatch --config `"$ConfigPath`" --name $ServiceName"
    if ($PSCmdlet.ShouldProcess($ServiceName, 'set start type, recovery actions and image path')) {
        Set-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName" -Name ImagePath -Value $imagePath
        Invoke-Exe 'sc.exe config' 'sc.exe' @('config', $ServiceName, 'start=', 'delayed-auto') | Out-Null
        Invoke-Exe 'sc.exe failure' 'sc.exe' @('failure', $ServiceName, 'reset=', '86400', 'actions=', 'restart/5000/restart/30000/restart/120000') | Out-Null
        Invoke-Exe 'sc.exe failureflag' 'sc.exe' @('failureflag', $ServiceName, '1') | Out-Null
        Invoke-Exe 'sc.exe sidtype' 'sc.exe' @('sidtype', $ServiceName, 'unrestricted') | Out-Null
    }
}

function Set-Acls {
    # Data: Administrators and SYSTEM full control, the service account read-only, nobody else
    # (the config holds the key hashes). Logs: the service account may write.
    $admins = '*S-1-5-32-544'
    $system = '*S-1-5-18'
    if ($PSCmdlet.ShouldProcess($DataRoot, "restrict ACL to Administrators, SYSTEM and $Account")) {
        Invoke-Exe 'icacls (data)' 'icacls.exe' @($DataRoot, '/inheritance:r', '/grant:r',
            "${admins}:(OI)(CI)F", "${system}:(OI)(CI)F", "${Account}:(OI)(CI)RX", '/Q') | Out-Null
        Invoke-Exe 'icacls (logs)' 'icacls.exe' @($LogDir, '/grant:r', "${Account}:(OI)(CI)M", '/Q') | Out-Null
        # The binaries: inherited from Program Files (Users: read and execute); make the service
        # account's read access explicit in case InstallRoot is elsewhere.
        Invoke-Exe 'icacls (install)' 'icacls.exe' @($InstallRoot, '/grant', "${Account}:(OI)(CI)RX", '/Q') | Out-Null
    }
}

function Set-Firewall([string] $BindAddress) {
    $existing = Get-NetFirewallRule -Name $FirewallRuleName -ErrorAction SilentlyContinue
    if ($NoFirewall -or (Test-Loopback $BindAddress)) {
        if ($existing -and $PSCmdlet.ShouldProcess($FirewallRuleName, 'remove firewall rule')) {
            Remove-NetFirewallRule -Name $FirewallRuleName
        }
        Write-Host '    firewall: no inbound rule (loopback bind or -NoFirewall)'
        return
    }
    $port = Get-BindPort $BindAddress
    if ($PSCmdlet.ShouldProcess($FirewallRuleName, "allow inbound TCP $port from $($FirewallRemoteAddress -join ',')")) {
        if ($existing) {
            Remove-NetFirewallRule -Name $FirewallRuleName
        }
        New-NetFirewallRule -Name $FirewallRuleName -DisplayName "Veriface ($ServiceName, TCP $port)" `
            -Description 'Inbound HTTP to the Veriface service. Restrict RemoteAddress to the integrating backends.' `
            -Direction Inbound -Action Allow -Protocol TCP -LocalPort $port -Service $ServiceName `
            -RemoteAddress $FirewallRemoteAddress -Profile Domain, Private | Out-Null
        Write-Host "    firewall: inbound TCP $port allowed for service $ServiceName from $($FirewallRemoteAddress -join ', ')"
    }
}

function Start-AndWait([string] $What) {
    $exe = Join-Path $CurrentLink $ExeName
    $url = Get-ProbeUrl (Get-ConfigBind $ConfigPath)
    if (-not $PSCmdlet.ShouldProcess($ServiceName, 'start service and wait for ready')) {
        return $true
    }
    Start-Service -Name $ServiceName
    Write-Host "    waiting for $url (up to $ReadyTimeoutSec s) ..."
    if (Wait-Ready $exe $url $ReadyTimeoutSec $null) {
        Write-Host "    $What is ready" -ForegroundColor Green
        return $true
    }
    return $false
}

function Stop-Graceful {
    $svc = Get-Service2
    if ($svc -and $svc.Status -ne 'Stopped' -and $PSCmdlet.ShouldProcess($ServiceName, 'stop service (in-flight requests finish)')) {
        Stop-Service -Name $ServiceName
        $svc.WaitForStatus('Stopped', [TimeSpan]::FromSeconds(120))
    }
}

function Show-Diagnostics {
    $log = Join-Path $LogDir 'veriface.log'
    if (Test-Path -LiteralPath $log) {
        Write-Host "Last lines of ${log}:" -ForegroundColor Yellow
        Get-Content -LiteralPath $log -Tail 15 | ForEach-Object { Write-Host "    $_" }
    }
    Write-Host 'Service Control Manager events (System log):' -ForegroundColor Yellow
    Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = (Get-Date).AddMinutes(-15) } -MaxEvents 20 -ErrorAction SilentlyContinue |
        Where-Object { $_.Message -match [regex]::Escape($ServiceName) -or $_.Message -match 'Veriface' } |
        ForEach-Object { Write-Host "    $($_.TimeCreated) $($_.Message)" }
    Write-Host 'Code Integrity blocks (events 3033/3077) would appear in Microsoft-Windows-CodeIntegrity/Operational.' -ForegroundColor Yellow
}

function Remove-OldReleases([string] $Current, [string] $Previous) {
    $all = @(Get-ChildItem -LiteralPath $ReleasesDir -Directory | Sort-Object LastWriteTime -Descending)
    $keep = @{}
    foreach ($v in @($Current, $Previous)) {
        if ($v) {
            $keep[$v] = $true
        }
    }
    foreach ($dir in $all) {
        if ($keep.Count -ge $KeepReleases) {
            break
        }
        $keep[$dir.Name] = $true
    }
    foreach ($dir in $all) {
        if (-not $keep.ContainsKey($dir.Name) -and $PSCmdlet.ShouldProcess($dir.FullName, 'remove old release')) {
            Remove-Item -LiteralPath $dir.FullName -Recurse -Force
            Write-Host "    removed old release $($dir.Name)"
        }
    }
}

# --------------------------------------------------------------------------------- actions

function Invoke-Verify {
    $opened = Open-Bundle $Bundle
    try {
        Write-Step "Verifying veriface $($opened.Info.version) ($($opened.Dir))"
        Test-BundleIntegrity $opened.Dir
        $config = $ConfigPath
        $tempConfig = $null
        if (-not (Test-Path -LiteralPath $config)) {
            # No installation yet: a throwaway config with a throwaway key.
            $tempConfig = Join-Path ([IO.Path]::GetTempPath()) ("veriface-verify-" + [guid]::NewGuid().ToString('N') + '.toml')
            $key = New-ApiKey
            $text = "[paths]`nmodels = 'models'`nprofiles = 'profiles'`n" + (Format-KeyEntry 'verify' $key.Sha256 @('*'))
            Write-Utf8 $tempConfig $text
            $config = $tempConfig
            Write-Host '    no installed configuration: using a temporary one'
        }
        try {
            Invoke-Preflight $opened.Dir $config
        } finally {
            if ($tempConfig) {
                Remove-Item -LiteralPath $tempConfig -Force -ErrorAction SilentlyContinue
            }
        }
        Write-Host "Bundle $($opened.Info.version) verified." -ForegroundColor Green
    } finally {
        Close-Bundle $opened
    }
}

function Invoke-Install {
    Assert-Admin
    if (Get-Service2) {
        throw "service '$ServiceName' already exists: use -Action Upgrade (or Uninstall first)"
    }
    $opened = Open-Bundle $Bundle
    try {
        $ver = $opened.Info.version
        Write-Step "Installing veriface $ver as service '$ServiceName'"
        Test-BundleIntegrity $opened.Dir

        foreach ($dir in @($InstallRoot, $ReleasesDir, $DataRoot, $ConfigDir, $LogDir)) {
            if ($PSCmdlet.ShouldProcess($dir, 'create directory')) {
                New-Item -ItemType Directory -Force -Path $dir | Out-Null
            }
        }
        $releaseDir = Install-Release $opened

        $newKey = $null
        if (-not (Test-Path -LiteralPath $ConfigPath)) {
            $template = Get-Content -Raw -LiteralPath (Join-Path (Join-Path $opened.Dir 'install') 'config.windows.toml')
            $text = $template.Replace('{{INSTALL_ROOT}}', $InstallRoot).Replace('{{DATA_ROOT}}', $DataRoot).Replace('{{BIND}}', $Bind)
            if ($PSCmdlet.ShouldProcess($ConfigPath, 'write configuration')) {
                Write-Utf8 $ConfigPath $text
            }
        } else {
            Write-Host "    keeping the existing $ConfigPath"
        }
        if (-not (Test-Path -LiteralPath $KeysPath)) {
            $projectName = $Project
            if (-not $projectName) {
                $projectName = 'default'
            }
            $newKey = New-ApiKey
            $header = "# Veriface API keys: only SHA-256 hashes. One [[api_keys]] entry per integrating project.`n# Add one with: Install-Veriface.ps1 -Action NewKey -Project <name>`n"
            if ($PSCmdlet.ShouldProcess($KeysPath, "write the first API key ($projectName)")) {
                Write-Utf8 $KeysPath ($header + (Format-KeyEntry $projectName $newKey.Sha256 $Profiles))
            }
        }

        Write-Step 'Preflight (check + trial run on a spare loopback port)'
        if ($PSCmdlet.ShouldProcess($releaseDir, 'preflight')) {
            Invoke-Preflight $releaseDir $ConfigPath
        }

        Write-Step 'Registering the service'
        Set-Current $ver
        if ($PSCmdlet.ShouldProcess($ServiceName, "register service as $Account")) {
            Invoke-Exe 'veriface service install' (Join-Path $CurrentLink $ExeName) @(
                'service', 'install', '--config', $ConfigPath, '--name', $ServiceName, '--account', $Account) |
                ForEach-Object { Write-Host "    $_" }
        }
        Set-ServiceOptions
        Set-Acls
        Set-Firewall (Get-ConfigBind $ConfigPath)
        Save-State $ver $null 'install'

        Write-Step 'Starting'
        if (-not (Start-AndWait "veriface $ver")) {
            Show-Diagnostics
            throw "the service did not become ready within $ReadyTimeoutSec s (it stays installed; fix and Start-Service $ServiceName)"
        }
        if ($newKey) {
            Write-Host ''
            Write-Host 'API key (shown ONCE; only its SHA-256 is stored in keys.toml). Give it to the integrating project:' -ForegroundColor Yellow
            Write-Host "    $($newKey.Key)" -ForegroundColor Yellow
        }
    } finally {
        Close-Bundle $opened
    }
}

function Invoke-Upgrade {
    Assert-Admin
    if (-not (Get-Service2)) {
        throw "service '$ServiceName' is not installed: use -Action Install"
    }
    $from = Get-CurrentVersion
    $opened = Open-Bundle $Bundle
    try {
        $to = $opened.Info.version
        Write-Step "Upgrading '$ServiceName' from $from to $to"
        if ($to -eq $from -and -not $Force) {
            throw "$to is already the current version (use -Force to re-install it)"
        }
        if ($from -and -not $Force) {
            try {
                $older = [version] ($to -replace '[-+].*$', '') -lt [version] ($from -replace '[-+].*$', '')
            } catch {
                $older = $false
            }
            if ($older) {
                throw "$to is older than ${from}: use -Action Rollback, or -Force to install it anyway"
            }
        }
        Test-BundleIntegrity $opened.Dir
        $releaseDir = Install-Release $opened

        Write-Step "Preflight of $to while $from keeps serving"
        if ($PSCmdlet.ShouldProcess($releaseDir, 'preflight')) {
            Invoke-Preflight $releaseDir $ConfigPath
        }

        Write-Step "Switching to $to"
        Stop-Graceful
        Set-Current $to
        Set-ServiceOptions
        if (Start-AndWait "veriface $to") {
            Save-State $to $from 'upgrade'
            Remove-OldReleases $to $from
            Write-Host "Upgrade to $to done. Roll back with: -Action Rollback" -ForegroundColor Green
            return
        }

        Write-Host "veriface $to did not become ready: rolling back to $from" -ForegroundColor Red
        Show-Diagnostics
        Stop-Graceful
        Set-Current $from
        if (Start-AndWait "veriface $from (rolled back)") {
            Save-State $from $to 'upgrade-failed-rolled-back'
            throw "upgrade to $to failed; $from is serving again"
        }
        throw "upgrade to $to failed AND $from did not become ready after the rollback: investigate now"
    } finally {
        Close-Bundle $opened
    }
}

function Invoke-Rollback {
    Assert-Admin
    $from = Get-CurrentVersion
    $to = $Version
    if (-not $to) {
        $to = (Get-State).previous
    }
    if (-not $to) {
        throw 'no previous version recorded in state.json; pass -Version'
    }
    if ($to -eq $from) {
        throw "$to is already current"
    }
    Write-Step "Rolling back '$ServiceName' from $from to $to"
    Test-BundleIntegrity (Get-ReleaseDir $to)
    Stop-Graceful
    Set-Current $to
    Set-ServiceOptions
    if (Start-AndWait "veriface $to") {
        Save-State $to $from 'rollback'
        return
    }
    Show-Diagnostics
    throw "veriface $to did not become ready after the rollback"
}

function Invoke-Uninstall {
    Assert-Admin
    Write-Step "Removing '$ServiceName'"
    $svc = Get-Service2
    if ($svc) {
        Stop-Graceful
        if ($PSCmdlet.ShouldProcess($ServiceName, 'delete service')) {
            Invoke-Exe 'sc.exe delete' 'sc.exe' @('delete', $ServiceName) | Out-Null
        }
    }
    if ((Get-NetFirewallRule -Name $FirewallRuleName -ErrorAction SilentlyContinue) -and
        $PSCmdlet.ShouldProcess($FirewallRuleName, 'remove firewall rule')) {
        Remove-NetFirewallRule -Name $FirewallRuleName
    }
    if ((Test-Path -LiteralPath $CurrentLink) -and $PSCmdlet.ShouldProcess($CurrentLink, 'remove junction')) {
        [IO.Directory]::Delete($CurrentLink, $false)
    }
    if ((Test-Path -LiteralPath $ReleasesDir) -and $PSCmdlet.ShouldProcess($ReleasesDir, 'remove releases')) {
        Remove-Item -LiteralPath $ReleasesDir -Recurse -Force
    }
    if ((Test-Path -LiteralPath $InstallRoot) -and @(Get-ChildItem -LiteralPath $InstallRoot -Force).Count -eq 0 -and
        $PSCmdlet.ShouldProcess($InstallRoot, 'remove empty install directory')) {
        Remove-Item -LiteralPath $InstallRoot -Force
    }
    if ($RemoveData) {
        if ((Test-Path -LiteralPath $DataRoot) -and $PSCmdlet.ShouldProcess($DataRoot, 'DELETE configuration, keys and logs')) {
            Remove-Item -LiteralPath $DataRoot -Recurse -Force
        }
    } else {
        Write-Host "    kept $DataRoot (configuration, keys, logs); -RemoveData deletes it"
    }
    Write-Host 'Removed.' -ForegroundColor Green
}

function Invoke-Status {
    $svc = Get-Service2
    $cim = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'" -ErrorAction SilentlyContinue
    $state = Get-State
    $current = Get-CurrentVersion
    $releases = @()
    if (Test-Path -LiteralPath $ReleasesDir) {
        $releases = @(Get-ChildItem -LiteralPath $ReleasesDir -Directory | ForEach-Object { $_.Name })
    }
    $health = 'n/a'
    if ($current -and (Test-Path -LiteralPath $ConfigPath)) {
        $exe = Join-Path $CurrentLink $ExeName
        $url = Get-ProbeUrl (Get-ConfigBind $ConfigPath)
        $previousKey = $env:VERIFACE_HEALTHCHECK_KEY
        if ($HealthKey) {
            $env:VERIFACE_HEALTHCHECK_KEY = $HealthKey
        }
        $ErrorActionPreference = 'Continue'
        $health = (& $exe healthcheck --url $url --timeout-ms 2000 2>&1 | ForEach-Object { "$_" }) -join ' '
        $ErrorActionPreference = 'Stop'
        $env:VERIFACE_HEALTHCHECK_KEY = $previousKey
    }
    [pscustomobject]@{
        Service = $ServiceName
        Status = $(if ($svc) { "$($svc.Status)" } else { 'not installed' })
        StartMode = $(if ($cim) { "$($cim.StartMode)" } else { $null })
        Account = $(if ($cim) { "$($cim.StartName)" } else { $null })
        ImagePath = $(if ($cim) { "$($cim.PathName)" } else { $null })
        Current = $current
        Previous = $state.previous
        Releases = $releases -join ', '
        Config = $ConfigPath
        Health = $health
    } | Format-List
}

function Invoke-NewKey {
    Assert-Admin
    if (-not $Project) {
        throw '-Project is required (the integrating project name, used in logs)'
    }
    if (-not (Test-Path -LiteralPath $ConfigDir)) {
        throw "$ConfigDir does not exist: install first"
    }
    $key = New-ApiKey
    if ($PSCmdlet.ShouldProcess($KeysPath, "append API key for $Project")) {
        $existing = ''
        if (Test-Path -LiteralPath $KeysPath) {
            $existing = Get-Content -Raw -LiteralPath $KeysPath
            if ($existing -match "(?m)^\s*project\s*=\s*`"$([regex]::Escape($Project))`"") {
                throw "keys.toml already has a key for project '$Project' (project names must be unique)"
            }
        }
        Write-Utf8 $KeysPath ($existing.TrimEnd() + "`n" + (Format-KeyEntry $Project $key.Sha256 $Profiles))
    }
    Write-Host "Added a key for '$Project' to $KeysPath. Restart the service to apply: Restart-Service $ServiceName"
    Write-Host 'API key (shown ONCE; only its SHA-256 is stored):' -ForegroundColor Yellow
    Write-Host "    $($key.Key)" -ForegroundColor Yellow
}

switch ($Action) {
    'Verify' { Invoke-Verify }
    'Install' { Invoke-Install }
    'Upgrade' { Invoke-Upgrade }
    'Rollback' { Invoke-Rollback }
    'Uninstall' { Invoke-Uninstall }
    'Status' { Invoke-Status }
    'NewKey' { Invoke-NewKey }
}
