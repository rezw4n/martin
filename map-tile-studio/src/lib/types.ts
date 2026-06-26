// TypeScript mirror of the `martin-tiler` engine model (serde shapes).

export interface BBox {
  min_x: number;
  min_y: number;
  max_x: number;
  max_y: number;
}

export interface RasterInfo {
  path: string;
  file_name: string;
  driver: string;
  width: number;
  height: number;
  band_count: number;
  has_alpha: boolean;
  crs_name: string | null;
  epsg: number | null;
  pixel_size: [number, number];
  bounds_native: BBox;
  bounds_wgs84: BBox;
  native_zoom: number | null;
  resolution_m: number | null;
  file_size: number;
  is_tiled: boolean;
  has_overviews: boolean;
  notes: string[];
}

export type TileFormatId = 'png' | 'webp';
export type ResamplingId = 'near' | 'bilinear' | 'cubic' | 'average' | 'lanczos';

/** Built-in grid id, or a custom projected grid `{ custom: epsg }`. */
export type TileGrid = 'web-mercator' | 'geodetic' | { custom: number };

export interface GridParams {
  epsg: number;
  proj4: string | null;
  tile_origin: [number, number];
  resolutions: number[];
  tile_size: number;
  bounds_crs: [number, number, number, number];
}

export interface ZoomTally {
  zoom: number;
  tiles: number;
}

export interface GridOutput {
  grid: TileGrid;
  mbtiles_path: string;
  source_id: string;
  min_zoom: number;
  max_zoom: number;
  bounds_wgs84: BBox;
  per_zoom: ZoomTally[];
  tiles_total: number;
  dense_total: number;
  empty_skipped: number;
  file_size: number;
  grid_params?: GridParams | null;
}

export interface CogOutput {
  cog_path: string;
  source_id: string;
  bounds_wgs84: BBox;
  max_zoom: number | null;
  format: string;
  file_size: number;
}

export interface GenerateReport {
  name: string;
  inputs: string[];
  outputs: GridOutput[];
  cog_output?: CogOutput | null;
  duration_secs: number;
}

export type CheckStatus = 'pass' | 'warn' | 'fail';

export interface ValidationCheck {
  name: string;
  status: CheckStatus;
  detail: string;
}

export interface ValidationReport {
  mbtiles_path: string;
  ok: boolean;
  checks: ValidationCheck[];
  tiles_total: number;
  min_zoom: number | null;
  max_zoom: number | null;
}

export interface GdalStatus {
  available: boolean;
  error: string | null;
  bin: string | null;
  output_dir: string;
}

export interface ServiceInfo {
  /** "running" | "stopped" | "not installed" | … */
  status: string;
  port: number;
  /** `http://<lan-ip>:<port>` — base for `/{source}/{z}/{x}/{y}`. */
  lan_url: string;
  maps_dir: string;
}

export interface MapEntry {
  source_id: string;
  path: string;
  kind: 'mbtiles' | 'cog';
  name: string;
  format: string;
  crs: string | null;
  min_zoom: number | null;
  max_zoom: number | null;
  tiles_total: number | null;
  /** Source GeoTIFFs stitched into this map (MBTiles only). */
  sources: number | null;
  bounds: [number, number, number, number] | null;
  size: number;
  modified: number;
}

/* ── PostGIS data sources ─────────────────────────────────────────────── */

/** A configured PostgreSQL/PostGIS connection + its live health. */
export interface PgConnDto {
  id: string;
  label: string;
  host: string;
  port: number;
  dbname: string;
  user: string;
  sslmode: string;
  enabled: boolean;
  bundled: boolean;
  ok: boolean;
  message: string;
  table_count: number;
}

/** A served PostGIS vector source (one table). */
export interface PgSourceDto {
  id: string;
  table: string;
  conn_id: string;
  conn_label: string;
  geom_type: string;
  srid: number;
  /** `[name, pg_type]` per feature property. */
  fields: [string, string][];
  minzoom: number;
  maxzoom: number;
  bounds: [number, number, number, number];
  tile_url: string;
  tilejson_url: string;
}

export interface PgOverview {
  available: boolean;
  bundled_running: boolean;
  connections: PgConnDto[];
  sources: PgSourceDto[];
}

export interface PgTestResult {
  ok: boolean;
  message: string;
  table_count: number;
}

/** Editable connection payload sent to `pg_save_connection` / `pg_test_connection`. */
export interface PgConnectionInput {
  id: string;
  label: string;
  host: string;
  port: number;
  dbname: string;
  user: string;
  password: string;
  sslmode: string;
  enabled: boolean;
  bundled: boolean;
}

export interface PgImportParams {
  path: string;
  conn_id?: string | null;
  table?: string | null;
  src_srs?: string | null;
}

export interface PgImportReport {
  table: string;
  source_id: string | null;
  message: string;
}

/** Mirror of martin_tiler::GenerateOptions (the input to the engine). */
export interface GenerateOptions {
  inputs: string[];
  output_dir: string;
  name: string;
  grids: TileGrid[];
  min_zoom?: number | null;
  max_zoom?: number | null;
  format?: TileFormatId;
  resampling?: ResamplingId;
  processes?: number | null;
  cog?: boolean;
  keep_intermediate?: boolean;
}

/** Progress event streamed from the engine while generating (tagged enum). */
export type ProgressEvent =
  | { kind: 'stage'; stage: string; index: number; total: number; grid: TileGrid | null }
  | { kind: 'percent'; stage: string; percent: number }
  | { kind: 'log'; message: string }
  | { kind: 'done'; report: GenerateReport }
  | { kind: 'failed'; error: string };
