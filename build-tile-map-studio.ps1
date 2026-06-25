# ─────────────────────────────────────────────────────────────────────────────
#  Tile Map Studio — build
#
#  Compiles the Martin server (with the Tile Map Studio UI embedded), the
#  martin-tiler generation engine, and the mbtiles CLI.
#
#  Prerequisites (Windows): Rust, Node.js, MSVC C++ Build Tools, and GDAL
#  (OSGeo4W). See TILE-MAP-STUDIO.md.
# ─────────────────────────────────────────────────────────────────────────────
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$features = 'mbtiles,pmtiles,sprites,styles,fonts,metrics,unstable-cog,studio,webui'

Write-Host "Building martin (server + Tile Map Studio UI) ..." -ForegroundColor Cyan
& cargo build --release -p martin --bin martin --no-default-features --features $features
if ($LASTEXITCODE -ne 0) { throw "martin build failed" }

Write-Host "Building martin-tiler (generation engine CLI) ..." -ForegroundColor Cyan
& cargo build --release -p martin-tiler
if ($LASTEXITCODE -ne 0) { throw "martin-tiler build failed" }

Write-Host "Building mbtiles CLI ..." -ForegroundColor Cyan
& cargo build --release -p mbtiles --bin mbtiles
if ($LASTEXITCODE -ne 0) { throw "mbtiles build failed" }

Write-Host ""
Write-Host "Done. Start the Studio with:  .\start-tile-map-studio.ps1" -ForegroundColor Green
