<#
.SYNOPSIS
    Build Map Tile Studio and assemble a fully self-contained, zip-and-ship folder.

.DESCRIPTION
    Produces a directory (and .zip) that the client can unzip anywhere and run by
    double-clicking the .exe - no installer, no GDAL, no Python, nothing to set up.

    Layout produced (next to the .exe is what the app auto-detects at startup):

        MapTileStudio/
          Map Tile Studio.exe        <- the app
          tile-serviced.exe          <- headless LAN tile service
          gdal/
            bin/                     <- GDAL tools + DLLs (+ libpq.dll for import)
            share/gdal/              <- GDAL_DATA
            share/proj/              <- proj.db  (PROJ_LIB)
          python/                    <- Python + osgeo + osgeo_utils + numpy
          pgsql/                     <- portable PostgreSQL 16 + PostGIS 3.6
            bin/ lib/ share/         <- server, initdb, psql, postgis, proj.db
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
    [string]$Postgres,
    [string]$Out,
    [switch]$SkipBuild,
    [switch]$NoZip
)

$ErrorActionPreference = 'Stop'
$proj = $PSScriptRoot
$tauri = Join-Path $proj 'src-tauri'
# Portable PostgreSQL + PostGIS (bin/lib/share) harvested into vendor\pgsql.
if (-not $Postgres) { $Postgres = Join-Path $proj 'vendor\pgsql' }
# NB: keep this OUT of the Vite `dist/` dir - `npm run build` empties dist/ and
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

# -------- 1. sanity: OSGeo4W harvest sources --------
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

# ogr2ogr needs libpq.dll (its PostgreSQL driver is built into the main GDAL DLL).
if (-not (Test-Path (Join-Path $srcBin 'libpq.dll'))) {
    throw "Missing libpq.dll in $srcBin - PostGIS import (ogr2ogr -f PostgreSQL) would fail."
}

