import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import type {
  GdalStatus,
  GenerateOptions,
  GenerateReport,
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
 * Build the raster tile URL template MapLibre uses to preview a generated
 * MBTiles. Tiles are served by the Rust `mbtile://` custom protocol — no HTTP
 * server, fully offline.
 */
export function tileUrlTemplate(mbtilesPath: string): string {
  // Windows serves custom schemes as http://<scheme>.localhost
  const base = 'http://mbtile.localhost';
  return `${base}/{z}/{x}/{y}?src=${encodeURIComponent(mbtilesPath)}`;
}
