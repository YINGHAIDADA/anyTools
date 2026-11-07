// Mirror of crates/modules/wheel/src/geometry.rs — the native glass mask and these paths must line up.
import type { Geom } from './types';

export function sectorPoints(i: number, n: number, g: Geom, steps = 48): [number, number][] {
  const c = g.win / 2;
  const w = (Math.PI * 2) / n;
  const mid = i * w;
  const at = (r: number, a: number): [number, number] => [c + r * Math.sin(a), c - r * Math.cos(a)];
  const off = (r: number) => Math.asin(g.gap / 2 / r);
  const pts: [number, number][] = [];
  const [o0, o1] = [mid - w / 2 + off(g.r_out), mid + w / 2 - off(g.r_out)];
  for (let k = 0; k <= steps; k++) pts.push(at(g.r_out, o0 + ((o1 - o0) * k) / steps));
  const [i0, i1] = [mid - w / 2 + off(g.r_in), mid + w / 2 - off(g.r_in)];
  for (let k = steps; k >= 0; k--) pts.push(at(g.r_in, i0 + ((i1 - i0) * k) / steps));
  return pts;
}

export function sectorPath(i: number, n: number, g: Geom, steps = 48): string {
  return 'M' + sectorPoints(i, n, g, steps).map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join('L') + 'Z';
}

/** Center of sector `i` at radius `r` (default: middle of the ring). */
export function polar(i: number, n: number, g: Geom, r = (g.r_out + g.r_in) / 2): [number, number] {
  const c = g.win / 2;
  const a = (i * Math.PI * 2) / n;
  return [c + r * Math.sin(a), c - r * Math.cos(a)];
}
