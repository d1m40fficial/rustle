param(
  [string[]]$Urls = @("https://example.com", "https://www.rust-lang.org", "https://servo.org"),
  [string[]]$Engines = @("obscura", "servo"),
  [int]$Repeat = 1
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $Root
New-Item -ItemType Directory -Force -Path "$Root/bench-out" | Out-Null

cargo build -p rustle-engine-cli --release | Out-Null
$cli = "$Root/target/release/rustle-capture.exe"
$rows = @()

foreach ($engine in $Engines) {
  foreach ($url in $Urls) {
    $out = "$Root/bench-out/$engine-$([guid]::NewGuid().ToString('N')).png"
    Write-Host "run $engine $url"
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $old = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $line = & $cli --engine $engine --url $url --out $out --width 1280 --height 720 --timeout 30000 --repeat $Repeat 2>$null | Select-Object -Last 1
    $exit = $LASTEXITCODE
    $ErrorActionPreference = $old
    $watch.Stop()
    Write-Host "exit=$exit line=$line"
    try {
      $data = $line | ConvertFrom-Json
      $rows += [pscustomobject]@{
        engine = $engine
        url = $url
        wall_ms = $watch.ElapsedMilliseconds
        render_ms = $data.elapsed_ms
        bytes = $data.bytes
        ok = ($exit -eq 0)
      }
    } catch {
      $rows += [pscustomobject]@{
        engine = $engine
        url = $url
        wall_ms = $watch.ElapsedMilliseconds
        render_ms = $null
        bytes = 0
        ok = $false
      }
    }
  }
}

$rows | Format-Table
$rows | ConvertTo-Json -Depth 3 | Set-Content "$Root/bench-out/latest.json" -Encoding utf8
