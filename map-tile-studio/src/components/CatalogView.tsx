import { useEffect, useMemo, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import {
  ArrowLeft,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  FolderOpen,
  ImageOff,
  Layers,
  Link2,
  Loader2,
  RefreshCw,
  Search,
  Trash2,
  Upload,
} from 'lucide-react';
import {
  deleteMaps,
  importMaps,
  listMaps,
  revealInExplorer,
  tileThumbUrl,
  tileUrlTemplate,
} from '@/lib/api';
import type { BBox, MapEntry } from '@/lib/types';
import { cn, formatBytes } from '@/lib/utils';
import { MapCanvas } from '@/components/MapCanvas';
import { Badge, Button } from '@/components/ui';

/** Build a MapCanvas tile-preview from a catalog entry (MBTiles only — a COG
 *  isn't a tile pyramid the XYZ server can serve). */
function entryPreview(
  base: string,
  e: MapEntry,
): { url: string; bounds: BBox; maxZoom: number } | null {
  if (!base || e.kind !== 'mbtiles' || !e.bounds || e.max_zoom == null) return null;
  const [w, s, ee, n] = e.bounds;
  return {
    url: tileUrlTemplate(base, e.source_id),
    bounds: { min_x: w, min_y: s, max_x: ee, max_y: n },
    maxZoom: e.max_zoom,
  };
}

/** The copyable XYZ tile URL for an entry (null for COG / before server ready). */
function entryTileUrl(base: string, e: MapEntry): string | null {
  if (!base || e.kind !== 'mbtiles') return null;
  return tileUrlTemplate(base, e.source_id);
}

/** A small copy-to-clipboard control showing a tile URL. */
function CopyUrl({ url, className }: { url: string; className?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className={cn(
        'group/url flex items-center gap-2 rounded-lg border border-line bg-[#f7f8fa] py-1.5 pr-2 pl-2.5 text-left transition-colors hover:border-[#cdd5e0] hover:bg-[#f1f2f5]',
        className,
      )}
      onClick={(e) => {
        e.stopPropagation();
        navigator.clipboard?.writeText(url).catch(() => {});
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      }}
      title="Copy tile URL"
      type="button"
    >
      <Link2 className="size-3.5 flex-none text-faint" />
      <code className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">{url}</code>
      {copied ? (
        <Check className="size-3.5 flex-none text-ok" />
      ) : (
        <Copy className="size-3.5 flex-none text-faint transition-colors group-hover/url:text-ink-soft" />
      )}
    </button>
  );
}

const PER_PAGE = 12;
type Sort = 'recent' | 'name' | 'size';

function relTime(unixSecs: number): string {
  if (!unixSecs) return '';
  const s = Math.max(0, Date.now() / 1000 - unixSecs);
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return `${Math.floor(s / (86400 * 30))}mo ago`;
}

export function CatalogView({
  onOpenInStudio,
  tileBase,
}: {
  onOpenInStudio?: () => void;
  tileBase: string;
}) {
  const [maps, setMaps] = useState<MapEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');
  const [sort, setSort] = useState<Sort>('recent');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [page, setPage] = useState(0);
  const [busy, setBusy] = useState(false);
  const [preview, setPreview] = useState<MapEntry | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      setMaps(await listMaps());
    } catch {
      setMaps([]);
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    load();
  }, []);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    const list = maps.filter((m) => !q || m.name.toLowerCase().includes(q));
    const sorted = [...list];
    if (sort === 'recent') sorted.sort((a, b) => b.modified - a.modified);
    else if (sort === 'name') sorted.sort((a, b) => a.name.localeCompare(b.name));
    else sorted.sort((a, b) => b.size - a.size);
    return sorted;
  }, [maps, search, sort]);

  const pages = Math.max(1, Math.ceil(filtered.length / PER_PAGE));
  const pageClamped = Math.min(page, pages - 1);
  const paged = filtered.slice(pageClamped * PER_PAGE, pageClamped * PER_PAGE + PER_PAGE);

  const totalSize = maps.reduce((s, m) => s + m.size, 0);
  const selectedList = filtered.filter((m) => selected.has(m.path));
  const selectedSize = selectedList.reduce((s, m) => s + m.size, 0);

  const toggle = (path: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(path) ? next.delete(path) : next.add(path);
      return next;
    });

  const onImport = async () => {
    setBusy(true);
    try {
      const n = await importMaps();
      if (n) await load();
    } finally {
      setBusy(false);
    }
  };

  const onDelete = async (paths: string[]) => {
    if (!paths.length) return;
    setBusy(true);
    try {
      await deleteMaps(paths);
      setSelected(new Set());
      await load();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-canvas">
      {/* header */}
      <div className="flex flex-none items-center gap-4 border-b border-line bg-white px-7 py-4">
        <div>
          <div className="font-semibold text-[18px] tracking-[-0.02em] text-ink">Tiles Catalog</div>
          <div className="mt-0.5 text-[12px] text-muted">
            {maps.length} tile map{maps.length === 1 ? '' : 's'} · {formatBytes(totalSize)}
          </div>
        </div>
        <div className="flex-1" />
        <div className="relative">
          <Search className="-translate-y-1/2 pointer-events-none absolute top-1/2 left-3 size-3.5 text-faint" />
          <input
            className="h-9 w-64 rounded-[9px] border border-[#dfe3e9] bg-white pr-3 pl-9 text-[12.5px] text-ink outline-none placeholder:text-faint focus:border-brand focus:ring-2 focus:ring-brand/15"
            onChange={(e) => {
              setSearch(e.target.value);
              setPage(0);
            }}
            placeholder="Search tile maps…"
            value={search}
          />
        </div>
        <select
          className="h-9 rounded-[9px] border border-[#dfe3e9] bg-white px-3 text-[12.5px] text-ink-soft outline-none focus:border-brand"
          onChange={(e) => setSort(e.target.value as Sort)}
          value={sort}
        >
          <option value="recent">Recent</option>
          <option value="name">Name</option>
          <option value="size">Size</option>
        </select>
        <Button onClick={load} size="sm" variant="ghost">
          <RefreshCw className={cn('size-3.5', loading && 'animate-spin')} />
        </Button>
        <Button busy={busy} onClick={onImport} size="sm" variant="primary">
          <Upload className="size-4" /> Import tile map
        </Button>
      </div>

      {/* selection bar */}
      <AnimatePresence>
        {selected.size > 0 && (
          <motion.div
            animate={{ height: 'auto', opacity: 1 }}
            className="flex flex-none items-center gap-3 overflow-hidden border-b border-[#dbe5fb] bg-[#f6f9ff] px-7"
            exit={{ height: 0, opacity: 0 }}
            initial={{ height: 0, opacity: 0 }}
          >
            <div className="flex items-center gap-3 py-2.5">
              <span className="font-semibold text-[13px] text-brand">{selected.size} selected</span>
              <span className="text-[12px] text-muted">· {formatBytes(selectedSize)}</span>
            </div>
            <div className="flex-1" />
            <Button
              onClick={() => onDelete(selectedList.map((m) => m.path))}
              size="sm"
              variant="secondary"
            >
              <Trash2 className="size-3.5 text-danger" /> Delete
            </Button>
            <Button onClick={() => setSelected(new Set())} size="sm" variant="ghost">
              Deselect
            </Button>
          </motion.div>
        )}
      </AnimatePresence>

      {/* grid */}
      <div className="min-h-0 flex-1 overflow-y-auto px-7 py-6">
        {loading ? (
          <div className="flex h-full items-center justify-center text-muted">
            <Loader2 className="size-5 animate-spin" />
          </div>
        ) : filtered.length === 0 ? (
          <EmptyState onImport={onImport} onStudio={onOpenInStudio} searching={!!search} />
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-4">
            {paged.map((m) => (
              <MapCard
                entry={m}
                key={m.path}
                onDelete={() => onDelete([m.path])}
                onOpen={() => setPreview(m)}
                onReveal={() => revealInExplorer(m.path)}
                onToggle={() => toggle(m.path)}
                selected={selected.has(m.path)}
                tileBase={tileBase}
              />
            ))}
          </div>
        )}
      </div>

      {/* pagination */}
      {pages > 1 && (
        <div className="flex flex-none items-center justify-center gap-2 border-t border-line bg-white py-2.5">
          <Button
            disabled={pageClamped === 0}
            onClick={() => setPage(pageClamped - 1)}
            size="sm"
            variant="ghost"
          >
            <ChevronLeft className="size-4" />
          </Button>
          <span className="font-mono text-[12px] text-muted tabular-nums">
            {pageClamped + 1} / {pages}
          </span>
          <Button
            disabled={pageClamped >= pages - 1}
            onClick={() => setPage(pageClamped + 1)}
            size="sm"
            variant="ghost"
          >
            <ChevronRight className="size-4" />
          </Button>
        </div>
      )}

      {/* preview overlay */}
      <AnimatePresence>
        {preview && (
          <PreviewOverlay entry={preview} onClose={() => setPreview(null)} tileBase={tileBase} />
        )}
      </AnimatePresence>
    </div>
  );
}