Step "Checking portable PostgreSQL + PostGIS ($Postgres)"
$pgProjDb = Get-ChildItem (Join-Path $Postgres 'share\contrib') -Recurse -Filter 'proj.db' `
    -ErrorAction SilentlyContinue | Select-Object -First 1
foreach ($p in @(
    @{ n = 'postgres.exe'; v = (Join-Path $Postgres 'bin\postgres.exe') },
    @{ n = 'initdb.exe';   v = (Join-Path $Postgres 'bin\initdb.exe') },
    @{ n = 'pg_ctl.exe';   v = (Join-Path $Postgres 'bin\pg_ctl.exe') },
    @{ n = 'psql.exe';     v = (Join-Path $Postgres 'bin\psql.exe') },
    @{ n = 'postgis-3.dll';v = (Join-Path $Postgres 'lib\postgis-3.dll') }
)) {
    if (-not (Test-Path $p.v)) {
        throw "Missing $($p.n): $($p.v). Harvest portable PG+PostGIS into vendor\pgsql (bin/lib/share)."
    }
}
if (-not $pgProjDb) { throw "Missing PostGIS proj.db under $Postgres\share\contrib (needed for ST_Transform)." }
Ok 'PostgreSQL + PostGIS sources OK'

# -------- 2. build the app (frontend + release exe, no installer) --------
if (-not $SkipBuild) {
    Step 'Building frontend + release exe (npm run tauri build --no-bundle)'
    Push-Location $proj
    try {
        if (-not (Test-Path (Join-Path $proj 'node_modules'))) {
            Ok 'installing npm deps...'
            & npm install
            if ($LASTEXITCODE) { throw 'npm install failed' }
        }
        & npm run tauri build -- --no-bundle
        if ($LASTEXITCODE) { throw 'tauri build failed' }
        Ok 'building tile-serviced (headless LAN tile service)...'
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

# -------- 3. assemble the bundle --------
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
    Write-Warning "tile-serviced.exe not found at $serviced - the Serving tab won't work"
}

# Robocopy is dramatically faster than Copy-Item for the big trees.
function Mirror($from, $to) {
    New-Item -ItemType Directory -Path $to -Force | Out-Null
    & robocopy $from $to /E /NFL /NDL /NJH /NJS /NP /R:1 /W:1 | Out-Null
    # robocopy exit codes 0-7 are success; >=8 is a real failure
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($from -> $to): $LASTEXITCODE" }
}

Ok 'gdal/bin (~215 MB)...'
Mirror $srcBin  (Join-Path $Out 'gdal\bin')
Ok 'gdal/share/gdal...'
Mirror $srcData (Join-Path $Out 'gdal\share\gdal')
Ok 'gdal/share/proj...'
Mirror $srcProj (Join-Path $Out 'gdal\share\proj')
Ok 'python (~223 MB)...'
Mirror $srcPy   (Join-Path $Out 'python')
Ok 'pgsql - portable PostgreSQL + PostGIS (~400 MB)...'
Mirror $Postgres (Join-Path $Out 'pgsql')

# -------- 4. trim obvious dead weight (safe: caches + bundled installers only) --------
Step 'Trimming caches'
Get-ChildItem $Out -Recurse -Directory -Filter '__pycache__' -ErrorAction SilentlyContinue |
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
Get-ChildItem $Out -Recurse -File -Include *.pyc, *.pdb -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

# -------- 5. README + smoke test --------
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
  Two sub-tabs (toggle at the top of the Catalog):

  * Tile Maps - every map you have made (12 per page). Search, sort, and click a
    card to preview it. Each card has copy-URL, reveal-in-Explorer and delete.
    "Import tile map" copies an existing .mbtiles / .tif into the catalog.

  * Database  - PostGIS vector data (see section 4).


4. POSTGIS DATA SOURCES   (Catalog tab -> Database)
---------------------------------------------------
  Import shapefiles / GeoJSON into a database and serve them as VECTOR tiles. The
  app ships its own PostgreSQL 16 + PostGIS 3.6 - nothing to install. It starts
  automatically (loopback only) the first time the app or the tile service runs.

  IMPORT DATA:
    Database tab -> "Import data" -> pick a .shp / .geojson / .gpkg / .kml ->
    choose the target connection + table name -> Import.
    The data is reprojected to EPSG:4326 on import and served as Web-Mercator
    vector tiles, so layers line up correctly NO MATTER the source projection
    (a local grid, UTM, etc. - it just works). Each imported table appears as a
    card with a copyable vector-tile URL and a live preview; "drop" deletes it.

  USE THE VECTOR TILES (same server + port as the raster tiles, section 5):
      Vector (MVT)   http://<ip>:7765/<table>/{z}/{x}/{y}
      TileJSON       http://<ip>:7765/<table>.json
    QGIS: Layer -> Add Vector Tile Layer -> paste the TileJSON URL.
    MapLibre/Mapbox: add a "vector" source with the tiles URL; the source-layer
    name is the table name.

  ADD AN EXTERNAL DATABASE (optional):
    Database tab -> "Add connection" -> enter host / port / database / user /
    password -> Test -> Save. Its spatial tables are auto-discovered and served
    too. Connections are NOT hardcoded - they live in a plain config file you can
    edit (see CREDENTIALS).

  CREDENTIALS  (bundled PostGIS - documented + changeable):
      Host       127.0.0.1   (loopback only; not exposed to the network)
      Port       5433
      Database   gis
      User       postgres
      Password   mapstudio
    These live in a plain-text config file, created on first run:
        %LOCALAPPDATA%\MapTileStudio\connections.json
    Edit it to change the bundled password or add external servers, then restart
    the app. The database files (the cluster) are stored next to it in:
        %LOCALAPPDATA%\MapTileStudio\pgdata
    The bundled database listens only on 127.0.0.1, so the password protects local
    access only; LAN clients reach the data through the tile service (section 5),
    not the database directly.


5. PUBLISH OVER YOUR NETWORK   (Serving tab)   <- keeps serving 24/7
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


6. ADVANCED - run the service from a command line
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


7. TROUBLESHOOTING
------------------
  - "GDAL not found" / engine errors: the folder is incomplete - re-unzip so the
    gdal\ and python\ folders sit next to the .exe.
  - PostGIS import fails or "Database" tab shows an error: re-unzip so the pgsql\
    folder sits next to the .exe; make sure no OTHER PostgreSQL is already using
    port 5433. The bundled database logs to %LOCALAPPDATA%\MapTileStudio\pgdata\
    server.log.
  - Vector layer not showing after import: give it a moment (tables refresh every
    ~30 s), or click the refresh icon in the Database tab.
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
    (Join-Path $Out 'python\Lib\site-packages\osgeo_utils\gdal2tiles.py'),
    (Join-Path $Out 'gdal\bin\libpq.dll'),              # ogr2ogr PostgreSQL driver (PostGIS import)
    (Join-Path $Out 'pgsql\bin\postgres.exe'),          # bundled cluster server
    (Join-Path $Out 'pgsql\bin\initdb.exe'),
    (Join-Path $Out 'pgsql\bin\psql.exe'),
    (Join-Path $Out 'pgsql\lib\postgis-3.dll')          # PostGIS extension
)) {
    if (-not (Test-Path $req)) { throw "Bundle is incomplete - missing: $req" }
}
if (-not (Get-ChildItem (Join-Path $Out 'pgsql\share\contrib') -Recurse -Filter 'proj.db' -ErrorAction SilentlyContinue | Select-Object -First 1)) {
    throw "Bundle is incomplete - PostGIS proj.db missing under pgsql\share\contrib"
}
Ok 'all required files present (GDAL + Python + PostgreSQL/PostGIS)'

$env:PYTHONHOME = Join-Path $Out 'python'
$env:Path = (Join-Path $Out 'gdal\bin') + ';' + $env:Path
$env:USE_PATH_FOR_GDAL_PYTHON = 'YES'
$env:GDAL_DATA = Join-Path $Out 'gdal\share\gdal'
$env:PROJ_LIB = Join-Path $Out 'gdal\share\proj'
$env:PROJ_NETWORK = 'OFF'
# Actually exercise the data dirs: a reprojection needs proj.db (PROJ_LIB); the
# osgeo_utils import needs the bindings. (Run on python.exe so PowerShell captures
# the output - pythonw.exe is verified present above and used by the app at runtime.)
$probeCode = "import warnings; warnings.filterwarnings('ignore'); " +
    "from osgeo import gdal, osr; gdal.UseExceptions(); import osgeo_utils.gdal2tiles; " +
    "s=osr.SpatialReference(); s.ImportFromEPSG(3857); assert s.ExportToWkt(), 'PROJ reprojection failed'; " +
    "print('osgeo', gdal.__version__, 'GDAL_DATA+PROJ ok')"
# A native command writing ANYTHING to stderr (e.g. a GDAL FutureWarning) would,
# under ErrorActionPreference='Stop' + 2>&1, be wrapped as a terminating error
# before we can inspect $LASTEXITCODE. Relax it just for this probe and key off the
# real exit code instead.
$prevEAP = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$probe = (& (Join-Path $Out 'python\python.exe') -c $probeCode 2>&1 | Out-String).Trim()
$probeCodeExit = $LASTEXITCODE
$ErrorActionPreference = $prevEAP
if ($probeCodeExit -ne 0) { throw "Bundled GDAL/PROJ smoke test FAILED:`n$probe" }
Ok "smoke test passed: $probe"

# -------- 6. zip --------
$bundleMB = [math]::Round(((Get-ChildItem $Out -Recurse -File | Measure-Object Length -Sum).Sum / 1MB), 0)
if (-not $NoZip) {
    $zip = "$Out.zip"
    Step "Zipping -> $zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path "$Out\*" -DestinationPath $zip -CompressionLevel Optimal
    $zipMB = [math]::Round((Get-Item $zip).Length / 1MB, 0)
    Write-Host "`nDONE - folder $bundleMB MB, zip $zipMB MB" -ForegroundColor Green
    Write-Host "Ship: $zip" -ForegroundColor Green
} else {
    Write-Host "`nDONE - folder $bundleMB MB at $Out" -ForegroundColor Green
}
