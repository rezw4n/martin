# Tile Map Studio

A tile-map generation tool built on top of [Martin](https://martin.maplibre.org/). It turns
**multiple GeoTIFF images — in any coordinate system — into a single, integrated, _sparse_ tile
map** served at standard `z/x/y` URLs, with an easy MapTiler-Engine-style web UI.

It was built to satisfy this requirement set:

| Requirement | How it is met |
|---|---|
| Single integrated tile map from multiple GeoTIFFs | `gdalbuildvrt` mosaics all inputs into one map |
| **Empty areas → only the index, no blank tile images** | `gdalbuildvrt -addalpha` makes uncovered areas transparent; `gdal2tiles` skips fully-transparent tiles; the MBTiles stores only tiles that exist |
| Performance / large-volume image processing | GDAL (the engine behind MapTiler/QGIS) does the raster work, multi-process tiling, VRT (no giant intermediate rasters) |
| Tile map output validation | `mbtiles`-based validation: integrity, zoom continuity, sparse-storage proof, blank-tile check, metadata |
| Easy, friendly GUI (like MapTiler Engine) | "Tile Map Studio" tab in Martin's web UI: **drag-drop GeoTIFFs** → see coverage on a map → configure → generate with live progress → preview → validate |
| Multiple coordinate systems | Inputs auto-detected (any CRS) and reprojected; output in **Web Mercator (EPSG:3857)**, **WGS84 (EPSG:4326)**, and/or **any custom projected EPSG** (e.g. 9680) |
| `z/x/y` tile URL format | Served by Martin at `/{map}/{z}/{x}/{y}` |
| Single COG (no MBTiles) | Optionally merge the GeoTIFFs into one **Cloud-Optimized GeoTIFF** (`-of COG TILING_SCHEME=GoogleMapsCompatible`) that Martin serves directly at z/x/y. Drop-in COGs are served too. |
| Add your own imagery | **Drag-and-drop upload** in the UI (`POST /studio/upload`) writes GeoTIFFs into the data folder |
| Zero-install for the client | A **portable Windows bundle** (`package-bundle.ps1`) ships martin.exe + a self-contained GDAL + Python; the client unzips and double-clicks `start.bat` — no installs |

## Architecture

```
 GeoTIFFs (any CRS)
   │  gdalbuildvrt -addalpha   →  mosaic with coverage mask (gaps = transparent)
   │  gdalwarp -t_srs EPSG:…    →  reproject to each output grid (alpha preserved)
   │  gdal2tiles --xyz          →  sparse z/x/y pyramid (transparent tiles skipped)
   │  mbtiles pack              →  sparse MBTiles  (only existing tiles stored)
   │  mbtiles validate          →  integrity + sparse-storage report
   ▼
 Martin serves  /{map}/{z}/{x}/{y}        ← empty areas return 204 (no blank image)
   ▲
 Tile Map Studio (React UI in Martin's web front-end)
```

Components added to the workspace:

- **`martin-tiler`** — the GDAL-backed generation engine (library + `martin-tiler` CLI). Discovers
  GDAL, inspects rasters, runs the mosaic→reproject→tile→pack pipeline, validates output.
- **`martin` `studio` feature** — HTTP API (`/studio/*`) + background job manager that drives the
  engine and serves generated maps.
- **`martin-ui` Tile Map Studio tab** — the React wizard.

## Prerequisites (Windows)

- **GDAL** — [OSGeo4W](https://trac.osgeo.org/osgeo4w/) (provides `gdalinfo`, `gdalbuildvrt`,
  `gdalwarp`, `gdal2tiles`). Default location `C:\OSGeo4W` is auto-detected. Override with the
  `MARTIN_GDAL_BIN` or `MARTIN_GDAL_PREFIX` environment variable.
- **Rust** (cargo), **Node.js** (npm), and **MSVC C++ Build Tools** — only needed to *build*.

## Build

```powershell
.\build-tile-map-studio.ps1
```

(or manually: `cargo build --release -p martin --no-default-features --features mbtiles,pmtiles,sprites,styles,fonts,metrics,unstable-cog,studio,webui`)

## Run — the web UI

```powershell
.\start-tile-map-studio.ps1
```

Then open <http://localhost:3000> and select the **Tile Map Studio** tab.

1. **Source imagery** — tick the GeoTIFFs to combine (read from `MARTIN_STUDIO_DATA_DIR`, default
   `./data`). Their CRS, resolution and native zoom are detected; footprints are drawn on the map so
   you can see gaps.
2. **Output settings** — name, output grid(s), tile format, resampling, and (optional) zoom range.
3. **Generate** — watch live stage/percent progress and a log feed.
4. **Result** — tiles written, **empty tiles skipped**, sparsity %, the copyable `z/x/y` URL, a live
   map preview, and a validation report. The new map is served immediately.

Environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `MARTIN_STUDIO_DATA_DIR` | `./data` | folder of input GeoTIFFs |
| `MARTIN_STUDIO_OUTPUT_DIR` | `./studio-maps` | where generated `.mbtiles` are written and served from |
| `MARTIN_GDAL_BIN` / `MARTIN_GDAL_PREFIX` | auto | GDAL location override |

## Run — the CLI (no server needed)

```powershell
# Inspect imagery
.\target\release\martin-tiler.exe inspect data\*.tif

# Generate a sparse tile map (auto zoom range; Web Mercator + WGS84)
.\target\release\martin-tiler.exe generate --name my_map --output .\studio-maps `
    --grid web-mercator --grid geodetic data\*.tif

# Validate the result
.\target\release\martin-tiler.exe validate .\studio-maps\my_map-webmercator.mbtiles
```

Useful flags: `--min-zoom`, `--max-zoom`, `--format png|webp`, `--resampling near|bilinear|cubic|average|lanczos`, `--processes N`, `--epsg <code>` (custom projected grid, e.g. `--epsg 9680`), `--cog` (also emit a single COG), `--json` (machine-readable output).

## Output options

You can produce any combination of these from one run:

- **Web Mercator (EPSG:3857) MBTiles** — the universal web grid; works in MapLibre, Leaflet, Google, OpenLayers.
- **WGS84 (EPSG:4326) MBTiles** — geographic grid.
- **Custom projected grid (any EPSG, e.g. 9680)** — tiles cut in the projection's own space
  (`gdal2tiles --profile=raster`). These are **not** Web Mercator, so MapLibre/Leaflet/Google cannot
  display them — use **OpenLayers** with a custom tile grid. The Studio shows a ready-to-paste
  OpenLayers config (projection proj4, origin, resolutions, tile size) after generation, and the grid
  parameters are stored in the MBTiles metadata (`crs`, `crs_proj4`, `tile_origin`, `resolutions`,
  `bounds_crs`). Tip: also tick Web Mercator so you have a universally-viewable copy.
- **Single COG** (`--cog`) — merge the inputs into one Cloud-Optimized GeoTIFF in EPSG:3857 aligned to
  the web-mercator tile grid (`-of COG TILING_SCHEME=GoogleMapsCompatible`, sparse blocks). Martin's
  COG source serves it directly at `/{name}-cog/{z}/{x}/{y}` — no MBTiles, one file. Empty areas
  return HTTP 204. **Already have a COG?** Drop it into the output (`maps/`) folder and it's served
  too — no conversion needed.

## Portable distribution (zero-install for the client)

The client should not need to install GDAL, Python, Rust or Node. Build a self-contained Windows
bundle:

```powershell
.\build-tile-map-studio.ps1     # compile martin.exe (with the UI embedded)
.\package-bundle.ps1            # assemble dist-bundle\MartinTileStudio + .zip
```

This produces `dist-bundle\MartinTileStudio.zip` (~119 MB) containing `martin.exe`, a self-contained
GDAL (`gdal\`), a slimmed portable Python (`python\`, for 4326/custom-CRS tiling), the MSVC runtime
DLLs, and a hardened `start.bat`. The client:

1. Right-click the `.zip` → Properties → **Unblock** → extract (clears the internet mark-of-the-web).
2. Extract to a short path (e.g. `C:\MartinTileStudio`), not OneDrive.
3. Double-click **`start.bat`** → open <http://localhost:3000>.

The engine runs `gdal2tiles` as `python -m osgeo_utils.gdal2tiles` and sets
`USE_PATH_FOR_GDAL_PYTHON=YES` so the bundled GDAL/Python work even though `C:\OSGeo4W` is absent
(verified by renaming `C:\OSGeo4W` away and generating + serving with it gone). For a smaller
3857-only bundle, use `package-bundle.ps1 -NoPython` (~180 MB, drops the Python/4326/custom-CRS path).

## How the "no blank tiles" guarantee works

Source GeoTIFFs usually have **no nodata/alpha**, so a naïve mosaic fills gaps between images with
opaque black and would emit blank tiles. The engine adds an **alpha coverage mask** at mosaic time
(`gdalbuildvrt -addalpha`): pixels covered by an image are opaque, everything else is transparent.
`gdal2tiles` then **skips fully-transparent tiles**, and the MBTiles only ever stores tiles that
contain real imagery. Requesting a tile in an empty area returns HTTP `204 No Content` — there is no
blank image on disk or on the wire.

The validation report proves this: it lists the per-zoom non-empty tile counts and the dense
(bounding-box) count the pyramid *would* have needed, so the storage saved is explicit.

## HTTP API (`/studio/*`)

| Method & path | Purpose |
|---|---|
| `GET /studio/config` | engine capabilities + configured directories |
| `GET /studio/browse` | list source GeoTIFFs in the data directory |
| `POST /studio/upload?name=<file.tif>` | upload a GeoTIFF (raw body) into the data directory |
| `POST /studio/inspect` | inspect GeoTIFFs (CRS, footprint, native zoom) |
| `POST /studio/generate` | start a generation job → `{ job_id }` |
| `GET /studio/jobs` · `GET /studio/jobs/{id}` | job list / live status + progress + report |
| `POST /studio/validate` | validate a generated MBTiles |
