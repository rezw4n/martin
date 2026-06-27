import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import type {
  GdalStatus,
  GenerateOptions,
  GenerateReport,
  MapEntry,
  PgConnectionInput,
  PgImportParams,
  PgImportReport,
  PgOverview,
  PgTestResult,
  ProgressEvent,
  RasterInfo,
  ServiceInfo,
  ValidationReport,
} from './types';

/** Discover GDAL + the default output directory. */
export const gdalStatus = () => invoke<GdalStatus>('gdal_status');

/** Native multi-select file picker for GeoTIFFs; returns absolute paths. */
export async function pickGeoTiffs(): Promise<string[]> {
  const selection = await open({
    multiple: true,
    directory: false,
    filters: [{ name: 'GeoTIFF', extensions: ['tif', 'tiff'] }],
    title: 'Add imagery',
  });
  if (!selection) return [];
  return Array.isArray(selection) ? selection : [selection];
}

/** Inspect GeoTIFFs (CRS, footprint, native zoom, …). */
export const inspect = (paths: string[]) => invoke<RasterInfo[]>('inspect_paths', { paths });

/** Validate a generated MBTiles file. */
export const validate = (path: string) =>
  invoke<ValidationReport>('validate_mbtiles', { path });

/** Reveal a generated file (or its folder) in the OS file explorer. */
export const revealInExplorer = (path: string) => revealItemInDir(path);

/**
 * Run a generation. Resolves with the final report. Progress is streamed via
 * `onProgress`; the returned promise resolves once the engine is finished.
 */
export async function generate(
  opts: GenerateOptions,
  onProgress: (ev: ProgressEvent) => void,
): Promise<GenerateReport> {
  const unlisten: UnlistenFn = await listen<ProgressEvent>('mts://progress', (e) =>
    onProgress(e.payload),
  );
  try {
    return await invoke<GenerateReport>('generate', { opts });
  } finally {
    unlisten();
  }
}

/**
 * Base URL of the local XYZ tile server (e.g. `http://127.0.0.1:7765`). Started
 * by the Rust backend; serves `{base}/{source}/{z}/{x}/{y}`, fully offline.
 */
export const getTileBase = () => invoke<string>('tile_base');

/** The `{z}/{x}/{y}` raster URL template MapLibre uses (and the user can copy). */
export function tileUrlTemplate(base: string, sourceId: string): string {
  return `${base}/${encodeURIComponent(sourceId)}/{z}/{x}/{y}`;
}

/* ── catalog ──────────────────────────────────────────────────────────── */
export const listMaps = () => invoke<MapEntry[]>('list_maps');
export const deleteMaps = (paths: string[]) => invoke<void>('delete_maps', { paths });

/** Native picker for catalog import — existing tile maps (.mbtiles) or GeoTIFFs.
 *  Returns the chosen absolute paths (no copying happens here). */
export async function pickImportFiles(): Promise<string[]> {
  const sel = await open({
    multiple: true,
    directory: false,
    title: 'Import tile map or imagery',
    filters: [{ name: 'Tile map or GeoTIFF', extensions: ['mbtiles', 'tif', 'tiff'] }],
  });
  if (!sel) return [];
  return Array.isArray(sel) ? sel : [sel];
}

/** Copy an existing, ready-to-serve tile map (.mbtiles) into the catalog folder. */
export const importMapFile = (path: string) => invoke<string>('import_map', { path });

/* ── PostGIS data sources ─────────────────────────────────────────────── */
/** Connections + discovered vector sources, in one call (for the catalog). */
export const pgOverview = () => invoke<PgOverview>('pg_overview');
/** Add or update an external connection (then the registry reconnects). */
export const pgSaveConnection = (conn: PgConnectionInput) =>
  invoke<void>('pg_save_connection', { conn });
export const pgDeleteConnection = (id: string) => invoke<void>('pg_delete_connection', { id });
/** Try a connection without saving it. */
export const pgTestConnection = (conn: PgConnectionInput) =>
  invoke<PgTestResult>('pg_test_connection', { conn });
/** Import a vector file into PostGIS (reprojected to EPSG:4326). */
export const pgImport = (params: PgImportParams) =>
  invoke<PgImportReport>('pg_import', { params });
/** Drop an imported table (delete a vector source). */
export const pgDropSource = (sourceId: string) =>
  invoke<void>('pg_drop_source', { sourceId });

/** Native picker for vector files to import into PostGIS. */
export async function pickVectorFile(): Promise<string | null> {
  const sel = await open({
    multiple: false,
    directory: false,
    title: 'Import data to PostGIS',
    filters: [
      { name: 'Vector data', extensions: ['shp', 'geojson', 'json', 'gpkg', 'kml', 'gml'] },
      { name: 'All files', extensions: ['*'] },
    ],
  });
  if (!sel) return null;
  return Array.isArray(sel) ? (sel[0] ?? null) : sel;
}

/* ── background tile service ──────────────────────────────────────────── */
export const serviceStatus = () => invoke<ServiceInfo>('service_status');
/** Install + start the LAN tile service (triggers a UAC / admin prompt). */
export const serviceInstall = (port: number) => invoke<void>('service_install', { port });
export const serviceUninstall = () => invoke<void>('service_uninstall');
export const serviceSetRunning = (start: boolean) => invoke<void>('service_set_running', { start });

/** A single representative tile (the overview tile at min-zoom) as a thumbnail. */
export function tileThumbUrl(base: string, e: MapEntry): string | null {
  if (!base || !e.bounds || e.min_zoom == null) return null;
  const [w, s, ee, n] = e.bounds;
  const lon = (w + ee) / 2;
  const lat = (s + n) / 2;
  const z = e.min_zoom;
  const scale = 2 ** z;
  const x = Math.floor(((lon + 180) / 360) * scale);
  const latR = (lat * Math.PI) / 180;
  const y = Math.floor(
    ((1 - Math.log(Math.tan(latR) + 1 / Math.cos(latR)) / Math.PI) / 2) * scale,
  );
  return `${base}/${encodeURIComponent(e.source_id)}/${z}/${x}/${y}`;
}
