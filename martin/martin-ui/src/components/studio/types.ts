// TypeScript mirror of the `martin-tiler` / studio API model (martin/src/srv/studio.rs +
// martin-tiler/src/model.rs). These endpoints are not part of the OpenAPI spec, so the
// shapes are declared here by hand.

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

export type TileGridId = 'web-mercator' | 'geodetic';
/** A grid in a report: a built-in id, or a custom projected grid `{ custom: epsg }`. */
export type GridRef = TileGridId | { custom: number };
export type TileFormatId = 'png' | 'webp';
export type ResamplingId = 'near' | 'bilinear' | 'cubic' | 'average' | 'lanczos';

/** Tile-grid parameters for a custom projected (non-Web-Mercator) output. */
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
  grid: GridRef;
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

export type JobStatus = 'running' | 'done' | 'failed';

export interface Job {
  id: string;
  name: string;
  status: JobStatus;
  log: string[];
  stage_index: number;
  stage_total: number;
  percent: number | null;
  report: GenerateReport | null;
  error: string | null;
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

export interface GridInfo {
  id: string;
  epsg: number;
  label: string;
}

export interface StudioConfig {
  gdal_available: boolean;
  gdal_error: string | null;
  gdal_bin: string | null;
  data_dir: string;
  output_dir: string;
  grids: GridInfo[];
  formats: TileFormatId[];
  resamplings: ResamplingId[];
}

export interface BrowseEntry {
  name: string;
  rel_path: string;
  size: number;
}

export interface GenerateRequest {
  inputs: string[];
  name: string;
  grids: TileGridId[];
  /** Additional custom projected output grids by EPSG code (e.g. 9680). */
  custom_epsgs?: number[];
  /** Also produce a single COG served directly by Martin. */
  cog?: boolean;
  min_zoom?: number;
  max_zoom?: number;
  format?: TileFormatId;
  resampling?: ResamplingId;
  processes?: number;
}
