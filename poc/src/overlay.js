import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  createElement,
  AppWindow,
  Bell,
  Calculator,
  Circle,
  Copy,
  Crop,
  Eye,
  FileArchive,
  FolderPlus,
  Globe,
  LayoutDashboard,
  Link,
  Scissors,
  Sparkles,
  SquareTerminal,
  Trash2,
  Type,
} from 'lucide';
import { sectorPath, iconPos } from './geometry.js';
import { createGlGlow } from './glow-gl.js';

// Only the icons the PoC wheels use: importing all of lucide adds ~450 kB to parse.
const icons = {
  AppWindow, Bell, Calculator, Circle, Copy, Crop, Eye, FileArchive, FolderPlus, Globe,
  LayoutDashboard, Link, Scissors, Sparkles, SquareTerminal, Trash2, Type,
};

const SVGNS = 'http://www.w3.org/2000/svg';
const $ = (id) => document.getElementById(id);
const stage = $('stage');
const svg = $('wheel');
const glowCss = $('glow-css');
const glowCanvas = $('glow-gl');
const centerIcon = $('center-icon');
const centerLabel = $('center-label');
const centerHint = $('center-hint');

/*
 * Latency model: the native window is invisible (alpha 0) between sessions while this page keeps the
 * DEFAULT wheel fully rendered in its "armed" state. On trigger the native side fades the window in
 * immediately, so the first visible frame never waits for this page. The page's own work (spring
 * scale-in, center text, a different wheel) lands a frame or two later while the window is still
 * fading in. After the window is invisible again (`wheel:hidden`) the page resets to armed.
 */

let cfg = { glow: 'css' };
let gl = null;
let geom = null;
const cache = new Map(); // wheel id -> { view, root: <g>, sectors, icons, labels }
let attached = null; // wheel id currently in the svg
let cur = null; // { seq, wid, hover, click, frame, angle }
const visLog = []; // [performance.now(), visibilityState]

document.addEventListener('visibilitychange', () => visLog.push([performance.now(), document.visibilityState]));

// ---------------------------------------------------------------- building (startup only)

function icon(name, size) {
  return createElement(icons[name] ?? icons.Circle, { width: size, height: size, 'stroke-width': 1.8 });
}

function el(tag, attrs = {}, parent) {
  const e = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  parent?.appendChild(e);
  return e;
}

