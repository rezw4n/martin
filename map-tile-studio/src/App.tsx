import { useCallback, useEffect, useMemo, useState } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { AnimatePresence, motion } from 'motion/react';
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  FolderOpen,
  Grid3x3,
  Image as ImageIcon,
  Info,
  Layers,
  Loader2,
  Play,
  RotateCcw,
  Sparkles,
  UploadCloud,
  X,
  XCircle,
} from 'lucide-react';
import {
  gdalStatus,
  generate,
  getTileBase,
  inspect,
  pickGeoTiffs,
  revealInExplorer,
  tileUrlTemplate,
  validate,
} from '@/lib/api';
import type {
  GdalStatus,
  GenerateOptions,
  GenerateReport,
  GridOutput,
  ProgressEvent,
  RasterInfo,
  ResamplingId,
  TileFormatId,
  TileGrid,
  ValidationReport,
} from '@/lib/types';
import { cn, formatBytes } from '@/lib/utils';
import { CatalogView } from '@/components/CatalogView';
import { MapCanvas } from '@/components/MapCanvas';
import { Titlebar, type Tab } from '@/components/Titlebar';
import {
  Badge,
  Button,
  Field,
  ProgressBar,
  Segmented,
  Stat,
  TextInput,
} from '@/components/ui';

type Phase = 'idle' | 'generating' | 'done';
type PrimaryGrid = 'web-mercator' | 'geodetic';
type OutputType = 'mbtiles' | 'cog';

interface ProgressState {
  stage: string;
  index: number;
  total: number;
  percent: number | null;
  logs: string[];
}

function gridLabel(g: TileGrid): string {
  if (g === 'web-mercator') return 'Web Mercator · 3857';
  if (g === 'geodetic') return 'WGS84 · 4326';
  return `Custom · EPSG:${g.custom}`;
}

