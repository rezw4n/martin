import { Layer, Map as MapLibreMap, type MapRef, Source } from '@vis.gl/react-maplibre';
import type { StyleSpecification } from 'maplibre-gl';
import { useEffect, useMemo, useRef } from 'react';
import 'maplibre-gl/dist/maplibre-gl.css';
import type { BBox, RasterInfo } from './types';

// A neutral raster basemap so footprints/imagery have geographic context.
const BASE_STYLE: StyleSpecification = {
  layers: [{ id: 'osm', source: 'osm', type: 'raster' }],
  sources: {
    osm: {
      attribution: '© OpenStreetMap contributors',
      tileSize: 256,
      tiles: ['https://tile.openstreetmap.org/{z}/{x}/{y}.png'],
      type: 'raster',
    },
  },
  version: 8,
};

function bboxToPolygon(b: BBox): [number, number][][] {
  return [
    [
      [b.min_x, b.min_y],
      [b.max_x, b.min_y],
      [b.max_x, b.max_y],
      [b.min_x, b.max_y],
      [b.min_x, b.min_y],
    ],
  ];
}

function unionBbox(boxes: BBox[]): BBox | null {
  if (boxes.length === 0) return null;
  return boxes.reduce((acc, b) => ({
    max_x: Math.max(acc.max_x, b.max_x),
    max_y: Math.max(acc.max_y, b.max_y),
    min_x: Math.min(acc.min_x, b.min_x),
    min_y: Math.min(acc.min_y, b.min_y),
  }));
}

interface CoverageMapProps {
  /** Source footprints to outline. */
  footprints: RasterInfo[];
  /** z/x/y template of the generated map to overlay, if any. */
  resultTilesUrl?: string;
  /** Bounds of the generated map (used for fit-bounds when previewing). */
  resultBounds?: BBox;
  /** Whether the generated overlay is shown. */
  showResult: boolean;
  height?: number;
}

export function CoverageMap({
  footprints,
  resultTilesUrl,
  resultBounds,
  showResult,
  height = 460,
}: CoverageMapProps) {
  const mapRef = useRef<MapRef>(null);

  const footprintsGeojson = useMemo(
    () => ({
      features: footprints.map((f) => ({
        geometry: { coordinates: bboxToPolygon(f.bounds_wgs84), type: 'Polygon' as const },
        properties: { name: f.file_name },
        type: 'Feature' as const,
      })),
      type: 'FeatureCollection' as const,
    }),
    [footprints],
  );

  // Fit the map to whatever is most relevant: the result, else the footprints.
  useEffect(() => {
    const map = mapRef.current?.getMap();
    if (!map) return;
    const target =
      showResult && resultBounds
        ? resultBounds
        : unionBbox(footprints.map((f) => f.bounds_wgs84));
    if (!target) return;
    map.fitBounds(
      [
        [target.min_x, target.min_y],
        [target.max_x, target.max_y],
      ],
      { duration: 600, maxZoom: 17, padding: 48 },
    );
  }, [footprints, resultBounds, showResult]);

  const hasFootprints = footprints.length > 0;

  return (
    <div className="overflow-hidden rounded-md border border-border" style={{ height }}>
      <MapLibreMap
        initialViewState={{ latitude: 0, longitude: 0, zoom: 1 }}
        mapStyle={BASE_STYLE}
        ref={mapRef}
        style={{ height: '100%', width: '100%' }}
      >
        {showResult && resultTilesUrl && (
          <Source id="studio-result" tileSize={256} tiles={[resultTilesUrl]} type="raster">
            <Layer id="studio-result-layer" source="studio-result" type="raster" />
          </Source>
        )}

        {hasFootprints && (
          <Source data={footprintsGeojson} id="studio-footprints" type="geojson">
            <Layer
              id="studio-footprints-fill"
              paint={{
                'fill-color': '#a855f7',
                'fill-opacity': showResult ? 0.04 : 0.18,
              }}
              source="studio-footprints"
              type="fill"
            />
            <Layer
              id="studio-footprints-outline"
              paint={{ 'line-color': '#a855f7', 'line-dasharray': [2, 1], 'line-width': 2 }}
              source="studio-footprints"
              type="line"
            />
          </Source>
        )}
      </MapLibreMap>
    </div>
  );
}
