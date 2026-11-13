# Authenticode signing

Sign `veriface.exe` for every production release. Signing gives three things:

- Windows Server and WDAC / AppLocker policies can allow the binary by publisher rather than by hash.
- The installer can refuse an unsigned or foreign build (`-RequireSignature`, `-TrustedSignerThumbprint`).
- SmartScreen and Smart App Control treat a signed, timestamped binary as known.

`onnxruntime.dll` and `onnxruntime_providers_shared.dll` already carry Microsoft's signature. Do not re-sign them.

## What you need

- A **code-signing certificate** (EKU 1.3.6.1.5.5.7.3.3) from a public CA, or from the enterprise CA for internal-only use. Since June 2023, CAs issue public code-signing keys only on hardware: a token, an HSM, or a cloud HSM such as Azure Key Vault or Azure Trusted Signing.
- `signtool.exe` from the Windows 10/11 SDK ("Windows SDK Signing Tools for Desktop Apps").
- Network access to the timestamp server at signing time. The service itself never needs network.

## Sign while packaging

`service/scripts/package.ps1` signs the executable before it computes `SHA256SUMS`, so the hashes cover the signed file:

```powershell
# Certificate in the Windows certificate store (token / smart card / HSM via its CSP or KSP)
powershell -ExecutionPolicy Bypass -File service/scripts/package.ps1 -IncludeModels -Archive `
    -RequireApprovedModels -CertThumbprint 0123456789ABCDEF0123456789ABCDEF01234567

# Or a .pfx file (internal CA, test certificates)
$pw = Read-Host -AsSecureString 'PFX password'
powershell -ExecutionPolicy Bypass -File service/scripts/package.ps1 -IncludeModels -Archive `
    -PfxPath C:\secure\codesign.pfx -PfxPassword $pw
```

Options:

| Option | Default | Notes |
|---|---|---|
| `-TimestampUrl` | `http://timestamp.digicert.com` | Use your CA's RFC 3161 server. Timestamping keeps the signature valid after the certificate expires. |
| `-SignTool` | Newest x64 `signtool.exe` in the Windows SDK | |

The script runs:

```
signtool sign /fd SHA256 /td SHA256 /tr <TimestampUrl> /d "Veriface" (/sha1 <thumbprint> | /f <pfx> /p <password>) veriface.exe
```

It then checks the result with `Get-AuthenticodeSignature` and records `"signed": true` in `BUILDINFO.json`.

For Azure Key Vault or Trusted Signing, sign with `AzureSignTool` or the Trusted Signing `signtool` dlib instead. Do that on `target\release\veriface.exe` **before** running `package.ps1 -SkipBuild`, so that `SHA256SUMS` covers the signed binary.

## Verify

```powershell
Get-AuthenticodeSignature .\veriface.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
signtool verify /pa /v .\veriface.exe
```

The installer checks the signature on every Install, Upgrade and Verify:

```powershell
# Refuse unsigned binaries, and pin the publisher's certificate
.\install\Install-Veriface.ps1 -Action Upgrade -Bundle .\veriface-1.1.0-win-x64.zip `
    -RequireSignature -TrustedSignerThumbprint 0123456789ABCDEF0123456789ABCDEF01234567
```

Without `-RequireSignature`, an unsigned binary only produces a warning, which is fine for test installs.

## Notes

- Pinning by thumbprint breaks when the certificate is renewed. Update the thumbprint in your runbook at every renewal, or pin by publisher in a WDAC policy instead.
- Never disable Code Integrity, Smart App Control or WDAC to run an unsigned build. Sign it, or add an allow rule through your policy process.
- The SHA-256 of the signed executable is in `SHA256SUMS` and `BUILDINFO.json`. Record it in the release notes.
