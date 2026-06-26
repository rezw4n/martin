import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function formatBytes(bytes: number): string {
  if (!bytes) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** i;
  return `${value >= 100 || i === 0 ? Math.round(value) : value.toFixed(1)} ${units[i]}`;
}

export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** Convert a WGS84 lon/lat bbox into a maplibre fitBounds-friendly tuple. */
export function bboxToBounds(b: {
  min_x: number;
  min_y: number;
  max_x: number;
  max_y: number;
}): [[number, number], [number, number]] {
  return [
    [b.min_x, b.min_y],
    [b.max_x, b.max_y],
  ];
}
