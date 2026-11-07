import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const $ = (id) => document.getElementById(id);
let events = [];
let cfg = null;

// ---------------------------------------------------------------- stats

function pct(arr, p) {
  if (!arr.length) return null;
  const s = [...arr].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(p * s.length))];
}

const of = (kind, f = () => true) => events.filter((e) => e.kind === kind && f(e));
const num = (v) => typeof v === 'number' && Number.isFinite(v);

/**
 * Key-down → first visible frame, estimated: kernel→hook + hook→native window revealed + one display
 * frame for WindowServer to composite. Only valid when the page was live and the wheel pre-rendered at
 * trigger time (otherwise the first frame may show stale or missing content).
 */
function keyToVisible() {
  const trig = new Map(of('trigger').map((e) => [e.seq, e]));
  const painted = new Map(of('painted', (e) => e.what === 'show').map((e) => [e.seq, e]));
  return of('shown')
    .filter((s) => {
      const p = painted.get(s.seq);
      return p?.info && p.info.visibility === 'visible' && p.info.prebuilt;
    })
    .map((s) => {
      const t = trig.get(s.seq);
      return (t && num(t.hid_to_tap_ms) ? t.hid_to_tap_ms : 0) + s.native_ms + 1000 / (s.hz || 60);
    })
    .filter(num);
}

const ROWS = [
  { group: 'P1', name: '按键 → 首帧可见（估算，ms）', vals: keyToVisible, limit: 50, unit: 'ms', main: true },
  {
    group: '',
    name: ' 唤起时页面可见且已预渲染',
    custom: () => {
      const all = of('painted', (e) => e.what === 'show' && e.info);
      const ok = all.filter((e) => e.info.visibility === 'visible' && e.info.prebuilt).length;
      return { n: all.length, text: `${ok}/${all.length}`, pass: all.length ? ok === all.length : null };
    },
  },
  { group: '', name: ' 钩子 → 页面更新画完（弹簧动画起点）', vals: () => of('painted', (e) => e.what === 'show').map((e) => e.since_trigger_ms).filter(num) },
  { group: '', name: ' 内核事件 → 钩子回调', vals: () => of('trigger').map((e) => e.hid_to_tap_ms).filter(num) },
  { group: '', name: ' 钩子回调耗时（µs）', vals: () => of('trigger').map((e) => e.hook_us).filter(num), limit: 1000, unit: 'µs' },
  { group: '', name: ' 钩子 → 原生窗口开始淡入', vals: () => of('shown').map((e) => e.native_ms).filter(num) },
  {
    group: '',
    name: '不抢焦点（唤起后 150ms）',
    custom: () => {
      // Only sessions started from another app count: if our own panel was frontmost, staying active is expected.
      const before = new Map(of('shown').map((e) => [e.seq, e.we_were_active]));
      const all = of('focus_check', (e) => before.get(e.seq) === false);
      const bad = all.filter((e) => e.anytools_active).length;
      return { n: all.length, text: `${all.length - bad}/${all.length} 次前台未变（从其他应用唤起）`, pass: all.length ? bad === 0 : null };
    },
  },
  {
    group: '',
    name: '动作执行成功',
    custom: () => {
      const all = of('exec');
      const ok = all.filter((e) => e.ok).length;
      return { n: all.length, text: `${ok}/${all.length}`, pass: all.length ? ok === all.length : null };
    },
  },
  { group: 'P3', name: 'Finder 读取（唤起时）', vals: () => of('probe').map((e) => e.exec_ms).filter(num), limit: 80, unit: 'ms', main: true },
  { group: '', name: ' 从按键算起', vals: () => of('probe').map((e) => e.total_ms).filter(num) },
  { group: '', name: ' 连续读取（不唤起）', vals: () => of('probe_bench').flatMap((e) => e.samples), limit: 80, unit: 'ms' },
  {
    group: '',
    name: ' 成功切到文件轮盘',
    custom: () => {
      const all = of('probe', (e) => e.files > 0);
      const sw = all.filter((e) => e.swapped).length;
      return { n: all.length, text: `${sw}/${all.length}（有选中文件时）`, pass: null };
    },
  },
  { group: 'P4', name: '帧间隔 p95 · CSS', vals: () => of('frames', (e) => e.renderer === 'css').map((e) => e.p95_ms) },
  { group: '', name: '帧间隔 p95 · WebGL', vals: () => of('frames', (e) => e.renderer === 'webgl').map((e) => e.p95_ms) },
  { group: '', name: ' 丢帧率 % · CSS', vals: () => of('frames', (e) => e.renderer === 'css').map((e) => e.dropped_pct), limit: 1, unit: '%' },
  { group: '', name: ' 丢帧率 % · WebGL', vals: () => of('frames', (e) => e.renderer === 'webgl').map((e) => e.dropped_pct), limit: 1, unit: '%' },
  { group: '', name: ' 实际帧率（fps）', vals: () => of('frames').map((e) => e.fps) },
];

const fmt = (v) => (v == null ? '–' : v >= 100 ? v.toFixed(0) : v.toFixed(1));

