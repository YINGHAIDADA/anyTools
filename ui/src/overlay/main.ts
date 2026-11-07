/*
 * Wheel overlay page.
 *
 * Latency model (docs/M0-RESULTS.md): the native window is invisible (alpha 0) between sessions, and
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
import { createLight, palette, type LightRenderer, type LightState } from './light';
import { leavePanelMode } from './panel';

const SVGNS = 'http://www.w3.org/2000/svg';
const $ = <T extends Element = HTMLElement>(id: string) => document.getElementById(id) as unknown as T;
const stage = $('stage');
// Layers, bottom to top: sector bodies (svg) → light (WebGL) → icons/labels (svgFg) → center text.
const svg = $<SVGSVGElement>('wheel');
const svgFg = $<SVGSVGElement>('wheel-fg');
const lightCanvas = $<HTMLCanvasElement>('light');
const glow = $('glow');
const centerIcon = $('center-icon');
const centerLabel = $('center-label');
const centerHint = $('center-hint');

interface Built {
  view: WheelView;
  root: SVGGElement;
  fg: SVGGElement;
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

// Light layer state. h0 = hovered sector (fades in, light pours in from the rim), h1 = the previous one
// fading out.
let light: LightRenderer | null = null;
const lt = {
  h0: -1,
  h0Since: 0,
  h1: -1,
  h1Since: 0,
  flashAt: -1,
  cursorAngle: 0,
  cursorDist: 0,
  liquid: false,
  glassAvailable: false,
  light: false,
  palette: palette(undefined),
  reducedMotion: window.matchMedia('(prefers-reduced-motion: reduce)').matches,
};
const FADE_IN = 220;
const FADE_OUT = 260;
const FLASH = 220;

function lightState(now: number): LightState {
  const n = cache.get(cur?.key ?? attached ?? '')?.view.items.length ?? 8;
  const ease = (x: number) => 1 - Math.pow(1 - Math.min(Math.max(x, 0), 1), 3);
  return {
    n,
    h0: lt.h0,
    h0Amount: lt.h0 >= 0 ? ease((now - lt.h0Since) / FADE_IN) : 0,
    h0Age: (now - lt.h0Since) / 1000,
    h1: lt.h1,
    h1Amount: lt.h1 >= 0 ? 1 - ease((now - lt.h1Since) / FADE_OUT) : 0,
    flash: lt.flashAt >= 0 ? Math.max(0, 1 - (now - lt.flashAt) / FLASH) : 0,
    light: lt.light,
    palette: lt.palette,
    // Reduced motion: the light still fades and pours in, but stops flowing.
    time: lt.reducedMotion ? 0 : now / 1000,
  };
}

function drawLight(now = performance.now()): void {
  if (!light) return;
  try {
    light.draw(lightState(now));
  } catch (e) {
    report(e);
    disableLight();
  }
}

function disableLight(): void {
  light = null;
  document.documentElement.classList.remove('gl');
}

/** Moves the light to sector `idx` (or none). The old one fades out. */
function lightHover(idx: number | null): void {
  const next = idx ?? -1;
  if (next === lt.h0) return;
  const now = performance.now();
  lt.h1 = lt.h0;
  lt.h1Since = now;
  lt.h0 = next;
  lt.h0Since = now;
}

function lightReset(): void {
  lt.h0 = lt.h1 = -1;
  lt.flashAt = -1;
}

// ------------------------------------------------------------------------------------------------
// Building (startup / config change only)
// ------------------------------------------------------------------------------------------------

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number> = {}, parent?: Element) {
  const e = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  parent?.appendChild(e);
  return e;
}

function buildDefs(): SVGDefsElement {
  return el('defs', {}, svg);
}

function maskFor(i: number, n: number, g: Geom): string {
  const s = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${g.win} ${g.win}'><path d='${sectorPath(i, n, g)}' fill='none' stroke='black' stroke-width='2.4' stroke-linejoin='round'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(s)}")`;
}

function short(label: string): string {
  return [...label].length > 6 ? [...label].slice(0, 6).join('') + '…' : label;
}

