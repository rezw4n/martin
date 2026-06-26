import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Layer, Map as MapLibreMap, type MapRef, Source } from '@vis.gl/react-maplibre';
import type { StyleSpecification } from 'maplibre-gl';
import { Layers, Minus, Plus } from 'lucide-react';
import type { BBox, RasterInfo } from '@/lib/types';
import { cn } from '@/lib/utils';

const LIGHT_STYLE: StyleSpecification = {
  version: 8,
  sources: {
    basemap: {
      type: 'raster',
      tiles: ['https://a.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png'],
      tileSize: 256,
      attribution: '© OpenStreetMap, © CARTO',
    },
  },
  layers: [
    { id: 'bg', type: 'background', paint: { 'background-color': '#eef2f5' } },
    { id: 'basemap', type: 'raster', source: 'basemap', paint: { 'raster-opacity': 0.95 } },
  ],
};

function footprintGeoJson(files: RasterInfo[], selected: Set<string>) {
  return {
    type: 'FeatureCollection' as const,
    features: files.map((f) => {
      const b = f.bounds_wgs84;
      return {
        type: 'Feature' as const,
        properties: { selected: selected.has(f.path) ? 1 : 0, name: f.file_name },
        geometry: {
          type: 'Polygon' as const,
          coordinates: [
            [
              [b.min_x, b.min_y],
              [b.max_x, b.min_y],
              [b.max_x, b.max_y],
              [b.min_x, b.max_y],
              [b.min_x, b.min_y],
            ],
          ],
        },
      };
    }),
  };
}

function unionBounds(files: RasterInfo[]): BBox | null {
  if (!files.length) return null;
  return files.reduce<BBox>(
    (acc, f) => ({
      min_x: Math.min(acc.min_x, f.bounds_wgs84.min_x),
      min_y: Math.min(acc.min_y, f.bounds_wgs84.min_y),
      max_x: Math.max(acc.max_x, f.bounds_wgs84.max_x),
      max_y: Math.max(acc.max_y, f.bounds_wgs84.max_y),
    }),
    { ...files[0].bounds_wgs84 },
  );
}

export interface MapCanvasProps {
  footprints: RasterInfo[];
  selectedPaths: Set<string>;
  preview: { url: string; bounds: BBox; maxZoom: number } | null;
}

