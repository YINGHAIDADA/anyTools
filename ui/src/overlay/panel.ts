/*
 * Clipboard panel, drawn in the shared overlay window (crates/modules/clipboard).
 *
 * The window never takes focus or mouse events: the module reads keys and the mouse through the
 * global hook, keeps the state (query, selection, scroll) and sends what to draw. This page only
 * renders, then confirms (`ready`) so the native side fades the window in on a finished frame. The row
 * layout mirrors `panel::GEOM` in Rust, which hit-tests the mouse against it.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { lucideSvg } from '../shared/icons';

interface PanelGeom {
  w: number;
  pad: number;
  search_h: number;
  row_h: number;
  rows: number;
  foot_h: number;
  radius: number;
}

type Kind = 'link' | 'color' | 'number' | 'lines' | 'text';

interface Row {
  index: number;
  id: number;
  text: string;
  kind: Kind;
  lines: number;
  chars: number;
  app: string | null;
  age_ms: number;
}

interface View {
  seq: number;
  geom: PanelGeom;
  query: string;
  total: number;
  matches: number;
  top: number;
  sel: number;
  rows: Row[];
}

const KIND_ICON: Record<Kind, string> = { link: 'Link', color: 'Palette', number: 'Hash', lines: 'AlignLeft', text: 'Type' };

const root = document.documentElement;
const panel = document.getElementById('panel')!;
const queryEl = document.getElementById('panel-query')!;
const countEl = document.getElementById('panel-count')!;
const listEl = document.getElementById('panel-list')!;
const scrollEl = document.getElementById('panel-scroll')!;
const emptyEl = document.getElementById('panel-empty')!;

let seq = -1;
/** Row nodes, reused across renders (one per visible slot). */
const slots: HTMLElement[] = [];
document.getElementById('panel-search-icon')!.appendChild(lucideSvg('Search', 17, 2));

function age(ms: number): string {
  const m = Math.floor(ms / 60000);
  if (m < 1) return '刚刚';
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  const d = Math.floor(h / 24);
  return d === 1 ? '昨天' : `${d} 天前`;
}

/** `text` with every query term wrapped in <mark> (case-insensitive). */
function highlighted(text: string, query: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  const terms = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) {
    frag.append(text);
    return frag;
  }
  const lower = text.toLowerCase();
  const hit = new Array<boolean>(text.length).fill(false);
  for (const t of terms) {
    for (let i = lower.indexOf(t); i >= 0; i = lower.indexOf(t, i + t.length)) hit.fill(true, i, i + t.length);
  }
  let i = 0;
  while (i < text.length) {
    let j = i;
    while (j < text.length && hit[j] === hit[i]) j++;
    const part = text.slice(i, j);
    if (hit[i]) {
      const m = document.createElement('mark');
      m.textContent = part;
      frag.append(m);
    } else frag.append(part);
    i = j;
  }
  return frag;
}

function slot(i: number): HTMLElement {
  while (slots.length <= i) {
    const r = document.createElement('div');
    r.className = 'prow';
    r.innerHTML = '<div class="prow-ring"></div><div class="prow-icon"></div><div class="prow-text"></div><div class="prow-meta"></div><div class="prow-key"></div>';
    listEl.appendChild(r);
    slots.push(r);
  }
  return slots[i];
}

function renderRow(el: HTMLElement, r: Row, v: View, visible: number): void {
  el.hidden = false;
  el.classList.toggle('sel', r.index === v.sel);
  const [icon, text, meta, key] = [1, 2, 3, 4].map((k) => el.children[k] as HTMLElement);
  const color = r.kind === 'color' ? r.text.trim() : null;
  if (color && CSS.supports('color', color)) {
    const sw = document.createElement('i');
    sw.className = 'swatch';
    sw.style.background = color;
    icon.replaceChildren(sw);
  } else {
    icon.replaceChildren(lucideSvg(KIND_ICON[r.kind], 15, 1.9));
  }
  text.replaceChildren(highlighted(r.text, v.query));
  const bits = [r.kind === 'lines' ? `${r.lines} 行` : r.chars > 80 ? `${r.chars} 字` : '', r.app ?? '', age(r.age_ms)].filter(Boolean);
  meta.textContent = bits.join(' · ');
  key.textContent = r.index === v.sel ? '↩' : visible < 9 ? `⌘${visible + 1}` : '';
}

function render(v: View): void {
  const g = v.geom;
  panel.style.setProperty('--pw', `${g.w}px`);
  panel.style.setProperty('--pad', `${g.pad}px`);
  panel.style.setProperty('--search-h', `${g.search_h}px`);
  panel.style.setProperty('--row-h', `${g.row_h}px`);
  panel.style.setProperty('--rows', String(g.rows));
  panel.style.setProperty('--foot-h', `${g.foot_h}px`);
  panel.style.setProperty('--radius', `${g.radius}px`);

  queryEl.textContent = v.query;
  queryEl.classList.toggle('placeholder', !v.query);
  if (!v.query) queryEl.textContent = '搜索剪贴板';
  countEl.textContent = v.query ? `${v.matches} / ${v.total}` : v.total ? `${v.total} 条` : '';

  v.rows.forEach((r, i) => renderRow(slot(i), r, v, i));
  for (let i = v.rows.length; i < slots.length; i++) slots[i].hidden = true;

  emptyEl.hidden = v.rows.length > 0;
  emptyEl.textContent = v.total === 0 ? '复制的文字会出现在这里' : `没有包含「${v.query}」的记录`;

  // Scroll indicator, only when the list is longer than the panel.
  const over = v.matches > g.rows;
  scrollEl.hidden = !over;
  if (over) {
    const h = Math.max(0.12, g.rows / v.matches);
    scrollEl.style.setProperty('--thumb-h', String(h));
    scrollEl.style.setProperty('--thumb-y', String((v.top / (v.matches - g.rows)) * (1 - h)));
  }
}

/** Called by main.ts so the wheel and the panel never draw at the same time. */
export function panelActive(): boolean {
  return root.classList.contains('panel-mode');
}

export function leavePanelMode(): void {
  root.classList.remove('panel-mode', 'panel-open');
  seq = -1;
}

void listen<View & { frame: unknown }>('clip:show', ({ payload: v }) => {
  seq = v.seq;
  root.classList.add('panel-mode');
  root.classList.remove('panel-open');
  render(v);
  // Two frames: the new content is composited before the window fades in.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      if (seq !== v.seq) return;
      root.classList.add('panel-open');
      void invoke('module_call', { module: 'clipboard', msg: { type: 'ready', seq: v.seq } });
    }),
  );
});

void listen<View>('clip:render', ({ payload: v }) => {
  if (v.seq === seq) render(v);
});

void listen<{ seq: number }>('clip:hide', ({ payload }) => {
  if (payload.seq === seq) root.classList.remove('panel-open');
});

void listen<{ seq: number }>('clip:hidden', ({ payload }) => {
  if (payload.seq === seq) leavePanelMode();
});
