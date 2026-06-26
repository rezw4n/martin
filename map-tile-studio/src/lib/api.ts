import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import type {
  GdalStatus,
  GenerateOptions,
  GenerateReport,
  MapEntry,
  ProgressEvent,
  RasterInfo,
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

/** Native picker → copies the chosen tile map(s) into the output folder. */
export async function importMaps(): Promise<number> {
  const sel = await open({
    multiple: true,
    directory: false,
    title: 'Import tile map',
    filters: [{ name: 'Tile map', extensions: ['mbtiles', 'tif', 'tiff'] }],
  });
  if (!sel) return 0;
  const paths = Array.isArray(sel) ? sel : [sel];
  for (const p of paths) await invoke('import_map', { path: p });
  return paths.length;
}

/** A single representative tile (the overview tile at min-zoom) as a thumbnail. */
export function tileThumbUrl(base: string, e: MapEntry): string | null {
  if (!base || e.kind !== 'mbtiles' || !e.bounds || e.min_zoom == null) return null;
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