function buildWheel(view: WheelView, g: Geom, _defs: SVGDefsElement): Built {
  const n = view.items.length;
  const c = g.win / 2;
  const root = el('g', { class: 'wheel-root' });
  const fg = el('g', { class: 'wheel-fg' });
  const sectors: SVGPathElement[] = [];
  const icons: SVGGElement[] = [];
  const labels: SVGTextElement[] = [];
  view.items.forEach((_, i) => {
    sectors.push(el('path', { d: sectorPath(i, n, g), class: 'sector' }, root));
  });
  el('circle', { cx: c, cy: c, r: g.r_center, class: 'center-disc' }, root);
  // On a single Liquid Glass disc, hairlines are all that separate the sectors (hidden on classic glass,
  // which is already cut to the sector shapes). Inset from both rings so they read as dividers, not a grid.
  for (let i = 0; i < n; i++) {
    const a = ((i + 0.5) * Math.PI * 2) / n;
    const [r0, r1] = [g.r_in + 10, g.r_out - 12];
    el('line', { x1: c + r0 * Math.sin(a), y1: c - r0 * Math.cos(a), x2: c + r1 * Math.sin(a), y2: c - r1 * Math.cos(a), class: 'divider' }, root);
  }
  el('circle', { cx: c, cy: c, r: (g.r_in + g.r_center) / 2, class: 'divider' }, root);

  view.items.forEach((item, i) => {
    const [x, y] = polar(i, n, g);
    // Position on an outer group (SVG attribute), hover scale on an inner one (CSS). A CSS `transform`
    // replaces the SVG `transform` attribute rather than composing with it, so they can't share a node.
    const pos = el('g', { transform: `translate(${(x - 11).toFixed(2)} ${(y - 17).toFixed(2)})` }, fg);
    const ig = el('g', { class: 'item-icon' }, pos);
    ig.appendChild(iconElement(item.icon, 22, appIcons));
    icons.push(ig);
    const t = el('text', { x: x.toFixed(2), y: (y + 18).toFixed(2), class: 'item-label' }, fg);
    t.textContent = short(item.label);
    labels.push(t);
    if (item.submenu) {
      // A small chevron at the outer edge marks submenus.
      const [cx, cy] = polar(i, n, g, g.r_out - 9);
      const a = (i * 360) / n;
      el('path', { d: 'M-3,1.5 L0,-1.5 L3,1.5', class: 'submenu-mark', transform: `translate(${cx.toFixed(2)} ${cy.toFixed(2)}) rotate(${a})` }, fg);
    }
  });
  return { view, root, fg, sectors, icons, labels, masks: view.items.map((_, i) => maskFor(i, n, g)) };
}

async function loadSnapshot(): Promise<void> {
  const snap = await invoke<Snapshot | null>('module_call', { module: 'wheel', msg: { type: 'snapshot' } });
  if (!snap) throw new Error('wheel module not ready');
  geom = snap.geom;
  fallback = snap.fallback;
  svg.replaceChildren();
  svgFg.replaceChildren();
  attached = null;
  cache.clear();
  defs = buildDefs();
  for (const v of snap.wheels) cache.set(v.key, buildWheel(v, geom, defs));
  if (!light && !document.documentElement.classList.contains('nogl')) {
    try {
      light = createLight(lightCanvas, geom);
    } catch (e) {
      report(e);
      light = null;
    }
    document.documentElement.classList.toggle('gl', !!light);
    document.documentElement.classList.toggle('nogl', !light);
    lightCanvas.addEventListener('webglcontextlost', (ev) => (ev.preventDefault(), disableLight()));
  }
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

/** Dynamic wheels (a menu action's choices) are computed when opened; build them on the spot. */
function ensure(view: WheelView): void {
  if (!view.dynamic || !geom || !defs) return;
  const old = cache.get(view.key);
  if (old && attached === view.key) {
    old.root.remove();
    old.fg.remove();
    attached = null;
  }
  cache.set(view.key, buildWheel(view, geom, defs));
}

function attach(key: string): Built | undefined {
  const w = cache.get(key);
  if (!w) return undefined;
  if (attached !== key) {
    if (attached !== null) {
      const old = cache.get(attached);
      old?.root.remove();
      old?.fg.remove();
    }
    svg.appendChild(w.root);
    svgFg.appendChild(w.fg);
    attached = key;
  }
  return w;
}

function resetVisual(w: Built | undefined): void {
  if (w) {
    for (const root of [w.root, w.fg]) {
      for (const e of root.querySelectorAll('.hover, .flash, .disabled')) e.classList.remove('hover', 'flash', 'disabled');
    }
  }
  glow.classList.remove('on');
  lightReset();
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
    if (on && !light) {
      // CSS fallback glow (no WebGL).
      glow.style.setProperty('--mask', w.masks[idx]);
      glow.style.setProperty('--angle-base', `${(a * 180) / Math.PI - 60}deg`);
    }
  }
  glow.classList.toggle('on', on && !light);
  lightHover(on ? idx : null);
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
  // The armed frame must already show the glass highlights.
  drawLight();
}

