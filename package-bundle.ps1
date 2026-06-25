# -----------------------------------------------------------------------------
#  package-bundle.ps1 - assemble a zero-install, portable Windows distribution of
#  Martin Tile Map Studio: martin.exe + the web UI + a self-contained GDAL
#  (DLLs + data + proj) + a slimmed portable Python (for 4326 / custom-CRS tiling).
#
#  The client unzips the folder and double-clicks start.bat - no GDAL, Python,
#  Rust or Node install required.
#
#  Usage:  powershell -ExecutionPolicy Bypass -File .\package-bundle.ps1
#          (build martin.exe first:  .\build-tile-map-studio.ps1)
# -----------------------------------------------------------------------------
param(
    [string]$OSGeo4W = 'C:\OSGeo4W',
    [switch]$NoPython,   # build the C++-only (EPSG:3857) bundle (~half the size)
    [switch]$NoZip
)
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$name = 'MartinTileStudio'
$dist = Join-Path $root "dist-bundle\$name"
$martin = Join-Path $root 'target\release\martin.exe'

function Size($p) { if (Test-Path $p) { '{0:N0} MB' -f ((Get-ChildItem $p -Recurse -File -EA SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB) } else { '0 MB' } }

if (-not (Test-Path $martin)) { throw "martin.exe not found at $martin - run .\build-tile-map-studio.ps1 first" }
if (-not (Test-Path $OSGeo4W)) { throw "GDAL (OSGeo4W) not found at $OSGeo4W" }

Write-Host "Assembling portable bundle -> $dist" -ForegroundColor Cyan
if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory -Force $dist | Out-Null
New-Item -ItemType Directory -Force "$dist\gdal\bin" | Out-Null
New-Item -ItemType Directory -Force "$dist\data" | Out-Null
New-Item -ItemType Directory -Force "$dist\maps" | Out-Null

# ---- 1. martin.exe + MSVC runtime DLLs next to it (audit H2) -----------------
Copy-Item $martin "$dist\martin.exe"
$vc = Get-ChildItem "$OSGeo4W\bin" -Filter '*.dll' |
    Where-Object { $_.Name -match '^(msvcp140|vcruntime140|vcomp140|concrt140).*\.dll$' }
$vc | ForEach-Object { Copy-Item $_.FullName "$dist\$($_.Name)" -Force }   # next to martin.exe

# ---- 2. GDAL bin: all DLLs (full closure, superset) + the tools we shell out to
Write-Host "  copying GDAL DLLs + tools ..."
Get-ChildItem "$OSGeo4W\bin" -Filter '*.dll' | ForEach-Object { Copy-Item $_.FullName "$dist\gdal\bin\$($_.Name)" -Force }
foreach ($t in 'gdalinfo','gdalwarp','gdal_translate','gdalbuildvrt','gdaladdo','gdalsrsinfo') {
    $src = Join-Path "$OSGeo4W\bin" "$t.exe"
    if (Test-Path $src) { Copy-Item $src "$dist\gdal\bin\$t.exe" -Force }
}

# ---- 3. GDAL_DATA + PROJ_LIB ------------------------------------------------
$gdalData = if (Test-Path "$OSGeo4W\apps\gdal\share\gdal") { "$OSGeo4W\apps\gdal\share\gdal" } else { "$OSGeo4W\share\gdal" }
Copy-Item $gdalData "$dist\gdal\share\gdal" -Recurse -Force
Copy-Item "$OSGeo4W\share\proj" "$dist\gdal\share\proj" -Recurse -Force

# ---- 4. Portable Python (for 4326 / custom-CRS via `python -m osgeo_utils.gdal2tiles`)
if (-not $NoPython) {
    Write-Host "  copying + slimming portable Python ..."
    $pySrc = Get-ChildItem "$OSGeo4W\apps" -Directory -Filter 'Python*' | Select-Object -First 1
    if (-not $pySrc) { throw "OSGeo4W Python not found under $OSGeo4W\apps" }
    Copy-Item $pySrc.FullName "$dist\python" -Recurse -Force

    # Remove debug CPython artifacts (non-redistributable debug CRT) - audit H1
    Get-ChildItem "$dist\python" -Recurse -File -Include '*_d.pyd','*_d.dll','python_d.exe','python3_d.dll','pythonw_d.exe' -EA SilentlyContinue |
        Remove-Item -Force -EA SilentlyContinue
    # Drop large packages gdal2tiles never needs (keep osgeo, osgeo_utils, numpy) - audit M4
    $drop = 'PyQt6','PyQt6_sip','lxml','plotly','tkinter','turtledemo','test','idlelib','ensurepip'
    foreach ($d in $drop) {
        Get-ChildItem "$dist\python" -Recurse -Directory -Filter $d -EA SilentlyContinue |
            ForEach-Object { Remove-Item $_.FullName -Recurse -Force -EA SilentlyContinue }
    }
    Get-ChildItem "$dist\python" -Recurse -Directory -Filter '__pycache__' -EA SilentlyContinue |
        ForEach-Object { Remove-Item $_.FullName -Recurse -Force -EA SilentlyContinue }
    Get-ChildItem "$dist\python" -Recurse -Directory -Filter '*.dist-info' -EA SilentlyContinue |
        ForEach-Object { Remove-Item $_.FullName -Recurse -Force -EA SilentlyContinue }
    # Sanity: osgeo_utils.gdal2tiles + numpy must still be present
    foreach ($need in 'Lib\site-packages\osgeo','Lib\site-packages\osgeo_utils','Lib\site-packages\numpy') {
        if (-not (Test-Path "$dist\python\$need")) { throw "slim removed a required package: $need" }
    }
}

# ---- 5. start.bat (hardened: %~dp0, x64 guard, USE_PATH_FOR_GDAL_PYTHON, 127.0.0.1)
$pyLines = if ($NoPython) {
@'
REM (C++-only bundle: no Python - EPSG:3857 output only)
'@
} else {
@'
set "PYTHONHOME=%ROOT%\python"
set "PYTHONPATH=%ROOT%\python\Lib;%ROOT%\python\Lib\site-packages"
set "MARTIN_GDAL_PYTHON=%ROOT%\python\python.exe"
'@
}
$startBat = @"
@echo off
setlocal enableextensions
chcp 65001 >nul

if /I not "%PROCESSOR_ARCHITECTURE%"=="AMD64" (
  echo This application requires 64-bit Windows. & pause & exit /b 1
)

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "MARTIN_GDAL_BIN=%ROOT%\gdal\bin"
set "MARTIN_GDAL_PREFIX=%ROOT%\gdal"
set "GDAL_DATA=%ROOT%\gdal\share\gdal"
set "PROJ_LIB=%ROOT%\gdal\share\proj"
set "PROJ_DATA=%ROOT%\gdal\share\proj"
set "PROJ_NETWORK=OFF"
set "USE_PATH_FOR_GDAL_PYTHON=YES"
$pyLines
set "PATH=%ROOT%\gdal\bin;%ROOT%\python;%ROOT%\python\Scripts;%PATH%"

set "MARTIN_STUDIO_DATA_DIR=%ROOT%\data"
set "MARTIN_STUDIO_OUTPUT_DIR=%ROOT%\maps"
if not exist "%MARTIN_STUDIO_DATA_DIR%"   mkdir "%MARTIN_STUDIO_DATA_DIR%"
if not exist "%MARTIN_STUDIO_OUTPUT_DIR%" mkdir "%MARTIN_STUDIO_OUTPUT_DIR%"

REM Serve the maps\ folder as BOTH MBTiles and COG sources (generated + drop-in).
REM Forward slashes: Martin's config env-substitution treats backslashes as escapes.
set "MAPS_FWD=%MARTIN_STUDIO_OUTPUT_DIR:\=/%"
> "%ROOT%\serve.yaml" echo mbtiles:
>>"%ROOT%\serve.yaml" echo   paths:
>>"%ROOT%\serve.yaml" echo     - %MAPS_FWD%
>>"%ROOT%\serve.yaml" echo cog:
>>"%ROOT%\serve.yaml" echo   paths:
>>"%ROOT%\serve.yaml" echo     - %MAPS_FWD%

echo ============================================================
echo   Martin Tile Map Studio
echo   Open http://localhost:3000 in your browser.
echo   (Tile Map Studio tab). Press Ctrl+C here to stop.
echo ============================================================
echo.

"%ROOT%\martin.exe" --webui enable-for-all --on-invalid warn --listen-addresses 127.0.0.1:3000 -c "%ROOT%\serve.yaml"
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" ( echo. & echo martin.exe exited with code %RC%. & pause )
endlocal
"@
# Write start.bat as ASCII/UTF8-no-BOM so cmd.exe reads it correctly
$startBatCrlf = $startBat -replace "`n", "`r`n"
[System.IO.File]::WriteAllText("$dist\start.bat", $startBatCrlf, (New-Object System.Text.UTF8Encoding($false)))

# ---- 6. README + license note -----------------------------------------------
$readme = @"
Martin Tile Map Studio - portable Windows build
================================================

QUICK START
  1. If you downloaded this as a .zip: right-click the .zip -> Properties ->
     check "Unblock" -> OK, THEN extract. (Clears the internet "mark of the web"
     so Windows does not block the app.)
  2. Extract to a SHORT local path, e.g.  C:\MartinTileStudio
     (avoid OneDrive/Desktop and very long paths.)
  3. Double-click  start.bat
  4. Windows may show "Windows protected your PC" (unsigned app) -> More info ->
     Run anyway. Allow the firewall prompt for Private networks if asked.
  5. Your browser: open  http://localhost:3000  and use the "Tile Map Studio" tab.

USING IT
  - Drag your GeoTIFFs onto the "Source imagery" box (or drop files into the
    data\ folder). Pick them, choose output grid(s) and zoom, click Generate.
  - Generated tile maps are written to maps\ and served at  /{name}/{z}/{x}/{y}.
  - Empty areas produce NO blank tiles (sparse output).
  - Custom projected grids (e.g. EPSG:9680) need OpenLayers - the app shows a
    ready-to-paste config after generation.

FOLDERS
  martin.exe     the server + web UI (self-contained)
  gdal\          bundled GDAL (no install needed)
  python\        bundled Python for 4326 / custom-CRS tiling (omit for 3857-only)
  data\          put your input GeoTIFFs here
  maps\          generated .mbtiles tile maps (served by the app)

LICENSES: martin (MIT/Apache-2.0), GDAL (MIT/X11), PROJ (MIT), Python (PSF),
SQLite (public domain), MSVC runtime (Microsoft redistributable). See THIRD-PARTY.
"@
$readmeCrlf = $readme -replace "`n", "`r`n"
[System.IO.File]::WriteAllText("$dist\README.txt", $readmeCrlf, (New-Object System.Text.UTF8Encoding($false)))
"GDAL: MIT/X11 (https://gdal.org)`nPROJ: MIT`nPython: PSF (see python\LICENSE.txt if present)`nSQLite: public domain`nMartin: MIT OR Apache-2.0`nMSVC runtime DLLs: Microsoft Visual C++ redistributable license" |
    Out-File -Encoding utf8 "$dist\THIRD-PARTY-LICENSES.txt"

# ---- 7. report sizes + zip --------------------------------------------------
Write-Host ""
Write-Host "Bundle sizes:" -ForegroundColor Green
Write-Host ("  martin.exe : {0}" -f (Size "$dist\martin.exe"))
Write-Host ("  gdal\      : {0}" -f (Size "$dist\gdal"))
if (-not $NoPython) { Write-Host ("  python\    : {0}" -f (Size "$dist\python")) }
Write-Host ("  TOTAL      : {0}" -f (Size $dist))

if (-not $NoZip) {
    $zip = Join-Path $root "dist-bundle\$name.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Write-Host "`nCompressing -> $zip ..." -ForegroundColor Cyan
    Compress-Archive -Path $dist -DestinationPath $zip -CompressionLevel Optimal
    Write-Host ("Done. Zip: {0}" -f (Size $zip)) -ForegroundColor Green
}
