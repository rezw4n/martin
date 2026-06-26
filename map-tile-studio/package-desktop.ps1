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

# Make sure Node + Cargo are reachable even if not on this shell's PATH.
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    $node = 'C:\Program Files\nodejs'
    if (Test-Path (Join-Path $node 'node.exe')) { $env:Path = "$node;$env:Path" }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargo = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path (Join-Path $cargo 'cargo.exe')) { $env:Path = "$cargo;$env:Path" }
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
        Ok 'building tile-serviced (headless LAN tile service)…'
        & cargo build --release -p mts-tile-server
        if ($LASTEXITCODE) { throw 'tile-serviced build failed' }
    } finally { Pop-Location }
} else {
    Step 'Skipping build (reusing existing release binary)'
}

# locate the produced exes (workspace target dir; Tauri names the app from productName)
$relDir = Join-Path $proj 'target\release'
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

# Headless tile service (the GUI installs/controls it from the Serving tab).
$serviced = Join-Path $relDir 'tile-serviced.exe'
if (Test-Path $serviced) {
    Copy-Item $serviced (Join-Path $Out 'tile-serviced.exe') -Force
    Ok 'included tile-serviced.exe (LAN service)'
} else {
    Write-Warning "tile-serviced.exe not found at $serviced — the Serving tab won't work"
}

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
$readmeText = @'
====================================================================
 Map Tile Studio   -   by AiGeoLAB   -   https://ai-geolab.org
====================================================================

Turn GeoTIFF imagery into web map tiles (MBTiles tile pyramids or a single
Cloud-Optimized GeoTIFF) and serve them over your network.


1. INSTALL / RUN
----------------
  * Keep every file in this folder together. Do NOT move "Map Tile Studio.exe"
    out on its own - it needs the gdal\ and python\ folders and tile-serviced.exe
    sitting next to it.
  * Double-click  "Map Tile Studio.exe".

  Nothing to install: GDAL and Python travel inside this folder. Windows 11
  already includes the WebView2 runtime the app uses. On older Windows, install
  it once (free):  https://go.microsoft.com/fwlink/p/?LinkId=2124703

  To put it on another PC: copy the WHOLE folder (or send the .zip and unzip it).


2. MAKE A TILE MAP   (Studio tab)
---------------------------------
  1) Source imagery - click "Add GeoTIFFs" or drag .tif/.tiff onto the window.
     The map zooms to each image as you add it.
  2) Output type:
       - Tile map   : a sparse MBTiles z/x/y pyramid (empty areas are skipped).
       - Single COG : one Cloud-Optimized GeoTIFF at the imagery's resolution.
  3) Settings:
       - Map name   : becomes the file name AND the URL "source" name (sec. 4).
       - Coordinate system (Tile map): Web Mercator (EPSG:3857) or WGS84.
       - Tile format: PNG (lossless) or WEBP (smaller).
       - Resampling : bilinear / cubic / lanczos / average / nearest.
       - Zoom range (Tile map): leave blank for auto (max = native resolution,
         min = 8 levels below).
  4) Click "Generate tile map". You then get a live preview and a copyable URL.

  Output files are saved here:
       %LOCALAPPDATA%\MapTileStudio\maps
  e.g.  C:\Users\<you>\AppData\Local\MapTileStudio\maps


3. TILES CATALOG   (Catalog tab)
--------------------------------
  Lists every map you have made (12 per page). Search, sort, and click a card to
  preview it on the map. Each card has copy-URL, reveal-in-Explorer and delete.
  "Import tile map" copies an existing .mbtiles / .tif into the catalog.


4. PUBLISH OVER YOUR NETWORK   (Serving tab)   <- keeps serving 24/7
-------------------------------------------------------------------
  The in-app preview server only runs while the app is open. To serve tiles to
  OTHER machines, and keep serving after you close the app and after the PC
  restarts, install the background service:

      Serving tab  ->  set Port (default 7765)  ->  "Install & start service"
      ->  approve the Windows admin (UAC) prompt.

  This installs tile-serviced.exe as a Windows Service (starts automatically on
  boot) and opens the firewall port for you. The tab shows the address to share.

  URLS  (replace <ip> with this PC's LAN address shown in the Serving tab;
         replace <source> with a map name from the Catalog):

      XYZ tiles      http://<ip>:7765/<source>/{z}/{x}/{y}
      Map list page  http://<ip>:7765/
      TileJSON       http://<ip>:7765/<source>.json
      Health check   http://<ip>:7765/health

  USE THE TILES:
    - QGIS    : Browser panel -> XYZ Tiles -> New Connection -> paste the XYZ URL
                (or add the TileJSON URL). Set min/max zoom if asked.
    - Leaflet : L.tileLayer("http://<ip>:7765/<source>/{z}/{x}/{y}").addTo(map)
    - MapLibre / OpenLayers : add a raster XYZ source with the same URL.
    - Browser : open  http://<ip>:7765/  to see the list and links.

  CONTROL: the Serving tab has Start / Stop / Uninstall. Uninstall also removes
  the firewall rule. Status refreshes automatically.

  INTERNET (not just LAN): put a reverse proxy (Caddy / nginx / IIS) with HTTPS
  in front of port 7765 - do not expose the port directly to the internet.


5. ADVANCED - run the service from a command line
-------------------------------------------------
  tile-serviced.exe also works standalone (Command Prompt / PowerShell):

      tile-serviced run       [--maps <folder>] [--bind <addr:port>]   foreground
      tile-serviced install   [--maps <folder>] [--bind <addr:port>]   (as Admin)
      tile-serviced uninstall                                          (as Admin)
      tile-serviced start | stop                                       (as Admin)
      tile-serviced status

  Defaults: --maps = %LOCALAPPDATA%\MapTileStudio\maps , --bind = 0.0.0.0:7765

  LINUX: the same binary installs a systemd unit
  (/etc/systemd/system/maptilestudio-tiles.service). Run "tile-serviced install"
  with sudo and pass your maps folder with --maps.


6. TROUBLESHOOTING
------------------
  - "GDAL not found" / engine errors: the folder is incomplete - re-unzip so the
    gdal\ and python\ folders sit next to the .exe.
  - Service will not install: approve the admin (UAC) prompt, or run
    "tile-serviced install" from an Administrator command prompt.
  - Another PC cannot reach the tiles: check that the service shows "running",
    both PCs are on the same network, you used THIS PC's LAN IP (Serving tab),
    and the firewall allowed it (re-run Install to re-add the rule).
  - Port already in use: change the Port in the Serving tab before installing.
  - Blank patches on the map: normal for sparse maps - empty areas have no tiles.


--------------------------------------------------------------------
 Map Tile Studio   -   by AiGeoLAB   -   https://ai-geolab.org
--------------------------------------------------------------------
'@
$readmeText | Set-Content -Path (Join-Path $Out 'README.txt') -Encoding UTF8

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
