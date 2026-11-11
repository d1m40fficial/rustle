param([string]$Exe = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) {
    $candidate = Get-ChildItem (Join-Path $root "dist") -Directory -Filter "Rustle-*-portable" |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $candidate) { throw "Portable build not found in dist" }
    $Exe = Join-Path $candidate.FullName "rustle.exe"
}
$Exe = (Resolve-Path $Exe).Path
Write-Host "Starting $Exe"
$process = Start-Process $Exe -WorkingDirectory (Split-Path $Exe) -PassThru
Start-Sleep -Seconds 8
if ($process.HasExited) { throw "Rustle exited during smoke test" }
$alive = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
if (-not $alive) { throw "Rustle process not found" }
Write-Host "Smoke OK: PID $($process.Id), version $((Get-Item $Exe).VersionInfo.ProductVersion)"
Stop-Process -Id $process.Id -Force
Read-Host "Press Enter to close"
