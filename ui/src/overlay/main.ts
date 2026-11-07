/*
 * Wheel overlay page.
 *
 * Latency model (poc/M0-RESULTS.md): the native window is invisible (alpha 0) between sessions, and
 * this page keeps every wheel pre-built and the most likely one attached in its "armed" state. On
 * trigger the native side fades the window in at once, so the first visible frame never waits for this
 * page; the page's own work (spring scale-in, center text, swapping to another wheel) lands a frame
 * later while the fade is still running. After the window is invisible again (`wheel:hidden`) we re-arm.
 */
import './overlay.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { sectorPath, polar } from '../shared/geometry';
import { iconElement, lucideSvg } from '../shared/icons';
import type { Geom, Settings, Snapshot, WheelView } from '../shared/types';

const SVGNS = 'http://www.w3.org/2000/svg';
const $ = <T extends Element = HTMLElement>(id: string) => document.getElementById(id) as unknown as T;
const stage = $('stage');
const svg = $<SVGSVGElement>('wheel');
const glow = $('glow');
const centerIcon = $('center-icon');
const centerLabel = $('center-label');
const centerHint = $('center-hint');

interface Built {
  view: WheelView;
  root: SVGGElement;
  sectors: SVGPathElement[];
  icons: SVGGElement[];
  labels: SVGTextElement[];
  masks: string[];
}

interface Session {
  seq: number;
  key: string;
  hover: number | null;
  click: boolean;
  depth: number;
  disabled: boolean[];
  hz: number;
}

let geom: Geom | null = null;
let fallback = 'default';
let defs: SVGDefsElement | null = null;
const cache = new Map<string, Built>();
let attached: string | null = null;
let cur: Session | null = null;
let appIcons: Record<string, string> = {};
let pendingRebuild = false;

// ------------------------------------------------------------------------------------------------
// Building (startup / config change only)
// ------------------------------------------------------------------------------------------------

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, parent?: Element) {
  const e = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  parent?.appendChild(e);
  return e;
}

