param(
  [int]$Tabs = 10,
  [string]$Engine = "servo"
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $Root
$exe = "$Root/target/debug/rustle.exe"
if (-not (Test-Path $exe)) {
  cargo build -p rustle-app | Out-Null
}
$urls = @()
for ($i = 1; $i -le $Tabs; $i++) {
  $urls += "https://example.com/?tab=$i"
}
Get-Process rustle,servoshell,obscura -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500
$started = Get-Date
$process = Start-Process $exe -ArgumentList (@("--private") + $urls) -WorkingDirectory "$Root/target/debug" -PassThru
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline -and $process.MainWindowHandle -eq 0) {
  Start-Sleep -Milliseconds 100
  $process.Refresh()
}
$startup_ms = [int]((Get-Date) - $started).TotalMilliseconds
Start-Sleep -Seconds 6
$rustle = Get-Process -Id $process.Id -ErrorAction SilentlyContinue
$servo = Get-Process servoshell -ErrorAction SilentlyContinue
$row = [pscustomobject]@{
  engine = $Engine
  tabs = $Tabs
  startup_ms = $startup_ms
  rustle_mb = if ($rustle) { [math]::Round($rustle.WorkingSet64 / 1MB, 1) } else { 0 }
  servo_mb = if ($servo) { [math]::Round((($servo | Measure-Object WorkingSet64 -Sum).Sum) / 1MB, 1) } else { 0 }
  servo_processes = @($servo).Count
}
$row | Format-List
$row | ConvertTo-Json | Set-Content "$Root/bench-out/servo-tabs.json" -Encoding utf8
Get-Process rustle,servoshell,obscura -ErrorAction SilentlyContinue | Stop-Process -Force
