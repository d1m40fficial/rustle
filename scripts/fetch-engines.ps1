param(
  [string]$Root = (Resolve-Path "$PSScriptRoot/..").Path
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Ensure-Archive($Name, $Url, $Archive, $Destination) {
  if (Test-Path $Destination) {
    Write-Host "$Name already present: $Destination"
    return
  }
  New-Item -ItemType Directory -Force -Path (Split-Path $Archive) | Out-Null
  if (-not (Test-Path $Archive)) {
    Write-Host "Downloading $Name..."
    Invoke-WebRequest -Uri $Url -OutFile $Archive
  }
  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  Expand-Archive -Path $Archive -DestinationPath $Destination -Force
}

$metadata = Get-Content "$Root/tools/engines.json" -Raw | ConvertFrom-Json

Ensure-Archive "Obscura" $metadata.obscura.url "$Root/tools/obscura/obscura.zip" "$Root/tools/obscura/bin"
Ensure-Archive "Servo" $metadata.servo.url "$Root/tools/servo/servo.zip" "$Root/tools/servo/bin"

Write-Host "Engines ready."
