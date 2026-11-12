param(
    [string]$Version = "0.14.0"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$dist = Join-Path $root "dist"
$portable = Join-Path $dist "Rustle-$Version-portable"
Remove-Item $portable -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $portable | Out-Null

Copy-Item "target\release\rustle.exe" $portable
if (Test-Path "target\release\WebView2Loader.dll") {
    Copy-Item "target\release\WebView2Loader.dll" $portable
}
Copy-Item "LICENSE" $portable
Copy-Item "README.md" $portable

$zip = Join-Path $dist "Rustle-$Version-portable.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$portable\*" -DestinationPath $zip -CompressionLevel Optimal

$iscc = $env:ISCC
if (-not $iscc) {
    $candidates = @(
        "C:\Tools\InnoSetup7\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 7\ISCC.exe",
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    )
    $iscc = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { throw "ISCC.exe not found; install Inno Setup or set ISCC" }
& $iscc "installer\rustle.iss"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compile failed" }

$setup = Join-Path $dist "Rustle-$Version-setup.exe"
$sums = Join-Path $dist "SHA256SUMS.txt"
$lines = @()
foreach ($file in @($zip, $setup)) {
    $hash = (Get-FileHash $file -Algorithm SHA256).Hash.ToLower()
    $lines += "$hash  $(Split-Path -Leaf $file)"
    Write-Host "$hash  $file"
}
$lines | Set-Content -Path $sums -Encoding ascii

Write-Host "Artifacts:"
Get-ChildItem $dist -File | Select-Object Name, Length
