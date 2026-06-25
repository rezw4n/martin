# ─────────────────────────────────────────────────────────────────────────────
#  Tile Map Studio — launcher
#
#  Starts Martin with the Tile Map Studio web UI enabled, serving any tile maps
#  you generate. Open http://localhost:3000 and use the "Tile Map Studio" tab.
#
#  Usage:   powershell -ExecutionPolicy Bypass -File .\start-tile-map-studio.ps1
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# Make cargo-installed tools and GDAL discoverable.
$env:Path = "$env:USERPROFILE\.cargo\bin;C:\OSGeo4W\bin;$env:Path"

# Where the Studio looks for input GeoTIFFs and writes generated tile maps.
$env:MARTIN_STUDIO_DATA_DIR   = Join-Path $root 'data'
$env:MARTIN_STUDIO_OUTPUT_DIR = Join-Path $root 'studio-maps'
New-Item -ItemType Directory -Force $env:MARTIN_STUDIO_OUTPUT_DIR | Out-Null

$martin = Join-Path $root 'target\release\martin.exe'
if (-not (Test-Path $martin)) {
    Write-Host "martin.exe not found. Build it first with:" -ForegroundColor Yellow
    Write-Host "  .\build-tile-map-studio.ps1" -ForegroundColor Cyan
    exit 1
}

Write-Host "Tile Map Studio:  http://localhost:3000   (Tile Map Studio tab)" -ForegroundColor Green
Write-Host "  data dir   : $($env:MARTIN_STUDIO_DATA_DIR)"
Write-Host "  output dir : $($env:MARTIN_STUDIO_OUTPUT_DIR)"
Write-Host ""

# Serve the output dir as BOTH MBTiles and COG sources, so generated .mbtiles AND
# .tif COGs (generated or dropped in) are all served at /{name}/{z}/{x}/{y}.
# Forward slashes: Martin's config env-substitution treats backslashes as escapes.
$mapsFwd = ($env:MARTIN_STUDIO_OUTPUT_DIR) -replace '\\','/'
$cfg = Join-Path $root 'studio-serve.yaml'
@"
mbtiles:
  paths:
    - $mapsFwd
cog:
  paths:
    - $mapsFwd
"@ | Out-File -Encoding utf8 $cfg

& $martin --webui enable-for-all --on-invalid warn --listen-addresses 0.0.0.0:3000 -c $cfg