function PreviewOverlay({
  entry,
  onClose,
  tileBase,
}: {
  entry: MapEntry;
  onClose: () => void;
  tileBase: string;
}) {
  const pv = entryPreview(tileBase, entry);
  const url = entryTileUrl(tileBase, entry);
  const probe = tileThumbUrl(tileBase, entry); // a concrete z/x/y tile URL
  const [diag, setDiag] = useState<string>('');
  const [imgOk, setImgOk] = useState<boolean | null>(null);
  useEffect(() => {
    if (!probe) {
      setDiag('no tile url');
      return;
    }
    setDiag('fetching…');
    fetch(probe)
      .then((r) => setDiag(`fetch ${r.status} ${r.headers.get('content-type') ?? ''}`))
      .catch((e) => setDiag(`fetch ERROR: ${String(e)}`));
  }, [probe]);
  return (
    <motion.div
      animate={{ opacity: 1 }}
      className="absolute inset-0 z-40 flex flex-col bg-white"
      exit={{ opacity: 0 }}
      initial={{ opacity: 0 }}
      transition={{ duration: 0.18 }}
    >
      <div className="flex flex-none items-center gap-3 border-b border-line px-5 py-3">
        <button
          className="flex size-9 flex-none items-center justify-center rounded-lg text-muted transition-colors hover:bg-[#f1f2f5] hover:text-ink"
          onClick={onClose}
          title="Back to catalog"
          type="button"
        >
          <ArrowLeft className="size-[18px]" />
        </button>
        <div className="min-w-0">
          <div className="truncate font-semibold text-[15px] tracking-[-0.01em] text-ink">
            {entry.name}
          </div>
          <div className="mt-0.5 flex items-center gap-1.5">
            {entry.min_zoom != null && entry.max_zoom != null && (
              <Badge tone="neutral">
                z{entry.min_zoom}–{entry.max_zoom}
              </Badge>
            )}
            {entry.tiles_total != null && (
              <Badge tone="neutral">{entry.tiles_total.toLocaleString()} tiles</Badge>
            )}
            <Badge tone="neutral">{formatBytes(entry.size)}</Badge>
            <Badge tone="neutral">{entry.crs ?? (entry.kind === 'cog' ? 'COG' : 'EPSG:3857')}</Badge>
          </div>
        </div>
        <span className="flex-1" />
        {url && <CopyUrl className="max-w-[440px]" url={url} />}
        <Button onClick={() => revealInExplorer(entry.path)} size="sm" variant="secondary">
          <FolderOpen className="size-4" /> Open folder
        </Button>
      </div>

      <div className="relative min-h-0 flex-1">
        {pv && probe && (
          <div className="absolute bottom-9 left-3 z-10 flex items-center gap-2 rounded-lg border border-line bg-white/95 px-2.5 py-1.5 font-mono text-[10.5px] text-ink shadow-md backdrop-blur">
            <span className="text-faint">diag:</span>
            <span>{diag}</span>
            <span className="text-faint">img:</span>
            {imgOk === null ? (
              <span className="text-faint">…</span>
            ) : imgOk ? (
              <span className="text-ok">OK</span>
            ) : (
              <span className="text-danger">FAIL</span>
            )}
            <img
              alt="probe"
              className="size-6 rounded border border-line"
              onError={() => setImgOk(false)}
              onLoad={() => setImgOk(true)}
              src={probe}
            />
          </div>
        )}
        {pv ? (
          <MapCanvas footprints={[]} preview={pv} selectedPaths={new Set()} />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 bg-canvas text-center">
            <div className="flex size-16 items-center justify-center rounded-2xl bg-[#eef2f5]">
              <Layers className="size-7 text-[#9aa6b6]" />
            </div>
            <div>
              <div className="font-semibold text-[14px] text-ink">No in-app preview for a COG</div>
              <div className="mt-1 max-w-sm text-[12.5px] leading-relaxed text-muted">
                A Cloud-Optimized GeoTIFF isn't a tile pyramid, so it can't be drawn on the offline
                preview map. Open it in a desktop GIS (QGIS, ArcGIS) or your tile server.
              </div>
            </div>
            <Button onClick={() => revealInExplorer(entry.path)} variant="secondary">
              <FolderOpen className="size-4" /> Reveal file
            </Button>
          </div>
        )}
      </div>
    </motion.div>
  );
}

