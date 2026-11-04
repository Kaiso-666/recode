# Install Re:Code on Windows.
#
#   irm https://raw.githubusercontent.com/Kaiso-666/recode/main/script/install.ps1 | iex
#
# Prefers a release binary; falls back to a local source build when run from a
# checkout. Adds the install directory to the user PATH.
param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\recode",
    [switch]$FromSource
)

$ErrorActionPreference = "Stop"
$repo = "Kaiso-666/recode"
$asset = "recode-windows-x86_64.exe"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$target = Join-Path $InstallDir "recode.exe"

if (-not $FromSource) {
    $url = if ($Version -eq "latest") {
        "https://github.com/$repo/releases/latest/download/$asset"
    } else {
        "https://github.com/$repo/releases/download/$Version/$asset"
    }
    try {
        Write-Host "Downloading $url"
        Invoke-WebRequest -Uri $url -OutFile $target -UseBasicParsing
    } catch {
        Write-Warning "download failed: $($_.Exception.Message)"
        $FromSource = $true
    }
}

if ($FromSource) {
    $root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
    if (-not (Test-Path (Join-Path $root "Cargo.toml"))) {
        throw "No release available and no source checkout found. Pass -FromSource from a checkout."
    }
    Write-Host "Building from $root"
    & cargo install --path (Join-Path $root "crates/recode-cli") --root $InstallDir --locked
    $built = Join-Path $InstallDir "bin/recode.exe"
    if (Test-Path $built) {
        Move-Item -Force $built $target
        Remove-Item -Recurse -Force (Join-Path $InstallDir "bin") -ErrorAction SilentlyContinue
    }
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Write-Host "Added $InstallDir to the user PATH (open a new terminal)."
}

& $target --version
Write-Host "Installed to $target"
