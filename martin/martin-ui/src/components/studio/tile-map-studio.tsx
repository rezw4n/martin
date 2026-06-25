import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  FileImage,
  Globe2,
  Layers,
  Loader2,
  MapPin,
  Play,
  Sparkles,
  UploadCloud,
  XCircle,
} from 'lucide-react';
import { type DragEvent, type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { useToast } from '@/hooks/use-toast';
import { buildMartinUrl } from '@/lib/api';
import { cn } from '@/lib/utils';
import { CoverageMap } from './coverage-map';
import { studioApi } from './studio-api';
import type {
  BBox,
  BrowseEntry,
  CogOutput,
  GenerateReport,
  GridOutput,
  GridParams,
  GridRef,
  Job,
  RasterInfo,
  ResamplingId,
  StudioConfig,
  TileFormatId,
  TileGridId,
  ValidationReport,
} from './types';

type PreviewTarget = { sourceId: string; bounds: BBox } | null;

function gridLabel(grid: GridRef): string {
  if (grid === 'web-mercator') return 'Web Mercator (EPSG:3857)';
  if (grid === 'geodetic') return 'WGS84 geographic (EPSG:4326)';
  return `Custom projected grid (EPSG:${grid.custom})`;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

const RESAMPLINGS: ResamplingId[] = ['near', 'bilinear', 'cubic', 'average', 'lanczos'];

export function TileMapStudio() {
  const { toast } = useToast();

  const [config, setConfig] = useState<StudioConfig | null>(null);
  const [files, setFiles] = useState<BrowseEntry[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [inspected, setInspected] = useState<RasterInfo[]>([]);
  const [inspecting, setInspecting] = useState(false);

  const [name, setName] = useState('my_tilemap');
  const [grids, setGrids] = useState<Set<TileGridId>>(new Set(['web-mercator']));
  const [customEnabled, setCustomEnabled] = useState(false);
  const [customEpsg, setCustomEpsg] = useState('9680');
  const [cogEnabled, setCogEnabled] = useState(false);
  const [format, setFormat] = useState<TileFormatId>('png');
  const [resampling, setResampling] = useState<ResamplingId>('bilinear');
  const [minZoom, setMinZoom] = useState<string>('');
  const [maxZoom, setMaxZoom] = useState<string>('');

  const [job, setJob] = useState<Job | null>(null);
  const [report, setReport] = useState<GenerateReport | null>(null);
  const [validation, setValidation] = useState<ValidationReport | null>(null);
  const [preview, setPreview] = useState<PreviewTarget>(null);
  const logEndRef = useRef<HTMLDivElement>(null);

  // ---- initial load -------------------------------------------------------
  useEffect(() => {
    studioApi
      .config()
      .then(setConfig)
      .catch((e: Error) => toast({ description: e.message, title: 'Studio', variant: 'destructive' }));
    studioApi.browse().then(setFiles).catch(() => setFiles([]));
  }, [toast]);

  // ---- inspect on selection change ---------------------------------------
  useEffect(() => {
    const inputs = [...selected];
    if (inputs.length === 0) {
      setInspected([]);
      return;
    }
    setInspecting(true);
    let cancelled = false;
    studioApi
      .inspect(inputs)
      .then((infos) => {
        if (!cancelled) setInspected(infos);
      })
      .catch((e: Error) => {
        if (!cancelled) toast({ description: e.message, title: 'Inspect failed', variant: 'destructive' });
      })
      .finally(() => {
        if (!cancelled) setInspecting(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selected, toast]);

  // suggested native max zoom from inspected imagery
  const suggestedMax = useMemo(() => {
    const zs = inspected.map((i) => i.native_zoom).filter((z): z is number => z != null);
    return zs.length ? Math.max(...zs) : null;
  }, [inspected]);

  // coverage-gap heuristic: union area vs. sum of footprint areas
  const gapInfo = useMemo(() => {
    if (inspected.length < 2) return null;
    const area = (b: RasterInfo['bounds_wgs84']) => (b.max_x - b.min_x) * (b.max_y - b.min_y);
    const sum = inspected.reduce((a, i) => a + area(i.bounds_wgs84), 0);
    const u = inspected.reduce((acc, i) => ({
      max_x: Math.max(acc.max_x, i.bounds_wgs84.max_x),
      max_y: Math.max(acc.max_y, i.bounds_wgs84.max_y),
      min_x: Math.min(acc.min_x, i.bounds_wgs84.min_x),
      min_y: Math.min(acc.min_y, i.bounds_wgs84.min_y),
    }), inspected[0].bounds_wgs84);
    const ratio = sum / Math.max(area(u), 1e-12);
    return ratio < 0.85 ? Math.round((1 - ratio) * 100) : null;
  }, [inspected]);

  // ---- poll a running job -------------------------------------------------
  useEffect(() => {
    if (!job || job.status !== 'running') return;
    const id = job.id;
    const timer = setInterval(async () => {
      try {
        const next = await studioApi.job(id);
        setJob(next);
        if (next.status === 'done' && next.report) {
          setReport(next.report);
          const first = next.report.outputs[0];
          if (first) {
            setPreview({ bounds: first.bounds_wgs84, sourceId: first.source_id });
            studioApi.validate(first.mbtiles_path).then(setValidation).catch(() => {});
          } else if (next.report.cog_output) {
            const c = next.report.cog_output;
            setPreview({ bounds: c.bounds_wgs84, sourceId: c.source_id });
          }
          toast({ description: 'Generation complete.', title: 'Done' });
        } else if (next.status === 'failed') {
          toast({ description: next.error ?? 'Generation failed', title: 'Failed', variant: 'destructive' });
        }
      } catch {
        /* keep polling */
      }
    }, 1200);
    return () => clearInterval(timer);
  }, [job, toast]);

  // auto-scroll the log feed
  useEffect(() => {
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [job?.log.length]);

  const toggleFile = (rel: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(rel)) next.delete(rel);
      else next.add(rel);
      return next;
    });
  };

  const toggleGrid = (g: TileGridId) => {
    setGrids((prev) => {
      const next = new Set(prev);
      if (next.has(g)) {
        if (next.size > 1) next.delete(g);
      } else next.add(g);
      return next;
    });
  };

  const refreshFiles = () => {
    studioApi.browse().then(setFiles).catch(() => setFiles([]));
  };

  const gdalReady = config?.gdal_available ?? false;
  const isRunning = job?.status === 'running';
  const canGenerate = gdalReady && selected.size > 0 && name.trim().length > 0 && !isRunning;

  const startGenerate = async () => {
    setReport(null);
    setValidation(null);
    setPreview(null);
    try {
      const { job_id } = await studioApi.generate({
        cog: cogEnabled,
        custom_epsgs: customEnabled && customEpsg ? [Number(customEpsg)] : [],
        format,
        grids: [...grids],
        inputs: [...selected],
        max_zoom: maxZoom ? Number(maxZoom) : undefined,
        min_zoom: minZoom ? Number(minZoom) : undefined,
        name: name.trim(),
        resampling,
      });
      setJob({
        error: null,
        id: job_id,
        log: [],
        name: name.trim(),
        percent: null,
        report: null,
        stage_index: 0,
        stage_total: 0,
        status: 'running',
      });
    } catch (e) {
      toast({ description: (e as Error).message, title: 'Could not start', variant: 'destructive' });
    }
  };

  return (
    <div className="space-y-6">
      <StudioHeader config={config} />

      {config && !gdalReady && (
        <Card className="border-destructive/40">
          <CardContent className="flex items-start gap-3 pt-6">
            <AlertTriangle className="mt-0.5 size-5 shrink-0 text-destructive" />
            <div>
              <p className="font-medium text-foreground">GDAL toolchain not found</p>
              <p className="text-muted-foreground text-sm">{config.gdal_error}</p>
            </div>
          </CardContent>
        </Card>
      )}

      <div className="grid grid-cols-1 gap-6 lg:grid-cols-5">
        {/* ---- left: workflow ------------------------------------------- */}
        <div className="space-y-6 lg:col-span-2">
          <SourceCard
            files={files}
            gapInfo={gapInfo}
            inspecting={inspecting}
            inspected={inspected}
            onToggle={toggleFile}
            onToggleAll={(all) =>
              setSelected(all ? new Set(files.map((f) => f.rel_path)) : new Set())
            }
            onUploaded={refreshFiles}
            selected={selected}
          />

          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-base">
                <span className="flex size-6 items-center justify-center rounded-full bg-primary/10 text-primary text-xs">
                  2
                </span>
                Output settings
              </CardTitle>
              <CardDescription>How the integrated tile map is generated.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-5">
              <Field label="Map name">
                <Input onChange={(e) => setName(e.target.value)} placeholder="my_tilemap" value={name} />
              </Field>

              <Field label="Coordinate systems (output grids)">
                <div className="flex flex-wrap gap-2">
                  <ToggleChip
                    active={grids.has('web-mercator')}
                    icon={<Globe2 className="size-3.5" />}
                    label="Web Mercator · 3857"
                    onClick={() => toggleGrid('web-mercator')}
                  />
                  <ToggleChip
                    active={grids.has('geodetic')}
                    icon={<Globe2 className="size-3.5" />}
                    label="WGS84 · 4326"
                    onClick={() => toggleGrid('geodetic')}
                  />
                </div>
                <div className="mt-2 space-y-2 rounded-md border border-border p-2.5">
                  <label className="flex cursor-pointer items-center gap-2 text-sm">
                    <input
                      checked={customEnabled}
                      className="size-4 accent-[hsl(var(--primary))]"
                      onChange={(e) => setCustomEnabled(e.target.checked)}
                      type="checkbox"
                    />
                    <span className="font-medium text-foreground">Custom projected grid (EPSG)</span>
                  </label>
                  {customEnabled && (
                    <>
                      <div className="flex items-center gap-2">
                        <span className="text-muted-foreground text-sm">EPSG:</span>
                        <Input
                          className="w-28"
                          onChange={(e) => setCustomEpsg(e.target.value.replace(/[^0-9]/g, ''))}
                          placeholder="9680"
                          value={customEpsg}
                        />
                        <span className="text-muted-foreground text-xs">e.g. 9680 = TM 90 NE</span>
                      </div>
                      <p className="flex items-start gap-1.5 text-amber-600 text-xs dark:text-amber-500">
                        <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                        Non-Web-Mercator tiles. MapLibre / Leaflet / Google can't display these — use
                        OpenLayers with the tile grid shown after generation. Keep Web Mercator ticked
                        for a universally-viewable copy.
                      </p>
                    </>
                  )}
                </div>
              </Field>

              <div className="grid grid-cols-2 gap-4">
                <Field label="Tile format">
                  <div className="flex gap-2">
                    {(['png', 'webp'] as TileFormatId[]).map((f) => (
                      <ToggleChip
                        active={format === f}
                        key={f}
                        label={f.toUpperCase()}
                        onClick={() => setFormat(f)}
                      />
                    ))}
                  </div>
                </Field>
                <Field label="Resampling">
                  <select
                    className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
                    onChange={(e) => setResampling(e.target.value as ResamplingId)}
                    value={resampling}
                  >
                    {RESAMPLINGS.map((r) => (
                      <option key={r} value={r}>
                        {r}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>

              <Field
                hint={
                  suggestedMax != null
                    ? `Imagery supports up to ~z${suggestedMax}. Leave blank for automatic.`
                    : 'Leave blank for automatic zoom range.'
                }
                label="Zoom range"
              >
                <div className="flex items-center gap-2">
                  <Input
                    className="w-24"
                    onChange={(e) => setMinZoom(e.target.value)}
                    placeholder="min"
                    type="number"
                    value={minZoom}
                  />
                  <span className="text-muted-foreground">–</span>
                  <Input
                    className="w-24"
                    onChange={(e) => setMaxZoom(e.target.value)}
                    placeholder={suggestedMax != null ? String(suggestedMax) : 'max'}
                    type="number"
                    value={maxZoom}
                  />
                </div>
              </Field>

              <div className="space-y-1.5 rounded-md border border-border p-2.5">
                <label className="flex cursor-pointer items-center gap-2 text-sm">
                  <input
                    checked={cogEnabled}
                    className="size-4 accent-[hsl(var(--primary))]"
                    onChange={(e) => setCogEnabled(e.target.checked)}
                    type="checkbox"
                  />
                  <span className="font-medium text-foreground">
                    Also create a single COG file
                  </span>
                </label>
                <p className="text-muted-foreground text-xs">
                  A Cloud-Optimized GeoTIFF (EPSG:3857) that Martin serves directly at z/x/y — one
                  file, no MBTiles. Empty areas stay tile-free. Already have a COG? Drop it into the
                  output folder and it's served too.
                </p>
              </div>

              <Button
                className="w-full"
                disabled={!canGenerate}
                onClick={startGenerate}
                size="lg"
              >
                {isRunning ? (
                  <>
                    <Loader2 className="size-4 animate-spin" /> Generating…
                  </>
                ) : (
                  <>
                    <Play className="size-4" /> Generate tile map
                  </>
                )}
              </Button>
              {selected.size === 0 && (
                <p className="text-center text-muted-foreground text-xs">
                  Select at least one source image to begin.
                </p>
              )}
            </CardContent>
          </Card>
        </div>

        {/* ---- right: map + progress + result --------------------------- */}
        <div className="space-y-6 lg:col-span-3">
          <Card>
            <CardHeader className="flex-row items-center justify-between space-y-0">
              <div>
                <CardTitle className="flex items-center gap-2 text-base">
                  <MapPin className="size-4 text-primary" /> Coverage &amp; preview
                </CardTitle>
                <CardDescription>
                  {preview
                    ? 'Generated tiles overlaid on the source footprints.'
                    : 'Source image footprints. Gaps stay empty — no blank tiles.'}
                </CardDescription>
              </div>
              {preview && (
                <Badge className="gap-1" variant="secondary">
                  <Sparkles className="size-3" /> live z/x/y
                </Badge>
              )}
            </CardHeader>
            <CardContent>
              <CoverageMap
                footprints={inspected}
                resultBounds={preview?.bounds}
                resultTilesUrl={
                  preview ? buildMartinUrl(`/${preview.sourceId}/{z}/{x}/{y}`) : undefined
                }
                showResult={!!preview}
              />
            </CardContent>
          </Card>

          {job && <ProgressCard job={job} />}

          {report && (
            <ResultCard
              onPreview={(sourceId, bounds, mbtilesPath) => {
                setPreview({ bounds, sourceId });
                if (mbtilesPath) {
                  studioApi.validate(mbtilesPath).then(setValidation).catch(() => {});
                } else {
                  setValidation(null);
                }
              }}
              previewId={preview?.sourceId}
              report={report}
              validation={validation}
            />
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// sub-components
// ---------------------------------------------------------------------------

function StudioHeader({ config }: { config: StudioConfig | null }) {
  return (
    <div className="flex flex-col items-start justify-between gap-4 md:flex-row md:items-center">
      <div>
        <h2 className="flex items-center gap-2 font-bold text-2xl text-foreground">
          <Layers className="size-6 text-primary" /> Tile Map Studio
        </h2>
        <p className="text-muted-foreground">
          Build a single, integrated <span className="font-medium text-foreground">sparse</span> tile
          map from multiple GeoTIFFs — empty areas are skipped, served at z/x/y.
        </p>
      </div>
      {config && (
        <Badge className="gap-1.5" variant={config.gdal_available ? 'secondary' : 'destructive'}>
          <span
            className={cn(
              'size-2 rounded-full',
              config.gdal_available ? 'bg-green-500' : 'bg-destructive',
            )}
          />
          {config.gdal_available ? 'GDAL ready' : 'GDAL missing'}
        </Badge>
      )}
    </div>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
}) {
  return (
    <div className="space-y-1.5">
      <label className="font-medium text-foreground text-sm">{label}</label>
      {children}
      {hint && <p className="text-muted-foreground text-xs">{hint}</p>}
    </div>
  );
}

function ToggleChip({
  active,
  label,
  icon,
  onClick,
}: {
  active: boolean;
  label: string;
  icon?: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      className={cn(
        'inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm transition-colors',
        active
          ? 'border-primary bg-primary/10 text-primary'
          : 'border-input bg-background text-muted-foreground hover:bg-accent',
      )}
      onClick={onClick}
      type="button"
    >
      {icon}
      {label}
    </button>
  );
}

function SourceCard({
  files,
  selected,
  inspected,
  inspecting,
  gapInfo,
  onToggle,
  onToggleAll,
  onUploaded,
}: {
  files: BrowseEntry[];
  selected: Set<string>;
  inspected: RasterInfo[];
  inspecting: boolean;
  gapInfo: number | null;
  onToggle: (rel: string) => void;
  onToggleAll: (all: boolean) => void;
  onUploaded: () => void;
}) {
  const { toast } = useToast();
  const [dragOver, setDragOver] = useState(false);
  const [uploading, setUploading] = useState<{ name: string; pct: number } | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const isTiff = (f: File) => /\.tiff?$/i.test(f.name);

  const uploadFiles = async (list: FileList | File[]) => {
    const tiffs = [...list].filter(isTiff);
    const rejected = [...list].length - tiffs.length;
    if (rejected > 0) {
      toast({
        description: `${rejected} file(s) skipped — only .tif/.tiff are accepted.`,
        title: 'Unsupported file',
        variant: 'destructive',
      });
    }
    for (const file of tiffs) {
      try {
        setUploading({ name: file.name, pct: 0 });
        await studioApi.upload(file, (pct) => setUploading({ name: file.name, pct }));
        toast({ description: `Uploaded ${file.name}.`, title: 'Upload complete' });
      } catch (e) {
        toast({ description: (e as Error).message, title: 'Upload failed', variant: 'destructive' });
      }
    }
    setUploading(null);
    onUploaded();
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    if (e.dataTransfer.files.length) uploadFiles(e.dataTransfer.files);
  };

  const infoByPath = useMemo(() => {
    const m = new Map<string, RasterInfo>();
    for (const i of inspected) m.set(i.file_name, i);
    return m;
  }, [inspected]);

  const allSelected = files.length > 0 && selected.size === files.length;
  const crs = inspected[0]?.crs_name;
  const sameCrs = inspected.every((i) => i.crs_name === crs);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <span className="flex size-6 items-center justify-center rounded-full bg-primary/10 text-primary text-xs">
            1
          </span>
          Source imagery
        </CardTitle>
        <CardDescription>
          {files.length} GeoTIFF{files.length === 1 ? '' : 's'} in the data directory.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        {/* drag & drop upload */}
        <button
          className={cn(
            'flex w-full flex-col items-center justify-center gap-1.5 rounded-md border-2 border-dashed p-4 text-center transition-colors',
            dragOver
              ? 'border-primary bg-primary/5'
              : 'border-border hover:border-primary/50 hover:bg-accent/40',
          )}
          onClick={() => fileInputRef.current?.click()}
          onDragLeave={() => setDragOver(false)}
          onDragOver={(e) => {
            e.preventDefault();
            setDragOver(true);
          }}
          onDrop={onDrop}
          type="button"
        >
          <UploadCloud className="size-5 text-muted-foreground" />
          <span className="font-medium text-foreground text-sm">
            Drop GeoTIFFs here, or click to browse
          </span>
          <span className="text-muted-foreground text-xs">
            Accepts <code className="text-xs">.tif</code> / <code className="text-xs">.tiff</code>
          </span>
          <input
            accept=".tif,.tiff,image/tiff"
            className="hidden"
            multiple
            onChange={(e) => {
              if (e.target.files?.length) uploadFiles(e.target.files);
              e.target.value = '';
            }}
            ref={fileInputRef}
            type="file"
          />
        </button>

        {uploading && (
          <div className="space-y-1">
            <div className="flex items-center justify-between text-xs">
              <span className="flex items-center gap-1.5 text-muted-foreground">
                <Loader2 className="size-3 animate-spin" /> Uploading {uploading.name}…
              </span>
              <span className="text-muted-foreground">{uploading.pct}%</span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-primary transition-all duration-200"
                style={{ width: `${Math.max(uploading.pct, 4)}%` }}
              />
            </div>
          </div>
        )}

        {files.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No GeoTIFFs yet. Drop <code className="text-xs">.tif</code> files above to get started.
          </p>
        ) : (
          <>
            <div className="flex items-center justify-between">
              <button
                className="text-muted-foreground text-xs hover:text-foreground"
                onClick={() => onToggleAll(!allSelected)}
                type="button"
              >
                {allSelected ? 'Clear selection' : 'Select all'}
              </button>
              <span className="text-muted-foreground text-xs">{selected.size} selected</span>
            </div>
            <div className="max-h-64 space-y-1.5 overflow-y-auto pr-1">
              {files.map((f) => {
                const info = infoByPath.get(f.name);
                const isSel = selected.has(f.rel_path);
                return (
                  <button
                    className={cn(
                      'flex w-full items-center gap-3 rounded-md border p-2.5 text-left transition-colors',
                      isSel ? 'border-primary/50 bg-primary/5' : 'border-border hover:bg-accent/50',
                    )}
                    key={f.rel_path}
                    onClick={() => onToggle(f.rel_path)}
                    type="button"
                  >
                    <div
                      className={cn(
                        'flex size-5 shrink-0 items-center justify-center rounded border',
                        isSel ? 'border-primary bg-primary text-primary-foreground' : 'border-input',
                      )}
                    >
                      {isSel && <CheckCircle2 className="size-3.5" />}
                    </div>
                    <FileImage className="size-4 shrink-0 text-muted-foreground" />
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-medium text-foreground text-sm">{f.name}</p>
                      <p className="text-muted-foreground text-xs">
                        {formatBytes(f.size)}
                        {info && (
                          <>
                            {' · '}
                            {info.epsg ? `EPSG:${info.epsg}` : info.crs_name}
                            {info.native_zoom != null && ` · ~z${info.native_zoom}`}
                          </>
                        )}
                      </p>
                    </div>
                  </button>
                );
              })}
            </div>

            {inspecting && (
              <p className="flex items-center gap-1.5 text-muted-foreground text-xs">
                <Loader2 className="size-3 animate-spin" /> Inspecting imagery…
              </p>
            )}

            {inspected.length > 0 && (
              <div className="space-y-2 rounded-md bg-muted/40 p-3 text-sm">
                <div className="flex items-center justify-between">
                  <span className="text-muted-foreground">Detected CRS</span>
                  <span className="font-medium text-foreground">
                    {sameCrs ? (crs ?? 'unknown') : 'mixed'}
                  </span>
                </div>
                <div className="flex items-center justify-between">
                  <span className="text-muted-foreground">Will reproject to</span>
                  <span className="font-medium text-foreground">Web Mercator / WGS84</span>
                </div>
                {gapInfo != null && (
                  <div className="flex items-start gap-2 border-border/60 border-t pt-2 text-xs">
                    <Sparkles className="mt-0.5 size-3.5 shrink-0 text-primary" />
                    <span className="text-foreground">
                      ~{gapInfo}% of the bounding box is empty between images — those areas will
                      produce <span className="font-semibold">no blank tiles</span>, only the index.
                    </span>
                  </div>
                )}
              </div>
            )}
          </>
        )}
      </CardContent>
    </Card>
  );
}

function ProgressCard({ job }: { job: Job }) {
  const pct = job.stage_total
    ? Math.round(((job.stage_index - 1 + (job.percent ?? 0) / 100) / job.stage_total) * 100)
    : 0;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          {job.status === 'running' && <Loader2 className="size-4 animate-spin text-primary" />}
          {job.status === 'done' && <CheckCircle2 className="size-4 text-green-500" />}
          {job.status === 'failed' && <XCircle className="size-4 text-destructive" />}
          {job.status === 'running'
            ? `Generating · stage ${job.stage_index}/${job.stage_total}`
            : job.status === 'done'
              ? 'Generation complete'
              : 'Generation failed'}
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
          <div
            className={cn(
              'h-full rounded-full transition-all duration-500',
              job.status === 'failed' ? 'bg-destructive' : 'bg-primary',
            )}
            style={{ width: `${job.status === 'done' ? 100 : Math.max(pct, 4)}%` }}
          />
        </div>
        <div className="max-h-40 overflow-y-auto rounded-md bg-zinc-950 p-3 font-mono text-xs text-zinc-300">
          {job.log.map((line, i) => (
            <div className="whitespace-pre-wrap" key={`${i}-${line}`}>
              {line}
            </div>
          ))}
          {job.error && <div className="text-red-400">error: {job.error}</div>}
        </div>
      </CardContent>
    </Card>
  );
}

function ResultCard({
  report,
  validation,
  previewId,
  onPreview,
}: {
  report: GenerateReport;
  validation: ValidationReport | null;
  previewId?: string;
  onPreview: (sourceId: string, bounds: BBox, mbtilesPath?: string) => void;
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <CheckCircle2 className="size-4 text-green-500" /> Result
        </CardTitle>
        <CardDescription>
          Generated in {report.duration_secs.toFixed(1)}s · {report.outputs.length} grid(s)
          {report.cog_output ? ' + 1 COG' : ''}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        {report.outputs.map((o) => (
          <div className="space-y-3 rounded-lg border border-border p-4" key={o.source_id}>
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="font-medium text-foreground">{gridLabel(o.grid)}</div>
              {o.grid_params ? (
                <Badge className="gap-1" variant="secondary">
                  <Globe2 className="size-3" /> OpenLayers grid
                </Badge>
              ) : (
                <Button
                  onClick={() => onPreview(o.source_id, o.bounds_wgs84, o.mbtiles_path)}
                  size="sm"
                  variant={previewId === o.source_id ? 'default' : 'outline'}
                >
                  {previewId === o.source_id ? 'Previewing' : 'Preview on map'}
                </Button>
              )}
            </div>

            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Stat label="Zoom" value={`z${o.min_zoom}–${o.max_zoom}`} />
              <Stat label="Tiles" value={o.tiles_total.toLocaleString()} />
              <Stat highlight label="Empty skipped" value={o.empty_skipped.toLocaleString()} />
              <Stat label="Size" value={formatBytes(o.file_size)} />
            </div>

            <SparsityBar output={o} />

            <CopyUrl url={buildMartinUrl(`/${o.source_id}/{z}/{x}/{y}`)} />

            {o.grid_params && <OpenLayersSnippet output={o} params={o.grid_params} />}
          </div>
        ))}

        {report.cog_output && (
          <CogResultBlock cog={report.cog_output} onPreview={onPreview} previewId={previewId} />
        )}

        {validation && <ValidationList report={validation} />}
      </CardContent>
    </Card>
  );
}

function CogResultBlock({
  cog,
  previewId,
  onPreview,
}: {
  cog: CogOutput;
  previewId?: string;
  onPreview: (sourceId: string, bounds: BBox, mbtilesPath?: string) => void;
}) {
  return (
    <div className="space-y-3 rounded-lg border border-primary/30 bg-primary/5 p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="font-medium text-foreground">Single COG file (EPSG:3857)</div>
        <Button
          onClick={() => onPreview(cog.source_id, cog.bounds_wgs84)}
          size="sm"
          variant={previewId === cog.source_id ? 'default' : 'outline'}
        >
          {previewId === cog.source_id ? 'Previewing' : 'Preview on map'}
        </Button>
      </div>
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
        <Stat label="Format" value={cog.format.toUpperCase()} />
        <Stat label="Max zoom" value={cog.max_zoom != null ? `z${cog.max_zoom}` : '—'} />
        <Stat label="Size" value={formatBytes(cog.file_size)} />
      </div>
      <p className="text-muted-foreground text-xs">
        One Cloud-Optimized GeoTIFF, served on-the-fly by Martin — no tiles stored, empty areas
        return 204.
      </p>
      <CopyUrl url={buildMartinUrl(`/${cog.source_id}/{z}/{x}/{y}`)} />
    </div>
  );
}

function Stat({
  label,
  value,
  highlight,
}: {
  label: string;
  value: string;
  highlight?: boolean;
}) {
  return (
    <div
      className={cn(
        'rounded-md border p-2.5',
        highlight ? 'border-primary/30 bg-primary/5' : 'border-border',
      )}
    >
      <p className="text-muted-foreground text-xs">{label}</p>
      <p className={cn('font-semibold', highlight ? 'text-primary' : 'text-foreground')}>{value}</p>
    </div>
  );
}

function SparsityBar({ output }: { output: GridOutput }) {
  const sparsePct = output.dense_total
    ? Math.round((output.empty_skipped / output.dense_total) * 100)
    : 0;
  return (
    <div className="space-y-1">
      <div className="flex justify-between text-xs">
        <span className="text-muted-foreground">Storage saved (empty tiles not written)</span>
        <span className="font-medium text-primary">{sparsePct}% sparse</span>
      </div>
      <div className="flex h-2.5 overflow-hidden rounded-full bg-muted">
        <div className="bg-primary" style={{ width: `${100 - sparsePct}%` }} />
        <div className="bg-primary/20" style={{ width: `${sparsePct}%` }} />
      </div>
    </div>
  );
}

function ValidationList({ report }: { report: ValidationReport }) {
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <p className="font-medium text-foreground text-sm">Output validation</p>
        <Badge variant={report.ok ? 'secondary' : 'destructive'}>
          {report.ok ? 'PASS' : 'FAIL'}
        </Badge>
      </div>
      <div className="space-y-1">
        {report.checks.map((c) => (
          <div className="flex items-start gap-2 text-sm" key={c.name}>
            {c.status === 'pass' && <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-green-500" />}
            {c.status === 'warn' && <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-500" />}
            {c.status === 'fail' && <XCircle className="mt-0.5 size-4 shrink-0 text-destructive" />}
            <span className="text-muted-foreground">
              <span className="font-medium text-foreground">{c.name}</span> — {c.detail}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function CopyUrl({ url }: { url: string }) {
  const { toast } = useToast();
  return (
    <div className="flex items-center gap-2 rounded-md bg-muted/50 p-2">
      <code className="flex-1 truncate text-muted-foreground text-xs">{url}</code>
      <Button
        onClick={() => {
          navigator.clipboard.writeText(url);
          toast({ description: 'Tile URL copied to clipboard.', title: 'Copied' });
        }}
        size="sm"
        variant="ghost"
      >
        <Copy className="size-3.5" /> Copy
      </Button>
    </div>
  );
}

function OpenLayersSnippet({ output, params }: { output: GridOutput; params: GridParams }) {
  const { toast } = useToast();
  const url = `${buildMartinUrl(`/${output.source_id}`)}/{z}/{x}/{y}`;
  const cx = (params.bounds_crs[0] + params.bounds_crs[2]) / 2;
  const cy = (params.bounds_crs[1] + params.bounds_crs[3]) / 2;
  const code = `// EPSG:${params.epsg} tiles — display with OpenLayers + proj4 (not Web Mercator)
import Map from 'ol/Map.js';
import View from 'ol/View.js';
import TileLayer from 'ol/layer/Tile.js';
import XYZ from 'ol/source/XYZ.js';
import TileGrid from 'ol/tilegrid/TileGrid.js';
import { get as getProjection } from 'ol/proj.js';
import { register } from 'ol/proj/proj4.js';
import proj4 from 'proj4';

proj4.defs('EPSG:${params.epsg}', '${params.proj4 ?? '<run: gdalsrsinfo -o proj4 EPSG:' + params.epsg + '>'}');
register(proj4);
const projection = getProjection('EPSG:${params.epsg}');
projection.setExtent([${params.bounds_crs.join(', ')}]);

const tileGrid = new TileGrid({
  origin: [${params.tile_origin.join(', ')}],
  resolutions: [${params.resolutions.join(', ')}],
  tileSize: [${params.tile_size}, ${params.tile_size}],
});

new Map({
  target: 'map',
  layers: [new TileLayer({
    extent: [${params.bounds_crs.join(', ')}],
    source: new XYZ({
      projection: 'EPSG:${params.epsg}',
      tileGrid,
      url: '${url}',
      minZoom: ${output.min_zoom}, maxZoom: ${output.max_zoom},
    }),
  })],
  view: new View({ projection, center: [${cx}, ${cy}], zoom: ${output.min_zoom} }),
});`;
  return (
    <div className="space-y-2 rounded-md border border-amber-500/30 bg-amber-500/5 p-3">
      <div className="flex items-center justify-between">
        <p className="font-medium text-foreground text-sm">OpenLayers config for this grid</p>
        <Button
          onClick={() => {
            navigator.clipboard.writeText(code);
            toast({ description: 'OpenLayers config copied.', title: 'Copied' });
          }}
          size="sm"
          variant="ghost"
        >
          <Copy className="size-3.5" /> Copy
        </Button>
      </div>
      <pre className="max-h-56 overflow-auto rounded-md bg-zinc-950 p-3 font-mono text-[11px] text-zinc-300">
        {code}
      </pre>
    </div>
  );
}