export function MapCanvas({ footprints, selectedPaths, preview }: MapCanvasProps) {
  const mapRef = useRef<MapRef>(null);
  const [hover, setHover] = useState<{ lng: number; lat: number; zoom: number } | null>(null);
  const geojson = useMemo(
    () => footprintGeoJson(footprints, selectedPaths),
    [footprints, selectedPaths],
  );

  // fit to preview bounds, else to footprint union
  const fitTarget = preview?.bounds ?? unionBounds(footprints);
  const fitKey = preview
    ? `p:${preview.url}`
    : footprints.map((f) => f.path).join('|');

  // Hold the latest target in a ref so the stable `fitToTarget` callback (shared
  // by the map's onLoad and the fitKey effect) never reads a stale closure.
  const fitTargetRef = useRef(fitTarget);
  fitTargetRef.current = fitTarget;

  const fitToTarget = useCallback(() => {
    const map = mapRef.current?.getMap();
    const t = fitTargetRef.current;
    if (!map || !t) return;
    map.resize(); // the overlay container may have only just been laid out
    const isWorld = t.min_x <= -179 && t.max_x >= 179;
    if (isWorld) return;
    map.fitBounds(
      [
        [t.min_x, t.min_y],
        [t.max_x, t.max_y],
      ],
      { padding: 80, maxZoom: 18, duration: 600 },
    );
  }, []);

  // Re-fit when the target changes on an already-loaded map (e.g. the Studio
  // map switching to a fresh preview). Initial fit is driven by `onLoad` below,
  // which — unlike a ref read on mount — can't miss the map-ready moment.
  useEffect(() => {
    const map = mapRef.current?.getMap();
    if (map?.loaded()) fitToTarget();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fitKey]);

  const zoomBy = (d: number) => {
    const map = mapRef.current?.getMap();
    if (map) map.easeTo({ zoom: map.getZoom() + d, duration: 200 });
  };

  return (
    <div className="relative h-full w-full overflow-hidden bg-[#eef2f5]">
      <MapLibreMap
        attributionControl={false}
        initialViewState={{ longitude: 0, latitude: 18, zoom: 1.4 }}
        mapStyle={LIGHT_STYLE}
        onLoad={fitToTarget}
        onMouseMove={(e) =>
          setHover({ lng: e.lngLat.lng, lat: e.lngLat.lat, zoom: e.target.getZoom() })
        }
        ref={mapRef}
        style={{ position: 'absolute', inset: 0 }}
      >
        {preview && (
          <Source
            id="preview"
            key={preview.url}
            maxzoom={preview.maxZoom}
            tileSize={256}
            tiles={[preview.url]}
            type="raster"
          >
            <Layer id="preview-layer" paint={{ 'raster-opacity': 1 }} type="raster" />
          </Source>
        )}

        <Source data={geojson} id="footprints" type="geojson">
          <Layer
            id="footprint-fill"
            paint={{
              'fill-color': '#2463eb',
              'fill-opacity': ['case', ['==', ['get', 'selected'], 1], 0.14, 0.05],
            }}
            type="fill"
          />
          <Layer
            id="footprint-line"
            paint={{
              'line-color': '#2463eb',
              'line-width': ['case', ['==', ['get', 'selected'], 1], 2.4, 1.4],
              'line-opacity': ['case', ['==', ['get', 'selected'], 1], 1, 0.55],
            }}
            type="line"
          />
        </Source>
      </MapLibreMap>

      {/* top-left mode pill */}
      <div className="pointer-events-none absolute top-4 left-4 flex items-center gap-2 rounded-full border border-line bg-white/95 px-3 py-1.5 text-[12px] font-medium text-ink-soft shadow-[0_4px_14px_rgba(16,24,40,.08)] backdrop-blur">
        <Layers className="size-[13px] text-brand" />
        {preview ? 'Tile preview' : 'Coverage'}
      </div>

      {/* zoom controls */}
      <div className="absolute top-4 right-4 flex flex-col overflow-hidden rounded-[11px] border border-line bg-white shadow-[0_6px_20px_rgba(16,24,40,.1)]">
        <button
          className="flex size-10 items-center justify-center text-ink-soft transition-colors hover:bg-[#f6f7f9]"
          onClick={() => zoomBy(1)}
          type="button"
        >
          <Plus className="size-[17px]" />
        </button>
        <div className="h-px bg-line-soft" />
        <button
          className="flex size-10 items-center justify-center text-ink-soft transition-colors hover:bg-[#f6f7f9]"
          onClick={() => zoomBy(-1)}
          type="button"
        >
          <Minus className="size-[17px]" />
        </button>
      </div>

      {/* bottom status + credit */}
      <div className="absolute inset-x-0 bottom-0 flex h-[30px] items-center gap-4 border-t border-line bg-white/85 px-4 backdrop-blur">
        <span className="font-mono text-[11px] text-muted tabular-nums">
          {hover
            ? `${hover.lat.toFixed(4)}°, ${hover.lng.toFixed(4)}°`
            : '— · —'}
        </span>
        <span className="font-mono text-[11px] text-muted tabular-nums">
          z{hover ? hover.zoom.toFixed(1) : '—'}
        </span>
        <span className={cn('font-mono text-[11px]', preview ? 'text-brand' : 'text-muted')}>
          {preview ? 'previewing output' : 'EPSG:3857'}
        </span>
        <span className="flex-1" />
        <a
          className="text-[11px] font-medium text-brand no-underline hover:underline"
          href="https://ai-geolab.org"
          rel="noreferrer"
          target="_blank"
        >
          © AiGeoLAB · ai-geolab.org
        </a>
      </div>
    </div>
  );
}
