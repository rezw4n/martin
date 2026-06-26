<#
.SYNOPSIS
    Build Map Tile Studio and assemble a fully self-contained, zip-and-ship folder.

.DESCRIPTION
    Produces a directory (and .zip) that the client can unzip anywhere and run by
    double-clicking the .exe — no installer, no GDAL, no Python, nothing to set up.

    Layout produced (next to the .exe is what the app auto-detects at startup):

        MapTileStudio/
          Map Tile Studio.exe        <- the app
          gdal/
            bin/                     <- GDAL tools + DLLs   (MARTIN_GDAL_BIN)
            share/gdal/              <- GDAL_DATA
            share/proj/              <- proj.db  (PROJ_LIB)
          python/                    <- Python + osgeo + osgeo_utils + numpy
          README.txt

    The Rust backend (src-tauri/src/lib.rs :: setup_bundled_gdal) points the engine
    at ./gdal and ./python when they sit beside the exe, so the bundle is portable.

.PARAMETER OSGeo4W
    Root of an OSGeo4W install to harvest GDAL + Python from. Default: C:\OSGeo4W

.PARAMETER Out
    Output folder for the assembled bundle. Default: <project>\dist\MapTileStudio

.PARAMETER SkipBuild
    Reuse an existing release build instead of rebuilding the frontend + exe.

.PARAMETER NoZip
    Assemble the folder but don't create the .zip.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\package-desktop.ps1
#>
[CmdletBinding()]
param(
    [string]$OSGeo4W = 'C:\OSGeo4W',
    [string]$Out,
    [switch]$SkipBuild,
    [switch]$NoZip
)

$ErrorActionPreference = 'Stop'
$proj = $PSScriptRoot
$tauri = Join-Path $proj 'src-tauri'
# NB: keep this OUT of the Vite `dist/` dir — `npm run build` empties dist/ and
# would wipe the assembled bundle.
if (-not $Out) { $Out = Join-Path $proj 'release\MapTileStudio' }

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "    $m" -ForegroundColor DarkGray }

# Make sure Node is reachable even if it isn't on this shell's PATH.
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    $node = 'C:\Program Files\nodejs'
    if (Test-Path (Join-Path $node 'node.exe')) { $env:Path = "$node;$env:Path" }
}

# ── 1. sanity: OSGeo4W harvest sources ──────────────────────────────────────
Step "Checking GDAL source ($OSGeo4W)"
$srcBin  = Join-Path $OSGeo4W 'bin'
$srcData = Join-Path $OSGeo4W 'apps\gdal\share\gdal'
$srcProj = Join-Path $OSGeo4W 'share\proj'
$srcPy   = Join-Path $OSGeo4W 'apps\Python312'
foreach ($p in @(
    @{ n = 'gdalinfo.exe'; v = (Join-Path $srcBin 'gdalinfo.exe') },
    @{ n = 'GDAL_DATA';    v = (Join-Path $srcData 'gdalvrt.xsd') },
    @{ n = 'proj.db';      v = (Join-Path $srcProj 'proj.db') },
    @{ n = 'python.exe';   v = (Join-Path $srcPy 'python.exe') },
    @{ n = 'osgeo_utils';  v = (Join-Path $srcPy 'Lib\site-packages\osgeo_utils\gdal2tiles.py') }
)) {
    if (-not (Test-Path $p.v)) { throw "Missing $($p.n): $($p.v)" }
}
Ok 'GDAL + Python sources OK'

# ── 2. build the app (frontend + release exe, no installer) ─────────────────
if (-not $SkipBuild) {
    Step 'Building frontend + release exe (npm run tauri build --no-bundle)'
    Push-Location $proj
    try {
        if (-not (Test-Path (Join-Path $proj 'node_modules'))) {
            Ok 'installing npm deps…'
            & npm install
            if ($LASTEXITCODE) { throw 'npm install failed' }
        }
        & npm run tauri build -- --no-bundle
        if ($LASTEXITCODE) { throw 'tauri build failed' }
    } finally { Pop-Location }
} else {
    Step 'Skipping build (reusing existing release binary)'
}

# locate the produced exe (Tauri names it from productName, fall back to crate name)
$relDir = Join-Path $tauri 'target\release'
$exe = @(
    (Join-Path $relDir 'Map Tile Studio.exe'),
    (Join-Path $relDir 'map-tile-studio.exe')
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
    $exe = Get-ChildItem $relDir -Filter *.exe -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -notmatch 'build|deps' } |
        Sort-Object Length -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $exe) { throw "Could not find a built .exe in $relDir" }
Ok "exe: $exe"

# ── 3. assemble the bundle ──────────────────────────────────────────────────
Step "Assembling bundle -> $Out"
if (Test-Path $Out) { Remove-Item $Out -Recurse -Force }
New-Item -ItemType Directory -Path $Out -Force | Out-Null

