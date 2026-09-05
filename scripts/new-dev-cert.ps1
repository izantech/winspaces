# Creates a self-signed code-signing certificate for DEVELOPMENT builds and
# prints its thumbprint for use with the dist pipeline:
#
#   .\scripts\new-dev-cert.ps1
#   $env:WINSPACES_SIGN_THUMBPRINT = '<thumbprint>'
#   .\dev dist
#
# Self-signed signatures do NOT satisfy SmartScreen for end users - shipping
# builds need a purchased OV/EV code-signing certificate (docs/distribution.md).
# The cert lands in CurrentUser\My; no elevation required. Re-running reuses
# an existing WinSpaces dev cert instead of stacking duplicates.

$ErrorActionPreference = 'Stop'
$SUBJECT = 'CN=WinSpaces Dev Signing'

$existing = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
  Where-Object { $_.Subject -eq $SUBJECT } | Select-Object -First 1
if ($existing) {
  Write-Host "Reusing existing dev cert: $($existing.Thumbprint)"
  Write-Host "  `$env:WINSPACES_SIGN_THUMBPRINT = '$($existing.Thumbprint)'"
  return
}

$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $SUBJECT `
  -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddYears(3) `
  -KeyAlgorithm RSA -KeyLength 3072

Write-Host "Created dev code-signing cert: $($cert.Thumbprint)"
Write-Host "  `$env:WINSPACES_SIGN_THUMBPRINT = '$($cert.Thumbprint)'"