/** Static grain texture, rendered once. Replaces a live feTurbulence filter (expensive to rasterize). */
function noiseDataUrl(size = 160) {
  const c = document.createElement('canvas');
  c.width = c.height = size;
  const ctx = c.getContext('2d');
  const img = ctx.createImageData(size, size);
  for (let i = 0; i < img.data.length; i += 4) {
    const v = (Math.random() * 255) | 0;
    img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
    img.data[i + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  return c.toDataURL('image/png');
}

function buildDefs(noise) {
  const defs = el('defs', {}, svg);
  const hl = el('linearGradient', { id: 'hl', x1: 0, y1: 0, x2: 0, y2: 1 }, defs);
  el('stop', { offset: '0%', 'stop-color': 'rgba(255,255,255,0.32)' }, hl);
  el('stop', { offset: '55%', 'stop-color': 'rgba(255,255,255,0.06)' }, hl);
  el('stop', { offset: '100%', 'stop-color': 'rgba(255,255,255,0.14)' }, hl);
  const pat = el('pattern', { id: 'grain', width: 160, height: 160, patternUnits: 'userSpaceOnUse' }, defs);
  el('image', { href: noise, width: 160, height: 160 }, pat);
  return defs;
}

function buildWheel(view, defs) {
  const n = view.items.length;
  const c = geom.win / 2;
  const root = el('g', { class: 'wheel-root' });
  const clip = el('clipPath', { id: `clip-${view.id}` }, defs);
  const sectors = [];
  const icons_ = [];
  const labels = [];
  view.items.forEach((_, i) => {
    const d = sectorPath(i, n, geom);
    el('path', { d }, clip);
    sectors.push(el('path', { d, class: 'sector' }, root));
    el('path', { d, class: 'sector-hl' }, root);
  });
  el('circle', { cx: c, cy: c, r: geom.r_center }, clip);
  el('circle', { cx: c, cy: c, r: geom.r_center, class: 'center-disc' }, root);
  el('circle', { cx: c, cy: c, r: geom.r_center - 0.5, fill: 'none', stroke: 'url(#hl)', 'stroke-width': 1 }, root);
  el('rect', { x: 0, y: 0, width: geom.win, height: geom.win, fill: 'url(#grain)', 'clip-path': `url(#clip-${view.id})`, class: 'noise' }, root);
  view.items.forEach((item, i) => {
    const [x, y] = iconPos(i, n, geom);
    const g = el('g', { class: 'item-icon', transform: `translate(${x - 11} ${y - 17})` }, root);
    g.appendChild(icon(item.icon, 22));
    icons_.push(g);
    const t = el('text', { x, y: y + 18, class: 'item-label' }, root);
    t.textContent = item.label.length > 6 ? item.label.slice(0, 6) + '…' : item.label;
    labels.push(t);
  });
  // Pre-render the per-sector glow masks too, so hover never builds strings.
  const masks = view.items.map((_, i) => maskFor(i, n));
  return { view, root, sectors, icons: icons_, labels, masks };
}

function maskFor(i, n) {
  const d = sectorPath(i, n, geom);
  const s = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 ${geom.win} ${geom.win}'><path d='${d}' fill='none' stroke='black' stroke-width='2.2' stroke-linejoin='round'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(s)}")`;
}

// ---------------------------------------------------------------- state changes (cheap)

function attach(wid) {
  if (attached === wid) return;
  const w = cache.get(wid);
  if (attached != null) cache.get(attached).root.remove();
  svg.appendChild(w.root);
  attached = wid;
}

function clearHover() {
  const w = attached != null && cache.get(attached);
  if (w) {
    for (const s of w.root.querySelectorAll('.hover, .flash')) s.classList.remove('hover', 'flash');
    for (const s of w.sectors) s.style.transform = '';
  }
  glowCss.classList.remove('on');
  glowCanvas.classList.remove('on');
}

function setCenter(view, idx, click) {
  const item = idx != null ? view.items[idx] : null;
  centerIcon.replaceChildren(icon(item ? item.icon : 'Sparkles', 24));
  centerLabel.textContent = item ? item.label : view.name;
  centerHint.textContent = click ? '点击选择 · Esc 取消' : item ? '松开执行' : '移动选择 · 松开取消';
}

/** Back to the pre-rendered default wheel, scaled down for the next spring-in. Window is invisible. */
function arm() {
  stage.classList.add('no-anim');
  stage.classList.remove('open');
  svg.classList.remove('swap-out', 'swap-in');
  clearHover();
  attach('default');
  setCenter(cache.get('default').view, null, false);
  void stage.offsetWidth;
  stage.classList.remove('no-anim');
  cur = null;
}

function applyHover(idx) {
  const w = cache.get(cur.wid);
  const prev = cur.hover;
  if (prev === idx) return;
  cur.hover = idx;
  if (prev != null) {
    w.sectors[prev].classList.remove('hover');
    w.sectors[prev].style.transform = '';
    w.icons[prev].classList.remove('hover');
    w.labels[prev].classList.remove('hover');
  }
  if (idx != null) {
    const n = w.view.items.length;
    const a = (idx * Math.PI * 2) / n;
    w.sectors[idx].classList.add('hover');
    w.sectors[idx].style.transform = `translate(${(4 * Math.sin(a)).toFixed(2)}px, ${(-4 * Math.cos(a)).toFixed(2)}px)`;
    w.icons[idx].classList.add('hover');
    w.labels[idx].classList.add('hover');
    if (cfg.glow === 'css') {
      glowCss.style.setProperty('--mask', w.masks[idx]);
      glowCss.style.setProperty('--angle-base', `${(a * 180) / Math.PI - 60}deg`);
    }
  }
  glowCss.classList.toggle('on', cfg.glow === 'css' && idx != null);
  glowCanvas.classList.toggle('on', cfg.glow === 'webgl' && idx != null);
  setCenter(w.view, idx, cur.click);
}

// ---------------------------------------------------------------- frame timing (P4)

const frames = { deltas: [], last: 0, running: false, glAlpha: 0, t0: 0 };

function loop(ts) {
  if (!frames.running) return;
  if (frames.last) frames.deltas.push(ts - frames.last);
  frames.last = ts;
  if (cfg.glow === 'webgl' && gl && cur) {
    const target = cur.hover != null ? 1 : 0;
    frames.glAlpha += (target - frames.glAlpha) * 0.25;
    if (cur.hover != null) {
      const n = cache.get(cur.wid).view.items.length;
      cur.glMid = (cur.hover * Math.PI * 2) / n;
      cur.glHalf = Math.PI / n - Math.asin(geom.gap / 2 / geom.r_out);
    }
    if (cur.glMid != null) {
      gl.draw({ t: (ts - frames.t0) / 1000, mid: cur.glMid, halfw: cur.glHalf, geom, alpha: frames.glAlpha, cursorA: ((cur.angle ?? 0) * Math.PI) / 180 });
    }
  }
  requestAnimationFrame(loop);
}

function startFrames() {
  frames.deltas = [];
  frames.last = 0;
  frames.glAlpha = 0;
  frames.t0 = performance.now();
  if (!frames.running) {
    frames.running = true;
    requestAnimationFrame(loop);
  }
}

function stopFrames(seq, hz) {
  frames.running = false;
  gl?.clear();
  const d = frames.deltas.slice(2); // skip the first frames after show (compositor warm-up)
  if (d.length < 5) return;
  const sorted = [...d].sort((a, b) => a - b);
  const q = (p) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))];
  const expected = 1000 / (hz || 60);
  const dropped = d.filter((x) => x > expected * 1.5).length;
  invoke('overlay_frames', {
    stats: {
      seq,
      renderer: cfg.glow,
      hz,
      frames: d.length,
      p50_ms: +q(0.5).toFixed(2),
      p95_ms: +q(0.95).toFixed(2),
      max_ms: +sorted[sorted.length - 1].toFixed(2),
      dropped,
      dropped_pct: +((100 * dropped) / d.length).toFixed(1),
      fps: +(1000 / q(0.5)).toFixed(1),
    },
  });
}