// ------------------------------------------------------------------------------------------------
// Frame timing (diagnostics)
// ------------------------------------------------------------------------------------------------

const frames = { deltas: [] as number[], last: 0, running: false };

function loop(ts: number): void {
  if (!frames.running) return;
  if (frames.last) frames.deltas.push(ts - frames.last);
  frames.last = ts;
  drawLight(ts);
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
  // The panel may still be fading out in this window.
  leavePanelMode();
  const prebuilt = attached === p.wheel.key && cur === null;
  if (cur !== null) arm(); // a new session began before the previous one finished hiding
  ensure(p.wheel);
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

void listen<{ seq: number; idx: number | null; angle: number; dist: number }>('wheel:hover', ({ payload }) => {
  if (cur?.seq !== payload.seq) return;
  lt.cursorAngle = (payload.angle * Math.PI) / 180;
  lt.cursorDist = payload.dist;
  applyHover(payload.idx);
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
    ensure(payload.wheel);
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
  if (payload.flash && cur.hover !== null) {
    cache.get(cur.key)?.sectors[cur.hover]?.classList.add('flash');
    lt.flashAt = performance.now();
  }
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

function setLiquid(on: boolean): void {
  lt.liquid = on;
  document.documentElement.classList.toggle('liquid', on);
}

function applySettings(s: Settings): void {
  settings = s;
  const isLight = s.theme === 'light' || (s.theme === 'auto' && mq.matches);
  document.documentElement.classList.toggle('light', isLight);
  lt.light = isLight;
  lt.palette = palette(s.accent);
  // Native Liquid Glass is used when the material asks for it and the system has it.
  setLiquid(lt.glassAvailable && s.material.startsWith('liquid'));
  const custom = s.accent && s.accent !== 'system' && CSS.supports('color', s.accent);
  if (custom) {
    document.documentElement.style.setProperty('--accent-a', s.accent);
    document.documentElement.style.setProperty('--accent-b', `color-mix(in oklch, ${s.accent} 55%, #22d3ee)`);
    document.documentElement.style.setProperty('--accent-c', `color-mix(in oklch, ${s.accent} 55%, #f472b6)`);
  } else {
    for (const v of ['--accent-a', '--accent-b', '--accent-c']) document.documentElement.style.removeProperty(v);
  }
  if (cur === null) drawLight();
}

mq.addEventListener('change', () => settings && applySettings(settings));
void listen<Settings>('settings', ({ payload }) => applySettings(payload));
// The native side says what it actually used (authoritative over the guess from settings).
void listen<{ liquid: boolean }>('overlay:glass', ({ payload }) => {
  setLiquid(payload.liquid);
  if (cur === null) drawLight();
});

// ------------------------------------------------------------------------------------------------

function report(e: unknown): void {
  void invoke('overlay_metric', { kind: 'error', data: { message: String((e as Error)?.stack ?? e) } });
}

window.addEventListener('error', (ev) => report(ev.error ?? ev.message));
window.addEventListener('unhandledrejection', (ev) => report(ev.reason));

(async () => {
  try {
    const st = await invoke<{ settings: Settings; liquid_glass?: boolean }>('status');
    lt.glassAvailable = !!st.liquid_glass;
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