function MapCard({
  entry: m,
  selected,
  onToggle,
  onReveal,
  onDelete,
  onOpen,
  tileBase,
}: {
  entry: MapEntry;
  selected: boolean;
  onToggle: () => void;
  onReveal: () => void;
  onDelete: () => void;
  onOpen: () => void;
  tileBase: string;
}) {
  const thumb = tileThumbUrl(tileBase, m);
  const url = entryTileUrl(tileBase, m);
  const [thumbOk, setThumbOk] = useState(true);
  const [copied, setCopied] = useState(false);
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return (
    <div
      className={cn(
        'group cursor-pointer overflow-hidden rounded-[13px] border bg-white transition-all',
        selected
          ? 'border-brand shadow-[0_0_0_3px_rgba(36,99,235,.12)]'
          : 'border-line hover:border-[#cdd5e0] hover:shadow-[0_8px_24px_-16px_rgba(16,24,40,.3)]',
      )}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onOpen();
        }
      }}
      role="button"
      tabIndex={0}
    >
      {/* thumbnail */}
      <div className="relative h-[132px] overflow-hidden border-line-soft border-b bg-[#eef2f5]">
        {thumb && thumbOk ? (
          <img
            alt={m.name}
            className="size-full object-cover"
            onError={() => setThumbOk(false)}
            src={thumb}
          />
        ) : (
          <div className="flex size-full items-center justify-center bg-gradient-to-br from-[#f3f5f8] to-[#e7ecf2]">
            {m.kind === 'cog' ? (
              <Layers className="size-7 text-[#b9c2cf]" />
            ) : (
              <ImageOff className="size-6 text-[#b9c2cf]" />
            )}
          </div>
        )}
        {/* hover hint */}
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-ink/0 opacity-0 transition-all group-hover:bg-ink/15 group-hover:opacity-100">
          <span className="flex items-center gap-1.5 rounded-full bg-white/95 px-2.5 py-1 font-medium text-[11px] text-ink shadow-sm">
            <Layers className="size-3 text-brand" /> {m.kind === 'cog' ? 'Details' : 'Preview map'}
          </span>
        </div>
        {/* checkbox */}
        <button
          className={cn(
            'absolute top-2.5 left-2.5 flex size-[20px] items-center justify-center rounded-[6px] border transition-all',
            selected
              ? 'border-brand bg-brand opacity-100'
              : 'border-white/80 bg-white/70 opacity-0 backdrop-blur group-hover:opacity-100',
          )}
          onClick={stop(onToggle)}
          type="button"
        >
          {selected && (
            <svg fill="none" height="11" stroke="#fff" strokeWidth="3" viewBox="0 0 24 24" width="11">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          )}
        </button>
        <span className="absolute top-2.5 right-2.5 rounded-md bg-white/90 px-1.5 py-0.5 font-semibold text-[10px] text-muted uppercase backdrop-blur">
          {m.format}
        </span>
      </div>

      {/* body */}
      <div className="p-3.5">
        <div className="truncate font-semibold text-[13.5px] text-ink" title={m.name}>
          {m.name}
        </div>
        <div className="mt-2 flex flex-wrap gap-1.5">
          {m.min_zoom != null && m.max_zoom != null && (
            <Badge tone="neutral">
              z{m.min_zoom}–{m.max_zoom}
            </Badge>
          )}
          {m.tiles_total != null && (
            <Badge tone="neutral">{m.tiles_total.toLocaleString()} tiles</Badge>
          )}
          <Badge tone="neutral">{formatBytes(m.size)}</Badge>
        </div>
        <div className="mt-3 flex items-center justify-between border-line-soft border-t pt-2.5">
          <span className="truncate font-mono text-[10.5px] text-faint">
            {m.crs ?? (m.kind === 'cog' ? 'COG' : '—')} · {relTime(m.modified)}
          </span>
          <div className="flex items-center gap-0.5">
            {url && (
              <button
                className="flex size-7 items-center justify-center rounded-md text-faint transition-colors hover:bg-[#f1f2f5] hover:text-ink-soft"
                onClick={stop(() => {
                  navigator.clipboard?.writeText(url).catch(() => {});
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1400);
                })}
                title={`Copy tile URL\n${url}`}
                type="button"
              >
                {copied ? (
                  <Check className="size-3.5 text-ok" />
                ) : (
                  <Link2 className="size-3.5" />
                )}
              </button>
            )}
            <button
              className="flex size-7 items-center justify-center rounded-md text-faint transition-colors hover:bg-[#f1f2f5] hover:text-ink-soft"
              onClick={stop(onReveal)}
              title="Reveal in Explorer"
              type="button"
            >
              <FolderOpen className="size-3.5" />
            </button>
            <button
              className="flex size-7 items-center justify-center rounded-md text-faint transition-colors hover:bg-danger-tint hover:text-danger"
              onClick={stop(onDelete)}
              title="Delete"
              type="button"
            >
              <Trash2 className="size-3.5" />
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function EmptyState({
  searching,
  onImport,
  onStudio,
}: {
  searching: boolean;
  onImport: () => void;
  onStudio?: () => void;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 text-center">
      <div className="flex size-16 items-center justify-center rounded-2xl bg-brand-tint">
        <Layers className="size-7 text-brand" />
      </div>
      <div>
        <div className="font-semibold text-[15px] text-ink">
          {searching ? 'No tile maps match' : 'No tile maps yet'}
        </div>
        <div className="mt-1 text-[12.5px] text-muted">
          {searching
            ? 'Try a different search.'
            : 'Generate one in the Studio, or import an existing .mbtiles / COG.'}
        </div>
      </div>
      {!searching && (
        <div className="flex gap-2.5">
          {onStudio && (
            <Button onClick={onStudio} variant="primary">
              Open Studio
            </Button>
          )}
          <Button onClick={onImport} variant="secondary">
            <Upload className="size-4" /> Import
          </Button>
        </div>
      )}
    </div>
  );
}
