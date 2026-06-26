import { useEffect, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import {
  ArrowLeft,
  Check,
  Circle,
  Copy,
  Database,
  Hexagon,
  Link2,
  Loader2,
  MapPin,
  Pencil,
  Plus,
  RefreshCw,
  Spline,
  Trash2,
  Upload,
} from 'lucide-react';
import {
  pgDeleteConnection,
  pgDropSource,
  pgImport,
  pgOverview,
  pgSaveConnection,
  pgTestConnection,
  pickVectorFile,
} from '@/lib/api';
import type { PgConnDto, PgConnectionInput, PgOverview, PgSourceDto } from '@/lib/types';
import { cn } from '@/lib/utils';
import { MapCanvas } from '@/components/MapCanvas';
import { Badge, Button, Field, TextInput } from '@/components/ui';

/* ── geometry helpers ─────────────────────────────────────────────────── */
function GeomIcon({ type, className }: { type: string; className?: string }) {
  if (/POINT/i.test(type)) return <MapPin className={className} />;
  if (/LINE/i.test(type)) return <Spline className={className} />;
  return <Hexagon className={className} />;
}

function geomLabel(type: string): string {
  if (/POINT/i.test(type)) return 'Points';
  if (/LINE/i.test(type)) return 'Lines';
  if (/POLYGON/i.test(type)) return 'Polygons';
  return type;
}

function blankConn(): PgConnectionInput {
  return {
    id: '',
    label: '',
    host: '127.0.0.1',
    port: 5432,
    dbname: '',
    user: 'postgres',
    password: '',
    sslmode: 'prefer',
    enabled: true,
    bundled: false,
  };
}

/* ── main view ────────────────────────────────────────────────────────── */
export function DataCatalog() {
  const [ov, setOv] = useState<PgOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState<PgConnectionInput | null>(null);
  const [importing, setImporting] = useState(false);
  const [preview, setPreview] = useState<PgSourceDto | null>(null);
  const [confirmDrop, setConfirmDrop] = useState<PgSourceDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [dropError, setDropError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = async () => {
    try {
      setOv(await pgOverview());
    } catch {
      setOv(null);
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    load();
    const t = setInterval(load, 6000);
    return () => clearInterval(t);
  }, []);

  const sources = ov?.sources ?? [];
  const connections = ov?.connections ?? [];

  const onDrop = async (src: PgSourceDto) => {
    setBusy(true);
    setDropError(null);
    try {
      await pgDropSource(src.id);
      setConfirmDrop(null);
      await load();
    } catch (e) {
      setDropError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const onDeleteConnection = async (id: string) => {
    setNotice(null);
    try {
      await pgDeleteConnection(id);
      await load();
    } catch (e) {
      setNotice(String(e));
    }
  };

  if (!loading && ov && !ov.available) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 text-center">
        <div className="flex size-16 items-center justify-center rounded-2xl bg-brand-tint">
          <Database className="size-7 text-brand" />
        </div>
        <div>
          <div className="font-semibold text-[15px] text-ink">PostGIS isn't available</div>
          <div className="mt-1 max-w-sm text-[12.5px] leading-relaxed text-muted">
            The bundled PostgreSQL wasn't found next to the app, and no external connection is
            configured. Add a connection to get started.
          </div>
        </div>
        <Button onClick={() => setEditing(blankConn())} variant="primary">
          <Plus className="size-4" /> Add connection
        </Button>
        <AnimatePresence>
          {editing && (
            <ConnectionEditor
              conn={editing}
              onClose={() => setEditing(null)}
              onSaved={async () => {
                setEditing(null);
                await load();
              }}
            />
          )}
        </AnimatePresence>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* toolbar */}
      <div className="flex flex-none items-center gap-3 px-1 pb-4">
        <div className="text-[13px] text-muted">
          <span className="font-semibold text-ink">{sources.length}</span> vector layer
          {sources.length === 1 ? '' : 's'} ·{' '}
          <span className="font-semibold text-ink">{connections.length}</span> connection
          {connections.length === 1 ? '' : 's'}
        </div>
        <div className="flex-1" />
        <Button onClick={load} size="sm" variant="ghost">
          <RefreshCw className={cn('size-3.5', loading && 'animate-spin')} />
        </Button>
        <Button onClick={() => setEditing(blankConn())} size="sm" variant="secondary">
          <Plus className="size-4" /> Add connection
        </Button>
        <Button onClick={() => setImporting(true)} size="sm" variant="primary">
          <Upload className="size-4" /> Import data
        </Button>
      </div>

      {/* action error banner */}
      {notice && (
        <div className="mb-3 flex items-start gap-2 rounded-[10px] border border-[#f4c7c4] bg-danger-tint px-3 py-2.5 text-[12px] text-[#b42318]">
          <span className="min-w-0 flex-1 break-words">{notice}</span>
          <button className="font-semibold text-[#b42318] hover:underline" onClick={() => setNotice(null)} type="button">
            Dismiss
          </button>
        </div>
      )}

      {/* connections strip */}
      <div className="flex flex-none flex-col gap-2 px-1 pb-5">
        {connections.map((c) => (
          <ConnectionRow
            connection={c}
            key={c.id}
            onDelete={() => onDeleteConnection(c.id)}
            onEdit={() =>
              setEditing({
                id: c.id,
                label: c.label,
                host: c.host,
                port: c.port,
                dbname: c.dbname,
                user: c.user,
                password: '',
                sslmode: c.sslmode,
                enabled: c.enabled,
                bundled: c.bundled,
              })
            }
          />
        ))}
      </div>

      {/* vector layer cards */}
      <div className="min-h-0 flex-1">
        {loading && !ov ? (
          <div className="flex h-40 items-center justify-center text-muted">
            <Loader2 className="size-5 animate-spin" />
          </div>
        ) : sources.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-4 rounded-[14px] border border-line border-dashed bg-[#fbfcfd] py-14 text-center">
            <div className="flex size-14 items-center justify-center rounded-2xl bg-brand-tint">
              <Hexagon className="size-6 text-brand" />
            </div>
            <div>
              <div className="font-semibold text-[14px] text-ink">No vector layers yet</div>
              <div className="mt-1 max-w-md text-[12.5px] leading-relaxed text-muted">
                Import a shapefile or GeoJSON — it's reprojected to WGS-84, stored in PostGIS, and
                served as vector tiles automatically (any source projection works).
              </div>
            </div>
            <Button onClick={() => setImporting(true)} variant="primary">
              <Upload className="size-4" /> Import data
            </Button>
          </div>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(248px,1fr))] gap-4">
            {sources.map((s) => (
              <VectorCard
                key={s.id}
                onDrop={() => setConfirmDrop(s)}
                onPreview={() => setPreview(s)}
                source={s}
              />
            ))}
          </div>
        )}
      </div>

      {/* overlays */}
      <AnimatePresence>
        {editing && (
          <ConnectionEditor
            conn={editing}
            onClose={() => setEditing(null)}
            onSaved={async () => {
              setEditing(null);
              await load();
            }}
          />
        )}
        {importing && (
          <ImportDialog
            connections={connections}
            onClose={() => setImporting(false)}
            onDone={async () => {
              setImporting(false);
              await load();
            }}
          />
        )}
        {preview && <VectorPreviewOverlay onClose={() => setPreview(null)} source={preview} />}
        {confirmDrop && (
          <ConfirmDrop
            busy={busy}
            error={dropError}
            onCancel={() => {
              setConfirmDrop(null);
              setDropError(null);
            }}
            onConfirm={() => onDrop(confirmDrop)}
            source={confirmDrop}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

/* ── connection row ───────────────────────────────────────────────────── */
function ConnectionRow({
  connection: c,
  onEdit,
  onDelete,
}: {
  connection: PgConnDto;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const dot = c.ok ? 'bg-ok' : c.message ? 'bg-danger' : 'bg-[#d0a64a]';
  return (
    <div className="flex items-center gap-3 rounded-[12px] border border-line bg-white px-4 py-3">
      <div className="flex size-9 flex-none items-center justify-center rounded-[10px] bg-brand-tint">
        <Database className="size-[18px] text-brand" />
      </div>
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate font-semibold text-[13.5px] text-ink">{c.label}</span>
          {c.bundled && <Badge tone="brand">Bundled</Badge>}
          <span className={cn('size-[7px] flex-none rounded-full', dot, c.ok && 'mts-pulse')} />
        </div>
        <div className="mt-0.5 truncate font-mono text-[11px] text-faint">
          {c.user}@{c.host}:{c.port}/{c.dbname}
          {c.message ? ` · ${c.message}` : ''}
        </div>
      </div>
      <div className="flex-1" />
      <Badge tone={c.ok ? 'ok' : 'neutral'}>
        {c.table_count} table{c.table_count === 1 ? '' : 's'}
      </Badge>
      <button
        className="flex size-8 items-center justify-center rounded-lg text-faint transition-colors hover:bg-[#f1f2f5] hover:text-ink-soft"
        onClick={onEdit}
        title="Edit connection"
        type="button"
      >
        <Pencil className="size-3.5" />
      </button>
      {!c.bundled && (
        <button
          className="flex size-8 items-center justify-center rounded-lg text-faint transition-colors hover:bg-danger-tint hover:text-danger"
          onClick={onDelete}
          title="Remove connection"
          type="button"
        >
          <Trash2 className="size-3.5" />
        </button>
      )}
    </div>
  );
}

/* ── vector card ──────────────────────────────────────────────────────── */
function VectorCard({
  source: s,
  onPreview,
  onDrop,
}: {
  source: PgSourceDto;
  onPreview: () => void;
  onDrop: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return (
    <div
      className="group cursor-pointer overflow-hidden rounded-[13px] border border-line bg-white transition-all hover:border-[#cdd5e0] hover:shadow-[0_8px_24px_-16px_rgba(16,24,40,.3)]"
      onClick={onPreview}
      onKeyDown={(e) => {
        // Don't hijack Enter/Space when an inner control is focused.
        if (e.target !== e.currentTarget) return;
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onPreview();
        }
      }}
      role="button"
      tabIndex={0}
    >
      <div className="relative flex h-[108px] items-center justify-center overflow-hidden border-line-soft border-b bg-gradient-to-br from-[#eef3fb] to-[#e2e9f4]">
        <GeomIcon className="size-9 text-[#9db4dd]" type={s.geom_type} />
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-ink/0 opacity-0 transition-all group-hover:bg-ink/10 group-hover:opacity-100">
          <span className="flex items-center gap-1.5 rounded-full bg-white/95 px-2.5 py-1 font-medium text-[11px] text-ink shadow-sm">
            <Database className="size-3 text-brand" /> Preview
          </span>
        </div>
        <span className="absolute top-2.5 right-2.5 rounded-md bg-white/90 px-1.5 py-0.5 font-semibold text-[10px] text-muted uppercase backdrop-blur">
          MVT
        </span>
      </div>
      <div className="p-3.5">
        <div className="truncate font-semibold text-[13.5px] text-ink" title={s.table}>
          {s.table}
        </div>
        <div className="mt-2 flex flex-wrap gap-1.5">
          <Badge tone="brand">{geomLabel(s.geom_type)}</Badge>
          <Badge tone="neutral">EPSG:{s.srid}</Badge>
          <Badge tone="neutral">{s.fields.length} field{s.fields.length === 1 ? '' : 's'}</Badge>
        </div>
        <div className="mt-3 flex items-center justify-between border-line-soft border-t pt-2.5">
          <span className="flex min-w-0 items-center gap-1 truncate font-mono text-[10.5px] text-faint">
            <Database className="size-3 flex-none" /> {s.conn_label}
          </span>
          <div className="flex items-center gap-0.5">
            <button
              className="flex size-7 items-center justify-center rounded-md text-faint transition-colors hover:bg-[#f1f2f5] hover:text-ink-soft"
              onClick={stop(() => {
                navigator.clipboard
                  ?.writeText(s.tile_url)
                  .then(() => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1400);
                  })
                  .catch(() => {});
              })}
              title={`Copy tile URL\n${s.tile_url}`}
              type="button"
            >
              {copied ? <Check className="size-3.5 text-ok" /> : <Link2 className="size-3.5" />}
            </button>
            <button
              className="flex size-7 items-center justify-center rounded-md text-faint transition-colors hover:bg-danger-tint hover:text-danger"
              onClick={stop(onDrop)}
              title="Drop table"
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

/* ── connection editor modal ──────────────────────────────────────────── */
function ConnectionEditor({
  conn,
  onClose,
  onSaved,
}: {
  conn: PgConnectionInput;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [form, setForm] = useState<PgConnectionInput>(conn);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; message: string } | null>(null);
  const isNew = !conn.id;
  const set = (patch: Partial<PgConnectionInput>) => setForm((f) => ({ ...f, ...patch }));

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const r = await pgTestConnection(form);
      setResult({ ok: r.ok, message: r.message });
    } catch (e) {
      setResult({ ok: false, message: String(e) });
    } finally {
      setTesting(false);
    }
  };
  const save = async () => {
    setSaving(true);
    try {
      await pgSaveConnection(form);
      onSaved();
    } catch (e) {
      setResult({ ok: false, message: String(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal onClose={onClose} title={isNew ? 'Add PostGIS connection' : `Edit ${conn.label}`} wide>
      <div className="grid grid-cols-2 gap-3">
        <div className="col-span-2">
          <Field label="Label">
            <TextInput
              onChange={(e) => set({ label: e.target.value })}
              placeholder="My PostGIS server"
              value={form.label}
            />
          </Field>
        </div>
        <Field label="Host">
          <TextInput
            disabled={form.bundled}
            mono
            onChange={(e) => set({ host: e.target.value })}
            value={form.host}
          />
        </Field>
        <Field label="Port">
          <TextInput
            disabled={form.bundled}
            mono
            onChange={(e) => set({ port: Number(e.target.value) || 0 })}
            type="number"
            value={form.port}
          />
        </Field>
        <Field label="Database">
          <TextInput
            disabled={form.bundled}
            mono
            onChange={(e) => set({ dbname: e.target.value })}
            value={form.dbname}
          />
        </Field>
        <Field label="User">
          <TextInput
            disabled={form.bundled}
            mono
            onChange={(e) => set({ user: e.target.value })}
            value={form.user}
          />
        </Field>
        <Field label="Password" hint={isNew ? undefined : 'leave blank to keep'}>
          <TextInput
            mono
            onChange={(e) => set({ password: e.target.value })}
            placeholder="••••••••"
            type="password"
            value={form.password}
          />
        </Field>
        <Field label="SSL mode">
          <select
            className="h-10 w-full rounded-[9px] border border-[#dfe3e9] bg-white px-3 text-[13px] text-ink outline-none focus:border-brand"
            onChange={(e) => set({ sslmode: e.target.value })}
            value={form.sslmode}
          >
            {['disable', 'prefer', 'require', 'verify-ca', 'verify-full'].map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </Field>
      </div>

      {result && (
        <div
          className={cn(
            'mt-4 flex items-start gap-2 rounded-[10px] border px-3 py-2.5 text-[12px]',
            result.ok
              ? 'border-[#bbe7cf] bg-ok-tint text-[#067647]'
              : 'border-[#f4c7c4] bg-danger-tint text-[#b42318]',
          )}
        >
          {result.ok ? (
            <Check className="mt-px size-3.5 flex-none" />
          ) : (
            <Circle className="mt-px size-3.5 flex-none" />
          )}
          <span className="min-w-0 break-words">{result.message}</span>
        </div>
      )}

      <div className="mt-5 flex items-center justify-end gap-2.5">
        <Button busy={testing} onClick={test} variant="secondary">
          Test connection
        </Button>
        <span className="flex-1" />
        <Button onClick={onClose} variant="ghost">
          Cancel
        </Button>
        <Button busy={saving} disabled={!form.label.trim()} onClick={save} variant="primary">
          Save
        </Button>
      </div>
    </Modal>
  );
}

/* ── import dialog ────────────────────────────────────────────────────── */
function ImportDialog({
  connections,
  onClose,
  onDone,
}: {
  connections: PgConnDto[];
  onClose: () => void;
  onDone: () => void;
}) {
  const targets = connections.filter((c) => c.enabled);
  const [path, setPath] = useState('');
  const [table, setTable] = useState('');
  const [connId, setConnId] = useState(targets.find((c) => c.bundled)?.id ?? targets[0]?.id ?? '');
  const [srcSrs, setSrcSrs] = useState('');
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pick = async () => {
    const p = await pickVectorFile();
    if (p) {
      setPath(p);
      if (!table) {
        const base = p.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') ?? '';
        setTable(base.toLowerCase().replace(/[^a-z0-9_]+/g, '_').replace(/^_+|_+$/g, ''));
      }
    }
  };
  const run = async () => {
    setRunning(true);
    setError(null);
    try {
      await pgImport({
        path,
        conn_id: connId || null,
        table: table || null,
        src_srs: srcSrs.trim() || null,
      });
      onDone();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  return (
    <Modal onClose={running ? () => {} : onClose} title="Import data to PostGIS" wide>
      <div className="flex flex-col gap-3">
        <Field label="Source file" hint="shp · geojson · gpkg · kml">
          <div className="flex gap-2">
            <TextInput
              className="flex-1"
              mono
              onChange={(e) => setPath(e.target.value)}
              placeholder="Choose a shapefile or GeoJSON…"
              value={path}
            />
            <Button onClick={pick} variant="secondary">
              Browse…
            </Button>
          </div>
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Target connection">
            <select
              className="h-10 w-full rounded-[9px] border border-[#dfe3e9] bg-white px-3 text-[13px] text-ink outline-none focus:border-brand"
              onChange={(e) => setConnId(e.target.value)}
              value={connId}
            >
              {targets.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.label}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Table name">
            <TextInput
              mono
              onChange={(e) =>
                setTable(e.target.value.toLowerCase().replace(/[^a-z0-9_]+/g, '_'))
              }
              placeholder="my_layer"
              value={table}
            />
          </Field>
        </div>
        <Field label="Source CRS override" hint="optional — only if the file lacks a .prj">
          <TextInput
            mono
            onChange={(e) => setSrcSrs(e.target.value)}
            placeholder="e.g. EPSG:3857"
            value={srcSrs}
          />
        </Field>
        <p className="rounded-[10px] bg-[#f6f9ff] px-3 py-2.5 text-[12px] leading-relaxed text-muted">
          The data is reprojected to <span className="font-semibold text-ink">EPSG:4326</span> on
          import and served as Web-Mercator vector tiles — so it lines up correctly no matter the
          original projection.
        </p>
        {error && (
          <pre className="max-h-32 overflow-auto whitespace-pre-wrap rounded-[10px] border border-[#f4c7c4] bg-danger-tint px-3 py-2.5 text-[11.5px] text-[#b42318]">
            {error}
          </pre>
        )}
      </div>

      <div className="mt-5 flex items-center justify-end gap-2.5">
        <Button disabled={running} onClick={onClose} variant="ghost">
          Cancel
        </Button>
        <Button busy={running} disabled={!path || !connId} onClick={run} variant="primary">
          <Upload className="size-4" /> Import
        </Button>
      </div>
    </Modal>
  );
}

/* ── vector preview overlay ───────────────────────────────────────────── */
function VectorPreviewOverlay({
  source: s,
  onClose,
}: {
  source: PgSourceDto;
  onClose: () => void;
}) {
  const [b0, b1, b2, b3] = s.bounds;
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
            {s.table}
          </div>
          <div className="mt-0.5 flex items-center gap-1.5">
            <Badge tone="brand">{geomLabel(s.geom_type)}</Badge>
            <Badge tone="neutral">EPSG:{s.srid}</Badge>
            <Badge tone="neutral">{s.fields.length} fields</Badge>
            <Badge tone="neutral">{s.conn_label}</Badge>
          </div>
        </div>
        <span className="flex-1" />
        <CopyUrl url={s.tile_url} />
      </div>
      <div className="relative min-h-0 flex-1">
        <MapCanvas
          footprints={[]}
          preview={null}
          selectedPaths={new Set()}
          vector={{
            tilesUrl: s.tile_url,
            sourceLayer: s.table,
            geomType: s.geom_type,
            bounds: { min_x: b0, min_y: b1, max_x: b2, max_y: b3 },
            maxZoom: s.maxzoom,
          }}
        />
      </div>
    </motion.div>
  );
}

/* ── drop confirm ─────────────────────────────────────────────────────── */
function ConfirmDrop({
  source,
  busy,
  error,
  onCancel,
  onConfirm,
}: {
  source: PgSourceDto;
  busy: boolean;
  error?: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Modal onClose={onCancel}>
      <div className="flex items-center gap-3">
        <div className="flex size-10 flex-none items-center justify-center rounded-full bg-danger-tint">
          <Trash2 className="size-[18px] text-danger" />
        </div>
        <div className="font-semibold text-[15px] text-ink">Drop table?</div>
      </div>
      <p className="mt-3 text-[13px] leading-relaxed text-muted">
        Are you sure you would like to delete{' '}
        <span className="font-semibold text-ink">{source.table}</span>? This permanently drops the
        table from PostGIS and can't be undone.
      </p>
      {error && (
        <pre className="mt-3 max-h-28 overflow-auto whitespace-pre-wrap rounded-[10px] border border-[#f4c7c4] bg-danger-tint px-3 py-2 text-[11.5px] text-[#b42318]">
          {error}
        </pre>
      )}
      <div className="mt-5 flex justify-end gap-2.5">
        <Button onClick={onCancel} variant="secondary">
          Cancel
        </Button>
        <Button busy={busy} className="bg-danger text-white hover:bg-[#c0322f]" onClick={onConfirm}>
          <Trash2 className="size-4" /> Yes, drop
        </Button>
      </div>
    </Modal>
  );
}

/* ── small shared pieces ──────────────────────────────────────────────── */
function CopyUrl({ url }: { url: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="group/url flex max-w-[440px] items-center gap-2 rounded-lg border border-line bg-[#f7f8fa] py-1.5 pr-2 pl-2.5 text-left transition-colors hover:border-[#cdd5e0] hover:bg-[#f1f2f5]"
      onClick={() => {
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

function Modal({
  title,
  children,
  onClose,
  wide,
}: {
  title?: string;
  children: React.ReactNode;
  onClose: () => void;
  wide?: boolean;
}) {
  return (
    <motion.div
      animate={{ opacity: 1 }}
      className="absolute inset-0 z-50 flex items-center justify-center bg-ink/30 p-6 backdrop-blur-[2px]"
      exit={{ opacity: 0 }}
      initial={{ opacity: 0 }}
      onClick={onClose}
      transition={{ duration: 0.14 }}
    >
      <motion.div
        animate={{ opacity: 1, scale: 1, y: 0 }}
        className={cn(
          'rounded-[16px] border border-line bg-white p-5 shadow-[0_24px_64px_-16px_rgba(16,24,40,.4)]',
          wide ? 'w-[520px]' : 'w-[380px]',
        )}
        exit={{ opacity: 0, scale: 0.96, y: 6 }}
        initial={{ opacity: 0, scale: 0.96, y: 6 }}
        onClick={(e) => e.stopPropagation()}
        transition={{ duration: 0.16, ease: 'easeOut' }}
      >
        {title && <div className="mb-4 font-semibold text-[15px] text-ink">{title}</div>}
        {children}
      </motion.div>
    </motion.div>
  );
}
