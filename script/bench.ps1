# Measure startup and size for the local release build.
#
#   pwsh script/bench.ps1 [-Runs 20]
param(
    [int]$Runs = 20
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))

& cargo build --release --locked --bin recode
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$exe = Join-Path (Get-Location) "target/release/recode.exe"
if (-not (Test-Path $exe)) { $exe = Join-Path (Get-Location) "target/release/recode" }

# Warm the file cache before timing.
& $exe --version | Out-Null
$times = New-Object System.Collections.Generic.List[double]
for ($i = 0; $i -lt $Runs; $i++) {
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    & $exe --version | Out-Null
    $watch.Stop()
    $times.Add($watch.Elapsed.TotalMilliseconds)
}
$times.Sort()
$mean = ($times | Measure-Object -Average).Average
$p50 = $times[[int][math]::Floor($times.Count / 2)]
$p95 = $times[[int][math]::Floor($times.Count * 0.95)]
$size = (Get-Item $exe).Length / 1MB

Write-Host ("startup: mean {0:N1} ms, p50 {1:N1} ms, p95 {2:N1} ms over {3} runs" -f $mean, $p50, $p95, $Runs)
Write-Host ("binary:  {0:N1} MiB" -f $size)