// ---------------------------------------------------------------- events

function reportPainted(seq, kind, tRecv, info) {
  const tHandled = performance.now();
  requestAnimationFrame((raf1) =>
    requestAnimationFrame(() =>
      invoke('overlay_painted', {
        seq,
        kind,
        jsMs: +(performance.now() - tRecv).toFixed(2),
        info: { ...info, handler_ms: +(tHandled - tRecv).toFixed(2), recv_to_raf1_ms: +(raf1 - tRecv).toFixed(2) },
      }),
    ),
  );
}

listen('wheel:show', ({ payload }) => {
  const tRecv = performance.now();
  const { seq, wheel: wid, frame } = payload;
  const lastVis = visLog.at(-1);
  const info = {
    visibility: document.visibilityState,
    // How long the page has been in its current visibility state (null = never changed since load).
    vis_age_ms: lastVis ? +(tRecv - lastVis[0]).toFixed(0) : null,
    prebuilt: attached === wid && cur == null,
  };
  if (cur != null) arm(); // a new session started before the previous one finished hiding
  attach(wid);
  cur = { seq, wid, hover: null, click: false, frame, angle: 0 };
  setCenter(cache.get(wid).view, null, false);
  stage.classList.add('open'); // spring 0.94 → 1 while the native window fades in
  startFrames();
  reportPainted(seq, 'show', tRecv, info);
});

listen('wheel:hover', ({ payload }) => {
  if (!cur || payload.seq !== cur.seq) return;
  cur.angle = payload.angle;
  applyHover(payload.idx);
});

listen('wheel:mode', ({ payload }) => {
  if (!cur || payload.seq !== cur.seq) return;
  cur.click = payload.click;
  setCenter(cache.get(cur.wid).view, cur.hover, cur.click);
});

listen('wheel:swap', ({ payload }) => {
  if (!cur || payload.seq !== cur.seq) return;
  const tRecv = performance.now();
  svg.classList.add('swap-out');
  setTimeout(() => {
    if (!cur || payload.seq !== cur.seq) return;
    clearHover();
    cur.hover = null;
    cur.wid = payload.wheel;
    attach(payload.wheel);
    setCenter(cache.get(cur.wid).view, null, cur.click);
    if (payload.dir) centerHint.textContent = payload.dir.replace(/\/$/, '').split('/').pop() || '/';
    svg.classList.remove('swap-out');
    svg.classList.add('swap-in');
    reportPainted(payload.seq, 'swap', tRecv, {});
  }, 70);
});

listen('wheel:hide', ({ payload }) => {
  if (!cur || payload.seq !== cur.seq) return;
  // The native side fades the window out; only the executed sector flashes here.
  if (payload.flash && cur.hover != null) cache.get(cur.wid).sectors[cur.hover].classList.add('flash');
  const { seq } = cur;
  const hz = cur.frame?.hz;
  setTimeout(() => stopFrames(seq, hz), 100);
});

listen('wheel:hidden', ({ payload }) => {
  if (cur && payload.seq === cur.seq) arm();
});

listen('cfg', ({ payload }) => {
  cfg = payload;
  if (cfg.glow === 'webgl' && !gl) gl = createGlGlow(glowCanvas);
});

(async () => {
  try {
    const st = await invoke('status');
    cfg = st.cfg;
    if (cfg.glow === 'webgl') gl = createGlGlow(glowCanvas);
    const all = await invoke('wheels');
    geom = all.geom;
    const defs = buildDefs(noiseDataUrl());
    for (const view of all.wheels) cache.set(view.id, buildWheel(view, defs));
    // Armed: default wheel fully rendered while the window is invisible.
    arm();
  } catch (e) {
    reportError('init', e);
  }
  await invoke('overlay_ready');
})();

function reportError(where, e) {
  invoke('overlay_error', { where, message: String(e?.stack || e?.message || e) }).catch(() => {});
}
window.addEventListener('error', (ev) => reportError('window', ev.error || ev.message));
window.addEventListener('unhandledrejection', (ev) => reportError('promise', ev.reason));
