# Build and stage a release binary for the current platform.
#
#   pwsh script/package.ps1
#
# Produces dist/recode-<os>-<arch>[.exe] plus a .sha256 file. The GitHub release
# workflow uses the same names, so script/install.ps1 and install.sh can fetch
# them directly.
param(
    [string]$Output = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))

& cargo build --release --locked --bin recode
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path $Output | Out-Null
$asset = "recode-windows-x86_64.exe"
$target = Join-Path $Output $asset
Copy-Item -Force "target/release/recode.exe" $target
$hash = (Get-FileHash -Algorithm SHA256 $target).Hash.ToLower()
"$hash  $asset" | Set-Content -NoNewline (Join-Path $Output "$asset.sha256")

Write-Host "wrote $target"
Write-Host "sha256 $hash"
