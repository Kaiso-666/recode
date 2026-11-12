# Drive the attached TUI against a running server in a real ConPTY.
#
#   pwsh script/attach_smoke.ps1 -Project <dir> -Config <dir> -Data <dir> [-Port 4319]
#
# Starts the mock provider and `recode serve`, then runs script/tui_smoke.py with
# --server so the TUI follows the server's event stream.
param(
    [Parameter(Mandatory = $true)][string]$Project,
    [Parameter(Mandatory = $true)][string]$Config,
    [Parameter(Mandatory = $true)][string]$Data,
    [string]$Binary = "C:\Users\meow\projects\recode\target\debug\recode.exe",
    [int]$Port = 4319,
    [int]$ProviderPort = 18894
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

$env:RECODE_CONFIG = $Config
$env:RECODE_DATA = $Data

$provider = Start-Process python -ArgumentList "$repo\script\mock_provider.py", "--port", "$ProviderPort" -WindowStyle Hidden -PassThru
$server = Start-Process -FilePath $Binary -ArgumentList "serve", "--port", "$Port", "--token", "smoketoken", "--auto" -WorkingDirectory $Project -WindowStyle Hidden -PassThru
try {
    Start-Sleep -Seconds 3
    python "$repo\script\tui_smoke.py" `
        --project $Project `
        --config $Config `
        --data $Data `
        --binary $Binary `
        --extra "tui --server http://127.0.0.1:$Port --token smoketoken --auto"
    if ($LASTEXITCODE -ne 0) { throw "attach smoke test failed" }
}
finally {
    foreach ($process in @($provider, $server)) {
        if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -Force }
    }
}
Write-Host "attach smoke test passed"