export default function App() {
  const [tab, setTab] = useState<Tab>('studio');
  const [gdal, setGdal] = useState<GdalStatus | null>(null);
  const [tileBase, setTileBase] = useState('');
  const [files, setFiles] = useState<RasterInfo[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [inspecting, setInspecting] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // output settings
  const [name, setName] = useState('my_tilemap');
  const [grid, setGrid] = useState<PrimaryGrid>('web-mercator');
  const [customOn, setCustomOn] = useState(false);
  const [customEpsg, setCustomEpsg] = useState('9680');
  const [format, setFormat] = useState<TileFormatId>('png');
  const [resampling, setResampling] = useState<ResamplingId>('bilinear');
  const [minZoom, setMinZoom] = useState('');
  const [maxZoom, setMaxZoom] = useState('');
  const [outputType, setOutputType] = useState<OutputType>('mbtiles');

  // run state
  const [phase, setPhase] = useState<Phase>('idle');
  const [progress, setProgress] = useState<ProgressState | null>(null);
  const [report, setReport] = useState<GenerateReport | null>(null);
  const [validation, setValidation] = useState<ValidationReport | null>(null);
  const [preview, setPreview] = useState<{ url: string; bounds: GridOutput['bounds_wgs84']; maxZoom: number } | null>(null);

  useEffect(() => {
    gdalStatus().then(setGdal).catch((e) => setError(String(e)));
    getTileBase().then(setTileBase).catch(() => {});
  }, []);

  const addPaths = useCallback(
    async (paths: string[]) => {
      const tiffs = paths.filter((p) => /\.tiff?$/i.test(p));
      if (!tiffs.length) return;
      setInspecting(true);
      setError(null);
      try {
        const infos = await inspect(tiffs);
        setFiles((prev) => {
          const seen = new Set(prev.map((f) => f.path));
          return [...prev, ...infos.filter((i) => !seen.has(i.path))];
        });
        setSelected((prev) => new Set([...prev, ...infos.map((i) => i.path)]));
      } catch (e) {
        setError(String(e));
      } finally {
        setInspecting(false);
      }
    },
    [],
  );

  // native OS drag & drop of files
  useEffect(() => {
    const win = getCurrentWindow();
    const un = win.onDragDropEvent((event) => {
      if (event.payload.type === 'over') setDragOver(true);
      else if (event.payload.type === 'drop') {
        setDragOver(false);
        addPaths(event.payload.paths);
      } else setDragOver(false);
    });
    return () => {
      un.then((f) => f());
    };
  }, [addPaths]);

  const pickFiles = async () => {
    const paths = await pickGeoTiffs();
    if (paths.length) addPaths(paths);
  };

  const removeFile = (path: string) => {
    setFiles((prev) => prev.filter((f) => f.path !== path));
    setSelected((prev) => {
      const next = new Set(prev);
      next.delete(path);
      return next;
    });
  };

  const toggleSelect = (path: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });

  const selectedFiles = files.filter((f) => selected.has(f.path));
  const totalSize = selectedFiles.reduce((s, f) => s + f.file_size, 0);
  const canGenerate = !!gdal?.available && selectedFiles.length > 0 && name.trim().length > 0;

  const startGenerate = async () => {
    if (!gdal || !canGenerate) return;
    const isCog = outputType === 'cog';
    // COG mode → no tile-pyramid grids, single COG file. Otherwise the chosen
    // grid (plus an optional custom projected grid) and no COG.
    const grids: TileGrid[] = isCog ? [] : [grid];
    if (!isCog && customOn && customEpsg) grids.push({ custom: Number(customEpsg) });

    const opts: GenerateOptions = {
      inputs: selectedFiles.map((f) => f.path),
      output_dir: gdal.output_dir,
      name: name.trim(),
      grids,
      min_zoom: isCog || !minZoom ? null : Number(minZoom),
      max_zoom: isCog || !maxZoom ? null : Number(maxZoom),
      format,
      resampling,
      cog: isCog,
      keep_intermediate: false,
    };

    setPhase('generating');
    setError(null);
    setProgress({ stage: 'starting', index: 0, total: 1, percent: null, logs: [] });
    setReport(null);
    setValidation(null);
    setPreview(null);

    const onProgress = (ev: ProgressEvent) => {
      setProgress((p) => {
        const base = p ?? { stage: '', index: 0, total: 1, percent: null, logs: [] };
        if (ev.kind === 'stage')
          return { ...base, stage: ev.stage, index: ev.index, total: ev.total, percent: null };
        if (ev.kind === 'percent') return { ...base, percent: ev.percent };
        if (ev.kind === 'log')
          return { ...base, logs: [...base.logs, ev.message].slice(-120) };
        return base;
      });
    };

    try {
      const rep = await generate(opts, onProgress);
      setReport(rep);
      const previewOut =
        rep.outputs.find((o) => o.grid === 'web-mercator') ??
        rep.outputs.find((o) => o.grid === 'geodetic') ??
        rep.outputs[0];
      if (previewOut) {
        setPreview({
          url: tileUrlTemplate(tileBase, previewOut.source_id),
          bounds: previewOut.bounds_wgs84,
          maxZoom: previewOut.max_zoom,
        });
        validate(previewOut.mbtiles_path).then(setValidation).catch(() => {});
      }
      setPhase('done');
    } catch (e) {
      setError(String(e));
      setPhase('idle');
    }
  };

  const reset = () => {
    setPhase('idle');
    setReport(null);
    setValidation(null);
    setPreview(null);
    setProgress(null);
  };

  const statusPill = useMemo(() => {
    if (!gdal)
      return (
        <Badge tone="neutral">
          <Loader2 className="size-3 animate-spin" /> starting…
        </Badge>
      );
    return gdal.available ? (
      <Badge tone="ok">
        <span className="size-1.5 rounded-full bg-ok mts-pulse" /> Engine ready
      </Badge>
    ) : (
      <Badge tone="danger">
        <AlertTriangle className="size-3" /> GDAL not found
      </Badge>
    );
  }, [gdal]);

  return (
    <div className="flex h-full flex-col bg-white text-ink">
      <Titlebar onTab={setTab} status={statusPill} tab={tab} />

      {tab === 'catalog' && (
        <CatalogView onOpenInStudio={() => setTab('studio')} tileBase={tileBase} />
      )}

      <div className={cn('min-h-0 flex-1', tab === 'catalog' ? 'hidden' : 'flex')}>
        {/* ── docked panel ─────────────────────────────────────────────── */}
        <aside className="flex w-[400px] flex-none flex-col border-r border-line bg-white">
          <AnimatePresence initial={false} mode="wait">
            {phase === 'idle' && (
              <motion.div
                animate={{ opacity: 1, x: 0 }}
                className="flex min-h-0 flex-1 flex-col"
                exit={{ opacity: 0, x: -12 }}
                initial={{ opacity: 0, x: 12 }}
                key="form"
                transition={{ duration: 0.22, ease: 'easeOut' }}
              >
                <PanelHeader />
                <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-6 py-5">
                  <SourceSection
                    dragOver={dragOver}
                    files={files}
                    inspecting={inspecting}
                    onPick={pickFiles}
                    onRemove={removeFile}
                    onToggle={toggleSelect}
                    selected={selected}
                  />
                  <OutputSection
                    customEpsg={customEpsg}
                    customOn={customOn}
                    format={format}
                    grid={grid}
                    maxZoom={maxZoom}
                    minZoom={minZoom}
                    name={name}
                    outputType={outputType}
                    resampling={resampling}
                    setCustomEpsg={setCustomEpsg}
                    setCustomOn={setCustomOn}
                    setFormat={setFormat}
                    setGrid={setGrid}
                    setMaxZoom={setMaxZoom}
                    setMinZoom={setMinZoom}
                    setName={setName}
                    setOutputType={setOutputType}
                    setResampling={setResampling}
                  />
                </div>
                <div className="flex-none border-t border-line-soft px-6 py-4">
                  {error && (
                    <div className="mb-3 flex items-start gap-2 rounded-lg bg-danger-tint px-3 py-2 text-[12px] text-[#b42318]">
                      <AlertTriangle className="mt-px size-3.5 flex-none" />
                      <span className="leading-snug">{error}</span>
                    </div>
                  )}
                  <div className="mb-2.5 flex items-center justify-between text-[11.5px] text-muted">
                    <span>
                      {selectedFiles.length} image{selectedFiles.length === 1 ? '' : 's'} ·{' '}
                      {formatBytes(totalSize)}
                    </span>
                    <span className="font-mono">
                      {outputType === 'cog' ? 'Single COG · GeoTIFF' : gridLabel(grid)}
                    </span>
                  </div>
                  <Button
                    className="w-full"
                    disabled={!canGenerate}
                    onClick={startGenerate}
                    size="lg"
                    variant="primary"
                  >
                    <Play className="size-[15px] fill-current" /> Generate tile map
                  </Button>
                </div>
              </motion.div>
            )}

            {phase === 'generating' && progress && (
              <motion.div
                animate={{ opacity: 1, x: 0 }}
                className="flex min-h-0 flex-1 flex-col"
                exit={{ opacity: 0, x: -12 }}
                initial={{ opacity: 0, x: 12 }}
                key="progress"
                transition={{ duration: 0.22 }}
              >
                <ProgressView name={name} progress={progress} />
              </motion.div>
            )}

            {phase === 'done' && report && (
              <motion.div
                animate={{ opacity: 1, x: 0 }}
                className="flex min-h-0 flex-1 flex-col"
                exit={{ opacity: 0, x: -12 }}
                initial={{ opacity: 0, x: 12 }}
                key="result"
                transition={{ duration: 0.22 }}
              >
                <ResultView
                  onNew={reset}
                  onReveal={(p) => revealInExplorer(p)}
                  report={report}
                  tileBase={tileBase}
                  validation={validation}
                />
              </motion.div>
            )}
          </AnimatePresence>
        </aside>

        {/* ── map canvas ───────────────────────────────────────────────── */}
        <main className="relative min-w-0 flex-1">
          <MapCanvas footprints={files} preview={preview} selectedPaths={selected} />
          <AnimatePresence>
            {dragOver && (
              <motion.div
                animate={{ opacity: 1 }}
                className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center bg-brand/8 backdrop-blur-[2px]"
                exit={{ opacity: 0 }}
                initial={{ opacity: 0 }}
              >
                <div className="flex flex-col items-center gap-3 rounded-2xl border-2 border-dashed border-brand bg-white/90 px-12 py-10 shadow-xl">
                  <UploadCloud className="size-9 text-brand" />
                  <div className="font-semibold text-[15px] text-ink">Drop GeoTIFFs to add</div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </main>
      </div>
    </div>
  );
}

/* ── panel header ─────────────────────────────────────────────────────── */
function PanelHeader() {
  return (
    <div className="flex-none border-b border-line-soft px-6 pt-5 pb-4">
      <div className="font-semibold text-[18px] tracking-[-0.02em] text-ink">Build a tile map</div>
      <p className="mt-1.5 text-[12.5px] leading-relaxed text-muted">
        Stitch GeoTIFFs into one sparse tile map — empty areas stay tile-free.
      </p>
    </div>
  );
}

function StepDot({ n }: { n: number }) {
  return (
    <span className="flex size-[22px] flex-none items-center justify-center rounded-full bg-brand-tint font-semibold text-[11px] text-brand">
      {n}
    </span>
  );
}

/* ── source section ───────────────────────────────────────────────────── */
function SourceSection({
  files,
  selected,
  inspecting,
  dragOver,
  onPick,
  onToggle,
  onRemove,
}: {
  files: RasterInfo[];
  selected: Set<string>;
  inspecting: boolean;
  dragOver: boolean;
  onPick: () => void;
  onToggle: (p: string) => void;
  onRemove: (p: string) => void;
}) {
  return (
    <section>
      <div className="mb-3 flex items-center gap-2.5">
        <StepDot n={1} />
        <span className="font-semibold text-[13px] text-ink">Source imagery</span>
        <span className="flex-1" />
        {inspecting && (
          <span className="flex items-center gap-1.5 text-[11px] text-muted">
            <Loader2 className="size-3 animate-spin" /> reading…
          </span>
        )}
      </div>

      <button
        className={cn(
          'flex w-full flex-col items-center gap-1.5 rounded-xl border-[1.5px] border-dashed p-4 text-center transition-colors',
          dragOver
            ? 'border-brand bg-brand-tint/40'
            : 'border-[#cfd6e0] bg-[#fafbfd] hover:border-brand/60 hover:bg-[#f4f8ff]',
        )}
        onClick={onPick}
        type="button"
      >
        <UploadCloud className="size-5 text-faint" />
        <span className="font-medium text-[12.5px] text-ink-soft">Add GeoTIFFs</span>
        <span className="text-[11px] text-faint">click or drop · .tif / .tiff</span>
      </button>

      {files.length > 0 && (
        <div className="mt-3 flex flex-col gap-1.5">
          {files.map((f) => {
            const on = selected.has(f.path);
            return (
              <div
                className={cn(
                  'group flex items-center gap-2.5 rounded-[10px] border px-2.5 py-2 transition-colors',
                  on ? 'border-[#dbe5fb] bg-[#f6f9ff]' : 'border-[#ebedf0] bg-white',
                )}
                key={f.path}
              >
                <button
                  className={cn(
                    'flex size-[18px] flex-none items-center justify-center rounded-[5px] border transition-colors',
                    on ? 'border-brand bg-brand' : 'border-[#cfd6e0] bg-white',
                  )}
                  onClick={() => onToggle(f.path)}
                  type="button"
                >
                  {on && <CheckCircle2 className="size-3 text-white" strokeWidth={3} />}
                </button>
                <div className="min-w-0 flex-1">
                  <div className="truncate font-mono text-[12px] text-ink">{f.file_name}</div>
                  <div className="mt-px flex items-center gap-1.5 text-[10.5px] text-faint">
                    <span>{formatBytes(f.file_size)}</span>
                    <span>·</span>
                    <span className="truncate">
                      {f.epsg ? `EPSG:${f.epsg}` : (f.crs_name ?? 'unknown CRS')}
                    </span>
                  </div>
                </div>
                <button
                  className="flex size-6 flex-none items-center justify-center rounded-md text-faint opacity-0 transition-all hover:bg-danger-tint hover:text-danger group-hover:opacity-100"
                  onClick={() => onRemove(f.path)}
                  type="button"
                >
                  <X className="size-3.5" />
                </button>
              </div>
            );
          })}
        </div>
      )}
      {files.length === 0 && !inspecting && (
        <p className="mt-3 flex items-center gap-1.5 text-[11.5px] text-faint">
          <ImageIcon className="size-3.5" /> No imagery yet — add one or more GeoTIFFs.
        </p>
      )}
    </section>
  );
}

/* ── output section ───────────────────────────────────────────────────── */
function OutputSection(props: {
  name: string;
  setName: (v: string) => void;
  outputType: OutputType;
  setOutputType: (v: OutputType) => void;
  grid: PrimaryGrid;
  setGrid: (v: PrimaryGrid) => void;
  customOn: boolean;
  setCustomOn: (v: boolean) => void;
  customEpsg: string;
  setCustomEpsg: (v: string) => void;
  format: TileFormatId;
  setFormat: (v: TileFormatId) => void;
  resampling: ResamplingId;
  setResampling: (v: ResamplingId) => void;
  minZoom: string;
  setMinZoom: (v: string) => void;
  maxZoom: string;
  setMaxZoom: (v: string) => void;
}) {
  const isCog = props.outputType === 'cog';
  return (
    <section className="flex flex-col gap-4">
      <div className="flex items-center gap-2.5">
        <StepDot n={2} />
        <span className="font-semibold text-[13px] text-ink">Output settings</span>
      </div>

      <Field hint={isCog ? 'one GeoTIFF' : 'tile pyramid'} label="Output type">
        <div className="grid grid-cols-2 gap-2">
          <OutputTypeCard
            active={!isCog}
            desc="Sparse z/x/y tiles in an MBTiles file"
            icon={<Grid3x3 className="size-4" />}
            onClick={() => props.setOutputType('mbtiles')}
            title="Tile map"
          />
          <OutputTypeCard
            active={isCog}
            desc="One Cloud-Optimized GeoTIFF"
            icon={<Layers className="size-4" />}
            onClick={() => props.setOutputType('cog')}
            title="Single COG"
          />
        </div>
      </Field>

      <Field label="Map name">
        <TextInput
          mono
          onChange={(e) => props.setName(e.target.value)}
          placeholder="my_tilemap"
          value={props.name}
        />
      </Field>

      <AnimatePresence initial={false} mode="wait">
        {isCog ? (
          <motion.div
            animate={{ opacity: 1, height: 'auto' }}
            className="overflow-hidden"
            exit={{ opacity: 0, height: 0 }}
            initial={{ opacity: 0, height: 0 }}
            key="cog"
            transition={{ duration: 0.18 }}
          >
            <div className="flex items-start gap-2 rounded-[11px] border border-brand/20 bg-brand-tint/40 px-3 py-2.5 text-[11.5px] leading-snug text-ink-soft">
              <Info className="mt-px size-3.5 flex-none text-brand" />
              A single overview-pyramided GeoTIFF in Web Mercator — zoom levels are derived
              automatically. No tile-grid choice needed.
            </div>
          </motion.div>
        ) : (
          <motion.div
            animate={{ opacity: 1, height: 'auto' }}
            className="flex flex-col gap-4 overflow-hidden"
            exit={{ opacity: 0, height: 0 }}
            initial={{ opacity: 0, height: 0 }}
            key="mbtiles"
            transition={{ duration: 0.18 }}
          >
            <Field label="Coordinate system">
              <Segmented
                onChange={props.setGrid}
                options={[
                  { value: 'web-mercator', label: 'Web Mercator' },
                  { value: 'geodetic', label: 'WGS84' },
                ]}
                value={props.grid}
              />
            </Field>

            <div className="space-y-2 rounded-[11px] border border-line bg-[#fbfcfd] p-2.5">
              <label className="flex cursor-pointer items-center gap-2.5">
                <input
                  checked={props.customOn}
                  className="size-4 accent-[#2463eb]"
                  onChange={(e) => props.setCustomOn(e.target.checked)}
                  type="checkbox"
                />
                <span className="font-medium text-[12.5px] text-ink">
                  Add a custom projected grid
                </span>
              </label>
              {props.customOn && (
                <>
                  <div className="flex items-center gap-2 pl-6">
                    <span className="text-[12px] text-muted">EPSG:</span>
                    <TextInput
                      className="h-9 w-28"
                      mono
                      onChange={(e) => props.setCustomEpsg(e.target.value.replace(/[^0-9]/g, ''))}
                      placeholder="9680"
                      value={props.customEpsg}
                    />
                  </div>
                  <p className="flex items-start gap-1.5 pl-6 text-[11px] leading-snug text-[#b54708]">
                    <Info className="mt-px size-3.5 flex-none" />
                    Custom-grid tiles are not Web Mercator — display them with OpenLayers (a config
                    is provided after generation).
                  </p>
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      <div className="grid grid-cols-2 gap-3">
        <Field label="Tile format">
          <Segmented
            onChange={props.setFormat}
            options={[
              { value: 'png', label: 'PNG' },
              { value: 'webp', label: 'WEBP' },
            ]}
            value={props.format}
          />
        </Field>
        <Field label="Resampling">
          <div className="relative">
            <select
              className="h-10 w-full appearance-none rounded-[9px] border border-[#dfe3e9] bg-white px-3 pr-8 text-[13px] text-ink outline-none focus:border-brand focus:ring-2 focus:ring-brand/15"
              onChange={(e) => props.setResampling(e.target.value as ResamplingId)}
              value={props.resampling}
            >
              {(['near', 'bilinear', 'cubic', 'average', 'lanczos'] as ResamplingId[]).map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
            <svg
              className="pointer-events-none absolute top-1/2 right-3 -translate-y-1/2 text-faint"
              fill="none"
              height="14"
              stroke="currentColor"
              strokeWidth="2"
              viewBox="0 0 24 24"
              width="14"
            >
              <polyline points="6 9 12 15 18 9" />
            </svg>
          </div>
        </Field>
      </div>

      {!isCog && (
        <Field hint="blank = auto" label="Zoom range">
          <div className="flex items-center gap-2.5">
            <TextInput
              mono
              onChange={(e) => props.setMinZoom(e.target.value.replace(/[^0-9]/g, ''))}
              placeholder="min"
              value={props.minZoom}
            />
            <span className="text-faint">–</span>
            <TextInput
              mono
              onChange={(e) => props.setMaxZoom(e.target.value.replace(/[^0-9]/g, ''))}
              placeholder="max"
              value={props.maxZoom}
            />
          </div>
        </Field>
      )}
    </section>
  );
}

function OutputTypeCard({
  active,
  icon,
  title,
  desc,
  onClick,
}: {
  active: boolean;
  icon: React.ReactNode;
  title: string;
  desc: string;
  onClick: () => void;
}) {
  return (
    <button
      className={cn(
        'flex flex-col gap-1.5 rounded-[11px] border p-3 text-left transition-all',
        active
          ? 'border-brand bg-brand-tint/50 shadow-[0_1px_2px_rgba(36,99,235,.12)]'
          : 'border-line bg-white hover:border-[#cdd5e0] hover:bg-[#fafbfd]',
      )}
      onClick={onClick}
      type="button"
    >
      <span className={cn('flex items-center gap-1.5', active ? 'text-brand' : 'text-muted')}>
        {icon}
        <span className="font-semibold text-[12.5px] text-ink">{title}</span>
      </span>
      <span className="text-[11px] leading-snug text-faint">{desc}</span>
    </button>
  );
}

/* ── progress view ────────────────────────────────────────────────────── */
function ProgressView({ name, progress }: { name: string; progress: ProgressState }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex-none border-b border-line-soft px-6 pt-5 pb-4">
        <div className="flex items-center gap-2.5">
          <span className="relative flex size-7 items-center justify-center">
            <Loader2 className="size-7 animate-spin text-brand" strokeWidth={1.6} />
          </span>
          <div>
            <div className="font-semibold text-[16px] tracking-[-0.01em] text-ink">
              Generating…
            </div>
            <div className="font-mono text-[11.5px] text-muted">{name}</div>
          </div>
        </div>
      </div>

      <div className="flex-none px-6 pt-5">
        <div className="mb-2 flex items-center justify-between text-[12px]">
          <span className="font-medium text-ink-soft capitalize">
            {progress.stage.replace(/_/g, ' ')}
          </span>
          <span className="font-mono text-muted">
            step {progress.index}/{progress.total}
            {progress.percent != null ? ` · ${Math.round(progress.percent)}%` : ''}
          </span>
        </div>
        <ProgressBar percent={progress.percent} />
      </div>

      <div className="mt-5 flex min-h-0 flex-1 flex-col px-6 pb-6">
        <div className="mb-2 font-semibold text-[10.5px] uppercase tracking-[0.04em] text-faint">
          Engine log
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto rounded-[10px] border border-line bg-[#0f1115] p-3 font-mono text-[10.5px] leading-relaxed text-[#9aa6b6]">
          {progress.logs.length === 0 ? (
            <span className="text-[#5b6573]">waiting for the engine…</span>
          ) : (
            progress.logs.map((l, i) => (
              <div className="whitespace-pre-wrap break-words" key={i}>
                {l}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

/* ── result view ──────────────────────────────────────────────────────── */
function ResultView({
  report,
  validation,
  onNew,
  onReveal,
  tileBase,
}: {
  report: GenerateReport;
  validation: ValidationReport | null;
  onNew: () => void;
  onReveal: (path: string) => void;
  tileBase: string;
}) {
  const folder = report.outputs[0]?.mbtiles_path ?? report.cog_output?.cog_path;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex-none border-b border-line-soft px-6 pt-5 pb-4">
        <div className="flex items-center gap-2.5">
          <CheckCircle2 className="size-6 text-ok" />
          <div>
            <div className="font-semibold text-[16px] tracking-[-0.01em] text-ink">
              Tile map ready
            </div>
            <div className="text-[11.5px] text-muted">
              {report.outputs.length} grid{report.outputs.length === 1 ? '' : 's'}
              {report.cog_output ? ' + 1 COG' : ''} · {report.duration_secs.toFixed(1)}s
            </div>
          </div>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-6 py-5">
        {report.outputs.map((o) => (
          <OutputCard key={o.source_id} output={o} tileBase={tileBase} />
        ))}

        {report.cog_output && (
          <div className="rounded-[12px] border border-brand/25 bg-brand-tint/40 p-4">
            <div className="mb-2 flex items-center gap-2">
              <Layers className="size-4 text-brand" />
              <span className="font-semibold text-[13px] text-ink">Single COG file</span>
            </div>
            <div className="grid grid-cols-3 gap-2">
              <Stat label="Format" value={report.cog_output.format.toUpperCase()} />
              <Stat
                label="Max z"
                value={report.cog_output.max_zoom != null ? `z${report.cog_output.max_zoom}` : '—'}
              />
              <Stat label="Size" value={formatBytes(report.cog_output.file_size)} />
            </div>
          </div>
        )}

        {validation && <ValidationCard report={validation} />}
      </div>

      <div className="flex-none border-t border-line-soft px-6 py-4">
        <div className="flex gap-2.5">
          <Button className="flex-1" onClick={() => folder && onReveal(folder)} variant="secondary">
            <FolderOpen className="size-4" /> Open folder
          </Button>
          <Button className="flex-1" onClick={onNew} variant="primary">
            <RotateCcw className="size-4" /> New map
          </Button>
        </div>
      </div>
    </div>
  );
}

function OutputCard({ output: o, tileBase }: { output: GridOutput; tileBase: string }) {
  const sparsity = o.dense_total > 0 ? o.empty_skipped / o.dense_total : 0;
  const url = tileBase ? tileUrlTemplate(tileBase, o.source_id) : `/${o.source_id}/{z}/{x}/{y}`;
  return (
    <div className="rounded-[12px] border border-line bg-white p-4 shadow-[0_1px_2px_rgba(16,24,40,.04)]">
      <div className="mb-3 flex items-center justify-between">
        <span className="font-semibold text-[13px] text-ink">{gridLabel(o.grid)}</span>
        <Badge tone="brand">
          z{o.min_zoom}–{o.max_zoom}
        </Badge>
      </div>
      <div className="grid grid-cols-3 gap-2">
        <Stat label="Tiles" value={o.tiles_total.toLocaleString()} />
        <Stat label="Sparse" value={`${Math.round(sparsity * 100)}%`} />
        <Stat label="Size" value={formatBytes(o.file_size)} />
      </div>
      {o.dense_total > 0 && (
        <div className="mt-3">
          <div className="mb-1 flex items-center justify-between text-[10.5px] text-faint">
            <span>{o.empty_skipped.toLocaleString()} empty tiles skipped</span>
            <span>{Math.round(sparsity * 100)}% saved</span>
          </div>
          <div className="flex h-1.5 overflow-hidden rounded-full bg-[#eef1f5]">
            <span className="h-full bg-brand" style={{ width: `${(1 - sparsity) * 100}%` }} />
            <span className="h-full bg-[#dbe5fb]" style={{ width: `${sparsity * 100}%` }} />
          </div>
        </div>
      )}
      <CopyRow text={url} />
    </div>
  );
}

function CopyRow({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="mt-3 flex w-full items-center gap-2 rounded-[9px] bg-[#f6f7f9] px-2.5 py-2 text-left transition-colors hover:bg-[#eef0f3]"
      onClick={() => {
        navigator.clipboard?.writeText(text).catch(() => {});
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      }}
      type="button"
    >
      <code className="flex-1 truncate font-mono text-[11px] text-muted">{text}</code>
      {copied ? (
        <CheckCircle2 className="size-3.5 flex-none text-ok" />
      ) : (
        <Copy className="size-3.5 flex-none text-faint" />
      )}
    </button>
  );
}

function ValidationCard({ report }: { report: ValidationReport }) {
  return (
    <div className="rounded-[12px] border border-line bg-white p-4">
      <div className="mb-3 flex items-center gap-2">
        <Sparkles className="size-4 text-brand" />
        <span className="font-semibold text-[13px] text-ink">Validation</span>
        <span className="flex-1" />
        <Badge tone={report.ok ? 'ok' : 'warn'}>{report.ok ? 'PASS' : 'CHECK'}</Badge>
      </div>
      <div className="flex flex-col gap-1.5">
        {report.checks.map((c) => (
          <div className="flex items-start gap-2 text-[12px]" key={c.name}>
            {c.status === 'pass' ? (
              <CheckCircle2 className="mt-px size-3.5 flex-none text-ok" />
            ) : c.status === 'warn' ? (
              <AlertTriangle className="mt-px size-3.5 flex-none text-warn" />
            ) : (
              <XCircle className="mt-px size-3.5 flex-none text-danger" />
            )}
            <div className="min-w-0">
              <span className="font-medium text-ink-soft">{c.name}</span>
              <span className="ml-1.5 text-muted">{c.detail}</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
