param([string]$Exe = "dist\Rustle-0.10.0-portable\rustle.exe")
$ErrorActionPreference = "Stop"
$process = Start-Process $Exe -WorkingDirectory (Split-Path $Exe) -PassThru
Start-Sleep -Seconds 6
if ($process.HasExited) { throw "Rustle exited during smoke test" }
$alive = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
if (-not $alive) { throw "Rustle process not found" }
Write-Host "Smoke OK: PID $($process.Id), version $((Get-Item $Exe).VersionInfo.ProductVersion)"
Stop-Process -Id $process.Id -Force
