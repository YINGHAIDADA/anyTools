// Mirror of wheel::sector_polygon in Rust — the native glass mask and this SVG must line up exactly.

export function sectorPath(i, n, g, steps = 48) {
  const c = g.win / 2;
  const w = (Math.PI * 2) / n;
  const mid = i * w;
  const at = (r, a) => [c + r * Math.sin(a), c - r * Math.cos(a)];
  const off = (r) => Math.asin(g.gap / 2 / r);
  const pts = [];
  const [o0, o1] = [mid - w / 2 + off(g.r_out), mid + w / 2 - off(g.r_out)];
  for (let k = 0; k <= steps; k++) pts.push(at(g.r_out, o0 + ((o1 - o0) * k) / steps));
  const [i0, i1] = [mid - w / 2 + off(g.r_in), mid + w / 2 - off(g.r_in)];
  for (let k = steps; k >= 0; k--) pts.push(at(g.r_in, i0 + ((i1 - i0) * k) / steps));
  return 'M' + pts.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join('L') + 'Z';
}

export function iconPos(i, n, g) {
  const c = g.win / 2;
  const a = (i * Math.PI * 2) / n;
  const r = (g.r_out + g.r_in) / 2;
  return [c + r * Math.sin(a), c - r * Math.cos(a)];
}