function render() {
  const tb = $('results').querySelector('tbody');
  tb.replaceChildren();
  for (const r of ROWS) {
    const tr = document.createElement('tr');
    const cells = [];
    if (r.custom) {
      const c = r.custom();
      cells.push(r.group, r.name, c.n, c.text, '', '', '', verdict(c.pass));
    } else {
      const v = r.vals();
      const p95 = pct(v, 0.95);
      const pass = r.limit != null && v.length ? p95 <= r.limit : null;
      cells.push(
        r.group,
        r.name,
        v.length,
        fmt(pct(v, 0.5)),
        fmt(p95),
        fmt(v.length ? Math.max(...v) : null),
        r.limit != null ? `p95 ≤ ${r.limit}${r.unit}` : '',
        verdict(pass, v.length < 10 && pass != null ? '样本少' : ''),
      );
    }
    cells.forEach((c, i) => {
      const td = document.createElement('td');
      if (c instanceof Node) td.appendChild(c);
      else td.textContent = c;
      if (i === 0) td.className = 'group';
      tr.appendChild(td);
    });
    tb.appendChild(tr);
  }

  const lines = events.slice(-80).reverse().map((e) => {
    const { kind, ts, ...rest } = e;
    return `${new Date(ts).toLocaleTimeString()}  ${kind.padEnd(13)} ${JSON.stringify(rest)}`;
  });
  $('log').textContent = lines.join('\n');

  const last = of('frames').at(-1);
  const fpsNote = last && last.hz > 61 && last.fps < 70 ? `；屏幕 ${last.hz}Hz 但动画约 ${last.fps}fps（WebKit 可能限制在 60fps）` : '';
  $('build-hint').dataset.fps = fpsNote;
  refreshHint();
}

function verdict(pass, note = '') {
  const s = document.createElement('span');
  if (pass == null) {
    s.textContent = '–';
  } else {
    s.textContent = (pass ? '通过' : '未通过') + (note ? `（${note}）` : '');
    s.className = pass ? (note ? 'warn' : 'ok') : 'bad';
  }
  return s;
}

// ---------------------------------------------------------------- status & config

let buildNote = '';
function refreshHint() {
  $('build-hint').textContent = buildNote + ($('build-hint').dataset.fps || '');
}

async function refreshStatus() {
  const st = await invoke('status');
  $('st-ax').textContent = st.accessibility
    ? '已授权'
    : '未授权 — 到「系统设置 › 隐私与安全性 › 辅助功能」打开 anyTools PoC 的开关（系统只弹一次授权框）';
  $('st-ax').className = st.accessibility ? 'ok' : 'bad';
  $('st-tap').textContent = st.tap ? '运行中（⌥Space）' : '未启动';
  $('st-tap').className = st.tap ? 'ok' : 'bad';
  $('log-path').textContent = st.log_path ?? '';
  buildNote =
    (st.debug_build ? 'Debug 构建：延迟数据可能偏高，最终结论以 release 构建为准。' : 'Release 构建。') +
    ` 显示器：${st.screens.map((s) => `${s.frame.w}×${s.frame.h}@${s.hz}Hz`).join('、')}`;
  refreshHint();
  if (!cfg) {
    cfg = st.cfg;
    $('cfg-material').value = cfg.material;
    $('cfg-strategy').value = cfg.strategy;
    $('cfg-glow').value = cfg.glow;
    $('cfg-probe').checked = cfg.probe;
  }
  return st;
}

async function pushCfg() {
  cfg = {
    material: $('cfg-material').value,
    strategy: $('cfg-strategy').value,
    glow: $('cfg-glow').value,
    probe: $('cfg-probe').checked,
  };
  await invoke('set_cfg', { cfg });
}

for (const id of ['cfg-material', 'cfg-strategy', 'cfg-glow', 'cfg-probe']) $(id).addEventListener('change', pushCfg);

$('btn-ax').onclick = async () => {
  await invoke('request_accessibility');
  refreshStatus();
};
$('btn-ax-open').onclick = () => invoke('open_settings_pane', { which: 'accessibility' });
$('btn-ae-open').onclick = () => invoke('open_settings_pane', { which: 'automation' });
$('btn-tap').onclick = async () => {
  try {
    await invoke('start_hook');
  } catch (e) {
    alert(e);
  }
  refreshStatus();
};
$('btn-clear').onclick = async () => {
  await invoke('clear_events');
  events = [];
  render();
};
$('btn-quit').onclick = () => invoke('quit');
$('btn-bench').onclick = async () => {
  $('bench-out').textContent = '读取中…（先切到 Finder 并选中文件效果更真实）';
  try {
    const r = await invoke('bench_probe', { n: 30 });
    const s = r.samples;
    $('bench-out').textContent = `p50 ${fmt(pct(s, 0.5))}ms · p95 ${fmt(pct(s, 0.95))}ms · 当前目录 ${r.last?.dir ?? '–'} · 选中 ${r.last?.files.length ?? 0} 个`;
  } catch (e) {
    $('bench-out').textContent = `失败：${e}`;
  }
};

listen('metric', ({ payload }) => {
  events.push(payload);
  if (events.length > 2000) events.shift();
  render();
  if (payload.kind.startsWith('tap')) refreshStatus();
});

(async () => {
  events = await invoke('events');
  await refreshStatus();
  render();
  // Accessibility can be granted while we're open; keep polling until the hook runs.
  const timer = setInterval(async () => {
    const st = await refreshStatus();
    if (st.accessibility && !st.tap) {
      try {
        await invoke('start_hook');
      } catch (e) {
        $('st-tap').textContent = `启动失败：${e}`;
      }
    }
    if (st.tap) clearInterval(timer);
  }, 1500);
})();
