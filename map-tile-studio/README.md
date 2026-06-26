# Map Tile Studio — by AiGeoLAB

A standalone **desktop app** that turns multiple GeoTIFF images into a single,
integrated, **sparse** tile map. Empty areas produce no blank tiles — only the
tiles that actually contain imagery are written.

White-themed, MapTiler-Engine-style workflow. Built with **Tauri 2 + React +
Vite + Tailwind v4**. The Rust backend calls the tiling engine directly (no
local web server), and the in-app map preview reads generated tiles through a
custom `mbtile://` protocol — fully offline.

<https://ai-geolab.org>

## Develop

```bash
npm install
npm run tauri dev      # launches the desktop app with hot reload
```

## Build a distributable

```bash
npm run tauri build    # produces an .exe + NSIS installer in src-tauri/target/release
```

## Requirements

- **Windows 10/11** with the WebView2 runtime (built into Windows 11).
- **GDAL** at runtime for the generation engine. On this machine it is auto-discovered
  from `C:\OSGeo4W`. For a self-contained installer, bundle a portable GDAL as Tauri
  resources and point `MARTIN_GDAL_BIN` / `GDAL_DATA` / `PROJ_LIB` / `PYTHONHOME`
  at it on startup (the same portable bundle used by the CLI works here).

## How it works

```
 Add GeoTIFFs ──▶ inspect (CRS, footprint)         shown as footprints on the map
       │
       ▼  Generate
 martin-tiler engine (Rust)
   gdalbuildvrt -addalpha → gdalwarp → gdal2tiles --xyz → sparse MBTiles (+ optional COG)
       │
       ▼
 mbtile:// custom protocol ──▶ MapLibre preview (offline, no server)
```

- **Source imagery** — add `.tif`/`.tiff` by click or drag-and-drop; each is inspected
  (CRS, footprint, native zoom) and drawn on the map.
- **Output** — name, coordinate system (Web Mercator / WGS84 / custom EPSG), tile format,
  resampling, zoom range, and an optional single COG.
- **Generate** — live per-stage progress + engine log, then statistics (tiles, sparsity,
  size), validation, a copy-able `z/x/y` URL, and an instant in-app preview.

## Project layout

```
src/                 React UI (white theme, Framer Motion, MapLibre)
  components/         Titlebar, MapCanvas, ui primitives
  lib/                api.ts (Tauri invoke + events), types.ts, utils.ts
src-tauri/           Rust backend
  src/lib.rs          Tauri commands (gdal_status, inspect, generate, validate) + mbtile:// protocol
  tauri.conf.json     window, CSP, bundle
```

The generation engine itself lives in the sibling `martin-tiler` crate and is reused
directly as a library dependency.