$bundleExe = Join-Path $Out 'Map Tile Studio.exe'
Copy-Item $exe $bundleExe -Force

# Robocopy is dramatically faster than Copy-Item for the big trees.
function Mirror($from, $to) {
    New-Item -ItemType Directory -Path $to -Force | Out-Null
    & robocopy $from $to /E /NFL /NDL /NJH /NJS /NP /R:1 /W:1 | Out-Null
    # robocopy exit codes 0-7 are success; >=8 is a real failure
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($from -> $to): $LASTEXITCODE" }
}

Ok 'gdal/bin (~215 MB)…'
Mirror $srcBin  (Join-Path $Out 'gdal\bin')
Ok 'gdal/share/gdal…'
Mirror $srcData (Join-Path $Out 'gdal\share\gdal')
Ok 'gdal/share/proj…'
Mirror $srcProj (Join-Path $Out 'gdal\share\proj')
Ok 'python (~223 MB)…'
Mirror $srcPy   (Join-Path $Out 'python')

# ── 4. trim obvious dead weight (safe: caches + bundled installers only) ─────
Step 'Trimming caches'
Get-ChildItem $Out -Recurse -Directory -Filter '__pycache__' -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
Get-ChildItem $Out -Recurse -File -Include *.pyc, *.pdb -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

# ── 5. README + smoke test ──────────────────────────────────────────────────
@"
Map Tile Studio — by AiGeoLAB (https://ai-geolab.org)

HOW TO RUN
  1. Keep every file in this folder together (don't move the .exe out on its own).
  2. Double-click "Map Tile Studio.exe".

That's it — GDAL and Python travel inside this folder, so nothing needs to be
installed. Windows 11 already includes the WebView2 runtime the app uses; on an
older Windows, install it once from https://go.microsoft.com/fwlink/p/?LinkId=2124703

Generated tile maps are saved under:  %LOCALAPPDATA%\MapTileStudio\maps
"@ | Set-Content -Path (Join-Path $Out 'README.txt') -Encoding UTF8

Step 'Verifying bundle contents + GDAL/PROJ runtime'
# The data trees are only used at tiling time, so assert they actually landed in
# the bundle (a partial robocopy returns a "success" code) before we ship.
foreach ($req in @(
    (Join-Path $Out 'gdal\bin\gdalinfo.exe'),
    (Join-Path $Out 'gdal\bin\gdalwarp.exe'),
    (Join-Path $Out 'gdal\share\gdal\gdalvrt.xsd'),     # GDAL_DATA
    (Join-Path $Out 'gdal\share\proj\proj.db'),         # PROJ_LIB
    (Join-Path $Out 'python\python.exe'),
    (Join-Path $Out 'python\pythonw.exe'),              # windowless interpreter the app uses
    (Join-Path $Out 'python\Lib\site-packages\osgeo_utils\gdal2tiles.py')
)) {
    if (-not (Test-Path $req)) { throw "Bundle is incomplete — missing: $req" }
}
Ok 'all required files present'

$env:PYTHONHOME = Join-Path $Out 'python'
$env:Path = (Join-Path $Out 'gdal\bin') + ';' + $env:Path
$env:USE_PATH_FOR_GDAL_PYTHON = 'YES'
$env:GDAL_DATA = Join-Path $Out 'gdal\share\gdal'
$env:PROJ_LIB = Join-Path $Out 'gdal\share\proj'
$env:PROJ_NETWORK = 'OFF'
# Actually exercise the data dirs: a reprojection needs proj.db (PROJ_LIB); the
# osgeo_utils import needs the bindings. (Run on python.exe so PowerShell captures
# the output — pythonw.exe is verified present above and used by the app at runtime.)
$probeCode = "from osgeo import gdal, osr; import osgeo_utils.gdal2tiles; " +
    "s=osr.SpatialReference(); s.ImportFromEPSG(3857); assert s.ExportToWkt(), 'PROJ reprojection failed'; " +
    "print('osgeo', gdal.__version__, 'GDAL_DATA+PROJ ok')"
$probe = & (Join-Path $Out 'python\python.exe') -c $probeCode 2>&1
if ($LASTEXITCODE -ne 0) { throw "Bundled GDAL/PROJ smoke test FAILED:`n$probe" }
Ok "smoke test passed: $probe"

# ── 6. zip ──────────────────────────────────────────────────────────────────
$bundleMB = [math]::Round(((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 0)
if (-not $NoZip) {
    $zip = "$Out.zip"
    Step "Zipping -> $zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path "$Out\*" -DestinationPath $zip -CompressionLevel Optimal
    $zipMB = [math]::Round((Get-Item $zip).Length / 1MB, 0)
    Write-Host "`nDONE — folder $bundleMB MB, zip $zipMB MB" -ForegroundColor Green
    Write-Host "Ship: $zip" -ForegroundColor Green
} else {
    Write-Host "`nDONE — folder $bundleMB MB at $Out" -ForegroundColor Green
}