/** Static grain texture rendered once (a live feTurbulence filter is expensive to rasterize). */
function noiseDataUrl(size = 160): string {
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const ctx = c.getContext('2d')!;
  const img = ctx.createImageData(size, size);
  for (let i = 0; i < img.data.length; i += 4) {
    const v = (Math.random() * 255) | 0;
    img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
    img.data[i + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  return c.toDataURL('image/png');
}

function buildDefs(): SVGDefsElement {
  const d = el('defs', {}, svg);
  const hl = el('linearGradient', { id: 'hl', x1: 0, y1: 0, x2: 0, y2: 1 }, d);
  el('stop', { offset: '0%', class: 'hl-top' }, hl);
  el('stop', { offset: '55%', class: 'hl-mid' }, hl);
  el('stop', { offset: '100%', class: 'hl-bottom' }, hl);
  const pat = el('pattern', { id: 'grain', width: 160, height: 160, patternUnits: 'userSpaceOnUse' }, d);
  el('image', { href: noiseDataUrl(), width: 160, height: 160 }, pat);
  return d;
}

function maskFor(i: number, n: number, g: Geom): string {
  const s = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${g.win} ${g.win}'><path d='${sectorPath(i, n, g)}' fill='none' stroke='black' stroke-width='2.4' stroke-linejoin='round'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(s)}")`;
}

function short(label: string): string {
  return [...label].length > 6 ? [...label].slice(0, 6).join('') + '…' : label;
}

function buildWheel(view: WheelView, g: Geom, d: SVGDefsElement): Built {
  const n = view.items.length;
  const c = g.win / 2;
  const root = el('g', { class: 'wheel-root' });
  const clipId = `clip-${CSS.escape(view.key)}`;
  const clip = el('clipPath', { id: clipId }, d);
  const sectors: SVGPathElement[] = [];
  const icons: SVGGElement[] = [];
  const labels: SVGTextElement[] = [];
  view.items.forEach((_, i) => {
    const p = sectorPath(i, n, g);
    el('path', { d: p }, clip);
    sectors.push(el('path', { d: p, class: 'sector' }, root));
    el('path', { d: p, class: 'sector-hl' }, root);
  });
  el('circle', { cx: c, cy: c, r: g.r_center }, clip);
  el('circle', { cx: c, cy: c, r: g.r_center, class: 'center-disc' }, root);
  el('circle', { cx: c, cy: c, r: g.r_center - 0.5, class: 'center-hl' }, root);
  el('rect', { x: 0, y: 0, width: g.win, height: g.win, fill: 'url(#grain)', 'clip-path': `url(#${clipId})`, class: 'noise' }, root);

  view.items.forEach((item, i) => {
    const [x, y] = polar(i, n, g);
    const ig = el('g', { class: 'item-icon', transform: `translate(${(x - 11).toFixed(2)} ${(y - 17).toFixed(2)})` }, root);
    ig.appendChild(iconElement(item.icon, 22, appIcons));
    icons.push(ig);
    const t = el('text', { x: x.toFixed(2), y: (y + 18).toFixed(2), class: 'item-label' }, root);
    t.textContent = short(item.label);
    labels.push(t);
    if (item.submenu) {
      // A small chevron at the outer edge marks submenus.
      const [cx, cy] = polar(i, n, g, g.r_out - 9);
      const a = (i * 360) / n;
      el('path', { d: 'M-3,1.5 L0,-1.5 L3,1.5', class: 'submenu-mark', transform: `translate(${cx.toFixed(2)} ${cy.toFixed(2)}) rotate(${a})` }, root);
    }
  });
  return { view, root, sectors, icons, labels, masks: view.items.map((_, i) => maskFor(i, n, g)) };
}

async function loadSnapshot(): Promise<void> {
  const snap = await invoke<Snapshot | null>('module_call', { module: 'wheel', msg: { type: 'snapshot' } });
  if (!snap) throw new Error('wheel module not ready');
  geom = snap.geom;
  fallback = snap.fallback;
  svg.replaceChildren();
  attached = null;
  cache.clear();
  defs = buildDefs();
  for (const v of snap.wheels) cache.set(v.key, buildWheel(v, geom, defs));
}

function refreshAppIcons(): void {
  for (const w of cache.values()) {
    w.view.items.forEach((item, i) => {
      if (item.icon.startsWith('app:') && appIcons[item.icon]) w.icons[i].replaceChildren(iconElement(item.icon, 22, appIcons));
    });
  }
}

// ------------------------------------------------------------------------------------------------
// State changes (cheap: class toggles only)
// ------------------------------------------------------------------------------------------------

function attach(key: string): Built | undefined {
  const w = cache.get(key);
  if (!w) return undefined;
  if (attached !== key) {
    if (attached !== null) cache.get(attached)?.root.remove();
    svg.appendChild(w.root);
    attached = key;
  }
  return w;
}

function resetVisual(w: Built | undefined): void {
  if (w) {
    for (const e of w.root.querySelectorAll('.hover, .flash, .disabled')) e.classList.remove('hover', 'flash', 'disabled');
    for (const s of w.sectors) s.style.transform = '';
  }
  glow.classList.remove('on');
}

function applyDisabled(w: Built, disabled: boolean[]): void {
  w.sectors.forEach((s, i) => {
    const off = !!disabled[i];
    s.classList.toggle('disabled', off);
    w.icons[i].classList.toggle('disabled', off);
    w.labels[i].classList.toggle('disabled', off);
  });
}

function setCenter(): void {
  if (!cur) return;
  const w = cache.get(cur.key);
  if (!w) return;
  const item = cur.hover !== null ? w.view.items[cur.hover] : null;
  const off = cur.hover !== null && !!cur.disabled[cur.hover];
  centerIcon.replaceChildren(item ? iconElement(item.icon, 24, appIcons) : lucideSvg(cur.depth > 0 ? 'CornerUpLeft' : 'Sparkles', 24));
  centerLabel.textContent = item ? item.label : cur.depth > 0 ? '返回' : w.view.name;
  centerHint.textContent = off
    ? '当前不可用'
    : item?.submenu
      ? '划出或停留展开'
      : cur.click
        ? item
          ? '点击执行'
          : cur.depth > 0
            ? '点击返回 · Esc'
            : '点击选择 · Esc 取消'
        : item
          ? '松开执行'
          : cur.depth > 0
            ? '停留返回 · Esc'
            : '移动选择 · 松开取消';
  centerLabel.classList.toggle('dim', off);
}

function applyHover(idx: number | null): void {
  if (!cur || !geom) return;
  const w = cache.get(cur.key);
  if (!w) return;
  const prev = cur.hover;
  if (prev === idx) return;
  cur.hover = idx;
  if (prev !== null && w.sectors[prev]) {
    w.sectors[prev].classList.remove('hover');
    w.sectors[prev].style.transform = '';
    w.icons[prev].classList.remove('hover');
    w.labels[prev].classList.remove('hover');
  }
  const on = idx !== null && !!w.sectors[idx] && !cur.disabled[idx];
  if (idx !== null && w.sectors[idx]) {
    const n = w.view.items.length;
    const a = (idx * Math.PI * 2) / n;
    w.sectors[idx].classList.add('hover');
    w.icons[idx].classList.add('hover');
    w.labels[idx].classList.add('hover');
    if (on) {
      w.sectors[idx].style.transform = `translate(${(4 * Math.sin(a)).toFixed(2)}px, ${(-4 * Math.cos(a)).toFixed(2)}px)`;
      glow.style.setProperty('--mask', w.masks[idx]);
      glow.style.setProperty('--angle-base', `${(a * 180) / Math.PI - 60}deg`);
    }
  }
  glow.classList.toggle('on', on);
  setCenter();
}

/** Back to the likely next wheel, scaled down for the next spring-in. Only while invisible. */
function arm(): void {
  stage.classList.add('no-anim');
  stage.classList.remove('open', 'swap-out', 'swap-in');
  resetVisual(attached !== null ? cache.get(attached) : undefined);
  const w = attach(fallback);
  cur = null;
  if (w) {
    centerIcon.replaceChildren(lucideSvg('Sparkles', 24));
    centerLabel.textContent = w.view.name;
    centerHint.textContent = '移动选择 · 松开取消';
  }
  void stage.offsetWidth;
  stage.classList.remove('no-anim');
}

// ------------------------------------------------------------------------------------------------
// Frame timing (diagnostics)
// ------------------------------------------------------------------------------------------------

const frames = { deltas: [] as number[], last: 0, running: false };

function loop(ts: number): void {
  if (!frames.running) return;
  if (frames.last) frames.deltas.push(ts - frames.last);
  frames.last = ts;
  requestAnimationFrame(loop);
}

function startFrames(): void {
  frames.deltas = [];
  frames.last = 0;
  if (!frames.running) {
    frames.running = true;
    requestAnimationFrame(loop);
  }
}

function stopFrames(seq: number, hz: number): void {
  frames.running = false;
  const d = frames.deltas.slice(2);
  if (d.length < 5) return;
  const sorted = [...d].sort((a, b) => a - b);
  const q = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
  const dropped = d.filter((x) => x > (1000 / (hz || 60)) * 1.5).length;
  void invoke('overlay_metric', {
    kind: 'frames',
    data: { seq, hz, frames: d.length, p50_ms: +q(0.5).toFixed(2), p95_ms: +q(0.95).toFixed(2), dropped },
  });
}

// ------------------------------------------------------------------------------------------------
// Events from the wheel module
// ------------------------------------------------------------------------------------------------

interface ShowPayload {
  seq: number;
  wheel: WheelView;
  disabled: boolean[];
  click: boolean;
  depth: number;
  hz: number;
  t0_ms: number;
}

void listen<ShowPayload>('wheel:show', ({ payload: p }) => {
  const tRecv = performance.now();
  const prebuilt = attached === p.wheel.key && cur === null;
  if (cur !== null) arm(); // a new session began before the previous one finished hiding
  const w = attach(p.wheel.key);
  if (!w) return;
  resetVisual(w);
  cur = { seq: p.seq, key: p.wheel.key, hover: null, click: p.click, depth: p.depth, disabled: p.disabled, hz: p.hz };
  applyDisabled(w, p.disabled);
  setCenter();
  stage.classList.add('open');
  startFrames();
  requestAnimationFrame(() =>
    requestAnimationFrame(() =>
      invoke('overlay_metric', {
        kind: 'painted',
        data: { seq: p.seq, prebuilt, visibility: document.visibilityState, rust_to_page_ms: +p.t0_ms.toFixed(2), page_ms: +(performance.now() - tRecv).toFixed(2) },
      }),
    ),
  );
});

void listen<{ seq: number; idx: number | null }>('wheel:hover', ({ payload }) => {
  if (cur?.seq === payload.seq) applyHover(payload.idx);
});

void listen<{ seq: number; click: boolean }>('wheel:mode', ({ payload }) => {
  if (cur?.seq !== payload.seq) return;
  cur.click = payload.click;
  setCenter();
});

void listen<{ seq: number; disabled: boolean[] }>('wheel:disabled', ({ payload }) => {
  if (cur?.seq !== payload.seq) return;
  cur.disabled = payload.disabled;
  const w = cache.get(cur.key);
  if (w) applyDisabled(w, payload.disabled);
  const h = cur.hover;
  cur.hover = null;
  applyHover(h);
});

void listen<{ seq: number; wheel: WheelView; disabled: boolean[]; depth: number }>('wheel:swap', ({ payload }) => {
  if (cur?.seq !== payload.seq) return;
  const going = payload.depth >= cur.depth ? 'in' : 'out';
  stage.classList.remove('swap-in', 'swap-out-back', 'swap-in-back');
  stage.classList.add(going === 'in' ? 'swap-out' : 'swap-out-back');
  const seq = payload.seq;
  setTimeout(() => {
    if (cur?.seq !== seq) return;
    resetVisual(cache.get(cur.key));
    const w = attach(payload.wheel.key);
    if (!w) return;
    cur.key = payload.wheel.key;
    cur.hover = null;
    cur.depth = payload.depth;
    cur.disabled = payload.disabled;
    applyDisabled(w, payload.disabled);
    setCenter();
    stage.classList.remove('swap-out', 'swap-out-back');
    void stage.offsetWidth;
    stage.classList.add(going === 'in' ? 'swap-in' : 'swap-in-back');
    void invoke('overlay_metric', { kind: 'swap', data: { seq, wheel: payload.wheel.key, depth: payload.depth, sectors: w.sectors.length } });
  }, 70);
});

void listen<{ seq: number; flash: boolean }>('wheel:hide', ({ payload }) => {
  if (cur?.seq !== payload.seq) return;
  if (payload.flash && cur.hover !== null) cache.get(cur.key)?.sectors[cur.hover]?.classList.add('flash');
  const { seq, hz } = cur;
  setTimeout(() => stopFrames(seq, hz), 100);
});

void listen<{ seq: number }>('wheel:hidden', async ({ payload }) => {
  if (cur !== null && cur.seq !== payload.seq) return;
  if (pendingRebuild) {
    pendingRebuild = false;
    await loadSnapshot().catch(report);
  }
  arm();
});

void listen<Record<string, string>>('wheel:icons', ({ payload }) => {
  appIcons = { ...appIcons, ...payload };
  refreshAppIcons();
});

void listen('wheel:config', async () => {
  // Never rebuild under the user's eyes; wait until the wheel is hidden.
  if (cur !== null) {
    pendingRebuild = true;
    return;
  }
  await loadSnapshot().catch(report);
  arm();
});

// ------------------------------------------------------------------------------------------------
// Look
// ------------------------------------------------------------------------------------------------

const mq = window.matchMedia('(prefers-color-scheme: light)');
let settings: Settings | null = null;

function applySettings(s: Settings): void {
  settings = s;
  const light = s.theme === 'light' || (s.theme === 'auto' && mq.matches);
  document.documentElement.classList.toggle('light', light);
  const custom = s.accent && s.accent !== 'system' && CSS.supports('color', s.accent);
  if (custom) {
    document.documentElement.style.setProperty('--accent-a', s.accent);
    document.documentElement.style.setProperty('--accent-b', `color-mix(in oklch, ${s.accent} 55%, #22d3ee)`);
    document.documentElement.style.setProperty('--accent-c', `color-mix(in oklch, ${s.accent} 55%, #f472b6)`);
  } else {
    for (const v of ['--accent-a', '--accent-b', '--accent-c']) document.documentElement.style.removeProperty(v);
  }
}

mq.addEventListener('change', () => settings && applySettings(settings));
void listen<Settings>('settings', ({ payload }) => applySettings(payload));

// ------------------------------------------------------------------------------------------------

function report(e: unknown): void {
  void invoke('overlay_metric', { kind: 'error', data: { message: String((e as Error)?.stack ?? e) } });
}

window.addEventListener('error', (ev) => report(ev.error ?? ev.message));
window.addEventListener('unhandledrejection', (ev) => report(ev.reason));

(async () => {
  try {
    const st = await invoke<{ settings: Settings }>('status');
    applySettings(st.settings);
    // The wheel module may still be starting; retry briefly.
    for (let i = 0; ; i++) {
      try {
        await loadSnapshot();
        break;
      } catch (e) {
        if (i > 40) throw e;
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    arm();
  } catch (e) {
    report(e);
  }
})();
