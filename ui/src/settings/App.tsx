import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useMemo, useState } from 'react';
import type { Geom, PermState, Status, WheelView } from '../shared/types';
import { Icon } from './Icon';
import { WheelPreview } from './WheelPreview';

type Page = 'general' | 'wheels' | 'config' | 'about';

interface Overview {
  geom: Geom;
  wheels: WheelView[];
  fallback: string;
  profiles: string[];
  triggers: { input: { keys?: { mods: number; key: string } } | { button?: string }; tap: string; wheel: string | null }[];
}

const PAGES: { id: Page; label: string; icon: string }[] = [
  { id: 'general', label: '通用', icon: 'Settings2' },
  { id: 'wheels', label: '快捷轮盘', icon: 'Disc3' },
  { id: 'config', label: '配置文件', icon: 'FileCode2' },
  { id: 'about', label: '关于', icon: 'Info' },
];

export function App() {
  const [page, setPage] = useState<Page>('general');
  const [status, setStatus] = useState<Status | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [icons, setIcons] = useState<Record<string, string>>({});

  const refresh = useCallback(async () => {
    const st = await invoke<Status>('status');
    setStatus(st);
    const ov = await invoke<Overview | null>('module_call', { module: 'wheel', msg: { type: 'overview' } });
    if (ov) setOverview(ov);
  }, []);

  useEffect(() => {
    void refresh();
    void invoke<Record<string, string> | null>('module_call', { module: 'wheel', msg: { type: 'icons' } }).then((i) => i && setIcons(i));
    const un = listen('core', () => void refresh());
    // Permissions are granted in System Settings; poll while the window is open.
    const t = setInterval(() => void refresh(), 1500);
    return () => {
      clearInterval(t);
      void un.then((f) => f());
    };
  }, [refresh]);

  const needsSetup = status !== null && (!status.hook || status.permissions.accessibility !== 'granted');

  return (
    <div className="app">
      <aside className="sidebar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region>
          <div className="logo" />
          <span>anyTools</span>
        </div>
        <nav>
          {PAGES.map((p) => (
            <button key={p.id} className={page === p.id ? 'active' : ''} onClick={() => setPage(p.id)}>
              <Icon name={p.icon} size={16} />
              <span>{p.label}</span>
              {p.id === 'general' && needsSetup && <i className="dot warn" />}
              {p.id === 'config' && status?.config_error && <i className="dot bad" />}
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">{status ? <StateBadge status={status} /> : null}</div>
      </aside>
      <main className="content">
        <div className="titlebar" data-tauri-drag-region />
        {!status ? (
          <div className="loading">加载中…</div>
        ) : page === 'general' ? (
          <General status={status} overview={overview} />
        ) : page === 'wheels' ? (
          <Wheels overview={overview} icons={icons} />
        ) : page === 'config' ? (
          <Config status={status} />
        ) : (
          <About status={status} />
        )}
      </main>
    </div>
  );
}

function StateBadge({ status }: { status: Status }) {
  const [cls, text] = !status.hook
    ? ['warn', '快捷键未启用']
    : status.secure_input
      ? ['warn', '密码输入中，已暂停']
      : status.config_error
        ? ['bad', '配置有误']
        : ['ok', '运行中'];
  return (
    <div className={`badge ${cls}`}>
      <i className={`dot ${cls}`} />
      {text}
    </div>
  );
}

// ------------------------------------------------------------------------------------------------

const PERM_TEXT: Record<PermState, string> = {
  granted: '已授权',
  denied: '未授权',
  not_determined: '尚未询问',
  not_needed: '无需授权',
  unknown: '未知',
};

function PermRow(props: { name: 'accessibility' | 'automation_finder'; state: PermState; title: string; desc: string; required?: boolean }) {
  const { name, state, title, desc, required } = props;
  const ok = state === 'granted' || state === 'not_needed';
  return (
    <div className="perm">
      <div className={`perm-icon ${ok ? 'ok' : required ? 'warn' : 'muted'}`}>
        <Icon name={ok ? 'CircleCheck' : 'CircleAlert'} size={20} />
      </div>
      <div className="perm-text">
        <div className="perm-title">
          {title}
          {required && <span className="tag">必需</span>}
        </div>
        <div className="perm-desc">{desc}</div>
      </div>
      <div className="perm-state">{PERM_TEXT[state]}</div>
      {!ok && (
        <div className="perm-actions">
          {state === 'not_determined' || name === 'accessibility' ? (
            <button className="primary" onClick={() => invoke('request_permission', { name })}>
              授权
            </button>
          ) : null}
          <button onClick={() => invoke('open_permission_settings', { name })}>打开系统设置</button>
        </div>
      )}
    </div>
  );
}

function chordText(k: { mods: number; key: string }, mac: boolean): string {
  const parts: string[] = [];
  if (k.mods & 2) parts.push(mac ? '⌃' : 'Ctrl');
  if (k.mods & 4) parts.push(mac ? '⌥' : 'Alt');
  if (k.mods & 8) parts.push(mac ? '⇧' : 'Shift');
  if (k.mods & 1) parts.push(mac ? '⌘' : 'Win');
  const key = k.key.length === 1 ? k.key.toUpperCase() : k.key.charAt(0).toUpperCase() + k.key.slice(1);
  parts.push(key);
  return parts.join(mac ? ' ' : ' + ');
}

const BUTTON_TEXT: Record<string, string> = { Middle: '鼠标中键', Back: '鼠标后退键', Forward: '鼠标前进键' };

function triggerText(t: Overview['triggers'][number], mac: boolean): string {
  if ('keys' in t.input && t.input.keys) return chordText(t.input.keys, mac);
  if ('button' in t.input && t.input.button) return BUTTON_TEXT[t.input.button] ?? String(t.input.button);
  return '?';
}

function General({ status, overview }: { status: Status; overview: Overview | null }) {
  const mac = status.platform === 'mac';
  const p = status.permissions;
  const trigger = overview?.triggers[0];
  return (
    <div className="page">
      <h1>通用</h1>

      {status.hook && trigger ? (
        <section className="hero">
          <div className="hero-keys">
            {triggerText(trigger, mac)
              .split(/\s\+\s|\s/)
              .map((k, i) => (
                <kbd key={i}>{k}</kbd>
              ))}
          </div>
          <div>
            <div className="hero-title">按住唤起快捷轮盘</div>
            <div className="hero-sub">
              移动到一格上松开即执行；{trigger.tap === 'click' ? '短按则轮盘保持打开，点击选择；' : ''}Esc 或右键取消。
            </div>
          </div>
        </section>
      ) : null}

      {status.secure_input && (
        <div className="notice warn">
          <Icon name="KeyRound" size={16} />
          某个应用正在输入密码，系统暂时对所有应用隐藏了按键，快捷键会在离开密码框后恢复。
        </div>
      )}

      {mac && (
        <section>
          <h2>权限</h2>
          <div className="card">
            <PermRow
              name="accessibility"
              state={p.accessibility}
              required
              title="辅助功能"
              desc="用于监听唤起键、向当前应用发送快捷键和文字。授权后立即生效，无需重启。"
            />
            <PermRow
              name="automation_finder"
              state={p.automation_finder}
              title="自动化（访达）"
              desc="用于读取访达中选中的文件和当前文件夹，以显示文件轮盘。第一次在访达中唤起时系统会询问。"
            />
          </div>
          {p.accessibility !== 'granted' && (
            <p className="hint">
              在「系统设置 › 隐私与安全性 › 辅助功能」中打开 anyTools 的开关。系统只会弹出一次授权框；如果列表里已有 anyTools 但开关无效，先用「−」移除再重新授权。
            </p>
          )}
        </section>
      )}

      <section>
        <h2>外观</h2>
        <div className="card rows">
          <Row label="主题" value={{ auto: '跟随系统', dark: '深色', light: '浅色' }[status.settings.theme]} />
          <Row label="强调色" value={status.settings.accent === 'system' ? '默认（紫→青渐变）' : status.settings.accent} />
          <Row label="毛玻璃材质" value={status.settings.material} />
        </div>
        <p className="hint">在配置文件的 settings 中修改；可视化编辑会在后续版本提供。</p>
      </section>
    </div>
  );
}

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="row">
      <span>{label}</span>
      <b>{value}</b>
    </div>
  );
}

// ------------------------------------------------------------------------------------------------

function Wheels({ overview, icons }: { overview: Overview | null; icons: Record<string, string> }) {
  const [sel, setSel] = useState<string | null>(null);
  const wheels = overview?.wheels ?? [];
  const current = useMemo(() => wheels.find((w) => w.key === sel) ?? wheels.find((w) => w.key === overview?.fallback) ?? wheels[0], [wheels, sel, overview]);
  if (!overview || !current) return <div className="loading">加载中…</div>;
  const usedBy = (key: string) => {
    const n = overview.profiles.filter((p) => p === key).length;
    const parts: string[] = [];
    if (key === overview.fallback) parts.push('默认');
    if (n) parts.push(`${n} 条规则`);
    return parts.join(' · ');
  };
  return (
    <div className="page wide">
      <h1>快捷轮盘</h1>
      <div className="wheels">
        <div className="wheel-list">
          {wheels.map((w) => (
            <button key={w.key} className={w.key === current.key ? 'active' : ''} onClick={() => setSel(w.key)}>
              <div className="wl-name">{w.name}</div>
              <div className="wl-meta">
                {w.items.length} 项{usedBy(w.key) ? ` · ${usedBy(w.key)}` : ''}
              </div>
            </button>
          ))}
        </div>
        <div className="wheel-detail">
          <WheelPreview wheel={current} geom={overview.geom} appIcons={icons} size={240} />
          <div className="items">
            {current.items.map((it, i) => (
              <div className="item" key={it.id}>
                <span className="item-n">{i + 1}</span>
                <Icon name={it.icon} size={18} appIcons={icons} />
                <span className="item-label">{it.label}</span>
                {it.submenu && <span className="tag">子轮盘</span>}
              </div>
            ))}
          </div>
        </div>
      </div>
      <p className="hint">
        顺序从 12 点钟方向开始顺时针排列。按应用或选中内容切换轮盘的规则、以及每一格的动作，在配置文件的 modules.wheel 中编辑。
      </p>
    </div>
  );
}

// ------------------------------------------------------------------------------------------------

function Config({ status }: { status: Status }) {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const reload = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await invoke('reload_config');
      setMsg('已重新加载');
    } catch {
      // The error itself is shown below via status.config_error.
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="page">
      <h1>配置文件</h1>
      <section>
        <div className="card">
          <div className="path">{status.config_path}</div>
          <div className="actions">
            <button className="primary" onClick={() => invoke('open_config')}>
              <Icon name="SquarePen" size={14} /> 编辑
            </button>
            <button onClick={() => invoke('reveal_config')}>
              <Icon name="FolderOpen" size={14} /> 在访达中显示
            </button>
            <button onClick={reload} disabled={busy}>
              <Icon name="RefreshCw" size={14} /> 重新加载
            </button>
            {msg && <span className="ok-text">{msg}</span>}
          </div>
        </div>
        {status.config_error ? (
          <div className="notice bad">
            <Icon name="CircleX" size={16} />
            <div>
              <b>配置文件有误，修改未生效</b>（仍在使用上一份正确的配置）
              <pre>{status.config_error}</pre>
            </div>
          </div>
        ) : (
          <p className="hint">保存后自动生效。写错时会在这里和通知中提示，并继续使用上一份正确的配置。</p>
        )}
      </section>
      <section>
        <h2>可用的动作类型</h2>
        <div className="chips">
          {status.actions.map((a) => (
            <code key={a}>{a}</code>
          ))}
          <code>submenu</code>
        </div>
      </section>
    </div>
  );
}

function About({ status }: { status: Status }) {
  return (
    <div className="page">
      <h1>关于</h1>
      <div className="about">
        <div className="logo big" />
        <div>
          <div className="about-name">anyTools</div>
          <div className="about-ver">版本 {status.version}</div>
        </div>
      </div>
      <div className="card rows">
        {status.modules.map((m) => (
          <Row key={m.id} label={m.name} value="已启用" />
        ))}
      </div>
      <p className="hint">以 GPL-3.0 协议开源。</p>
      <div className="actions">
        <button className="danger" onClick={() => invoke('quit')}>
          退出 anyTools
        </button>
      </div>
    </div>
  );
}
