import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';
import type { Geom, PermState, Status, WheelView } from '../shared/types';
import type { FieldEnv } from './fields';
import { ClipboardPage } from './ClipboardPage';
import { Icon } from './Icon';
import type { Platform, WheelModuleCfg } from './model';
import { RulesEditor, TriggersEditor } from './RulesEditor';
import { useDraft, type Draft } from './useDraft';
import { WheelEditor } from './WheelEditor';

type Page = 'general' | 'wheels' | 'rules' | 'clipboard' | 'config' | 'about';

interface Overview {
  geom: Geom;
  wheels: WheelView[];
  fallback: string;
  profiles: string[];
  triggers: { input: { keys?: { mods: number; key: string } } | { button?: string }; tap: string; wheel: string | null }[];
}

const PAGES: { id: Page; label: string; icon: string }[] = [
  { id: 'general', label: '通用', icon: 'Settings2' },
  { id: 'wheels', label: '轮盘', icon: 'Disc3' },
  { id: 'rules', label: '唤起与规则', icon: 'Filter' },
  { id: 'clipboard', label: '剪贴板', icon: 'ClipboardList' },
  { id: 'config', label: '配置与脚本', icon: 'FileCode2' },
  { id: 'about', label: '关于', icon: 'Info' },
];

export function App() {
  const [page, setPage] = useState<Page>('general');
  const [status, setStatus] = useState<Status | null>(null);
  const [overview, setOverview] = useState<Overview | null>(null);
  const [icons, setIcons] = useState<Record<string, string>>({});
  const [scripts, setScripts] = useState<string[]>([]);
  const draft = useDraft();

  const refresh = useCallback(async () => {
    const st = await invoke<Status>('status');
    setStatus(st);
    const ov = await invoke<Overview | null>('module_call', { module: 'wheel', msg: { type: 'overview' } });
    if (ov) setOverview(ov);
  }, []);

  const loadIcons = useCallback(() => {
    void invoke<Record<string, string> | null>('module_call', { module: 'wheel', msg: { type: 'icons' } }).then((i) => i && setIcons(i));
    void invoke<string[]>('list_scripts').then(setScripts);
  }, []);

  useEffect(() => {
    void refresh();
    loadIcons();
    const un = listen<{ type: string }>('core', (ev) => {
      void refresh();
      if (ev.payload.type === 'config_loaded') loadIcons();
    });
    // Permissions are granted in System Settings; poll while the window is open.
    const t = setInterval(() => void refresh(), 1500);
    return () => {
      clearInterval(t);
      void un.then((f) => f());
    };
  }, [refresh, loadIcons]);

  const needsSetup = status !== null && (!status.hook || status.permissions.accessibility !== 'granted');
  const platform: Platform = status?.platform === 'win' ? 'win' : 'mac';
  const wheelCfg = draft.cfg?.modules.wheel;
  const setWheel = (f: (c: WheelModuleCfg) => void) => draft.edit((c) => c.modules.wheel && f(c.modules.wheel));
  const env: FieldEnv = {
    platform,
    wheels: Object.entries(wheelCfg?.wheels ?? {}).map(([k, w]) => ({ key: k, name: w.name ?? k })),
    scripts,
  };
  const editing = page === 'wheels' || page === 'rules' || page === 'general' || page === 'clipboard';

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
              {(p.id === 'wheels' || p.id === 'rules' || p.id === 'clipboard') && draft.dirty && <i className="dot accent" title="有未保存的修改" />}
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
          <General status={status} overview={overview} draft={draft} />
        ) : page === 'wheels' ? (
          wheelCfg && overview ? (
            <div className="page wide">
              <h1>轮盘</h1>
              <WheelEditor cfg={wheelCfg} setCfg={setWheel} geom={overview.geom} icons={icons} env={env} />
            </div>
          ) : (
            <div className="loading">加载中…</div>
          )
        ) : page === 'rules' ? (
          wheelCfg ? (
            <div className="page">
              <h1>唤起与规则</h1>
              <TriggersEditor cfg={wheelCfg} setCfg={setWheel} platform={platform} />
              <RulesEditor cfg={wheelCfg} setCfg={setWheel} platform={platform} />
            </div>
          ) : (
            <div className="loading">加载中…</div>
          )
        ) : page === 'clipboard' ? (
          draft.cfg ? <ClipboardPage draft={draft} platform={platform} /> : <div className="loading">加载中…</div>
        ) : page === 'config' ? (
          <Config status={status} draft={draft} scripts={scripts} />
        ) : (
          <About status={status} />
        )}
        {editing && <SaveBar draft={draft} />}
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

function General({ status, overview, draft }: { status: Status; overview: Overview | null; draft: Draft }) {
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
        <h2>启动</h2>
        <div className="card rows">
          <Autostart />
        </div>
      </section>

      {draft.cfg && <Appearance draft={draft} mac={mac} liquid={status.liquid_glass} />}
    </div>
  );
}

const ACCENTS = [
  { value: 'system', label: '默认', css: 'conic-gradient(#8b6cff, #22d3ee, #f472b6, #8b6cff)' },
  { value: '#0a84ff', label: '蓝', css: '#0a84ff' },
  { value: '#30d158', label: '绿', css: '#30d158' },
  { value: '#ff9f0a', label: '橙', css: '#ff9f0a' },
  { value: '#ff375f', label: '粉', css: '#ff375f' },
  { value: '#bf5af2', label: '紫', css: '#bf5af2' },
];

// Named by how they look, not by the AppKit material they map to. `liquid` (regular) is the more
// transparent of the two; `liquid-clear` frosts the background harder, so it reads as milkier.
const MATERIALS = [
  { value: 'liquid', label: '液态玻璃 · 清透' },
  { value: 'liquid-clear', label: '液态玻璃 · 磨砂' },
  { value: 'hud', label: 'HUD · 深色半透明' },
  { value: 'popover', label: '浅色模糊' },
  { value: 'menu', label: '菜单同款' },
  { value: 'sidebar', label: '更透的模糊' },
  { value: 'fullscreen-ui', label: '压暗的模糊' },
  { value: 'under-window', label: '最淡的模糊' },
];

function Appearance({ draft, mac, liquid }: { draft: Draft; mac: boolean; liquid: boolean }) {
  const s = draft.cfg!.settings;
  const set = (f: (x: typeof s) => void) => draft.edit((c) => f(c.settings));
  const custom = s.accent !== 'system' && !ACCENTS.some((a) => a.value === s.accent);
  const term = typeof s.terminal === 'string' ? s.terminal : (s.terminal as { mac?: string; win?: string } | null | undefined)?.[mac ? 'mac' : 'win'] ?? '';
  return (
    <section>
      <h2>外观</h2>
      <div className="card rows">
        <div className="row">
          <span>主题</span>
          <div className="seg">
            {(['dark', 'light', 'auto'] as const).map((t) => (
              <button key={t} className={s.theme === t ? 'active' : ''} onClick={() => set((x) => void (x.theme = t))}>
                {{ dark: '深色', light: '浅色', auto: '跟随系统' }[t]}
              </button>
            ))}
          </div>
        </div>
        <div className="row">
          <span>强调色</span>
          <div className="swatches">
            {ACCENTS.map((a) => (
              <button key={a.value} title={a.label} className={'swatch' + (s.accent === a.value ? ' active' : '')} style={{ background: a.css }} onClick={() => set((x) => void (x.accent = a.value))} />
            ))}
            <label className={'swatch custom' + (custom ? ' active' : '')} title="自定义" style={custom ? { background: s.accent } : undefined}>
              <input type="color" value={custom ? s.accent : '#7c5cff'} onChange={(e) => set((x) => void (x.accent = e.target.value))} />
            </label>
          </div>
        </div>
        {mac && (
          <div className="row">
            <span>玻璃材质</span>
            <select value={s.material} onChange={(e) => set((x) => void (x.material = e.target.value))}>
              {MATERIALS.map((m) => (
                <option key={m.value} value={m.value}>
                  {m.label}
                  {m.value.startsWith('liquid') && !liquid ? '（需要 macOS 26，当前用深色半透明）' : ''}
                </option>
              ))}
            </select>
          </div>
        )}
        <div className="row">
          <span>「在此打开终端」使用</span>
          <select
            value={term}
            onChange={(e) =>
              set((x) => {
                const cur = typeof x.terminal === 'object' && x.terminal ? { ...x.terminal } : {};
                (cur as Record<string, string>)[mac ? 'mac' : 'win'] = e.target.value;
                x.terminal = cur;
              })
            }
          >
            {(mac
              ? [
                  ['com.apple.Terminal', '终端'],
                  ['com.googlecode.iterm2', 'iTerm2'],
                  ['dev.warp.Warp-Stable', 'Warp'],
                  ['com.mitchellh.ghostty', 'Ghostty'],
                  ['net.kovidgoyal.kitty', 'kitty'],
                ]
              : [
                  ['wt.exe', 'Windows Terminal'],
                  ['pwsh.exe', 'PowerShell'],
                  ['cmd.exe', '命令提示符'],
                ]
            )
              .concat(term && ![...['com.apple.Terminal', 'com.googlecode.iterm2', 'dev.warp.Warp-Stable', 'com.mitchellh.ghostty', 'net.kovidgoyal.kitty', 'wt.exe', 'pwsh.exe', 'cmd.exe']].includes(term) ? [[term, term]] : [])
              .map(([v, l]) => (
                <option key={v} value={v}>
                  {l}
                </option>
              ))}
          </select>
        </div>
      </div>
    </section>
  );
}

function Autostart() {
  const [on, setOn] = useState<boolean | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    void invoke<boolean>('autostart_get').then(setOn, (e) => setErr(String(e)));
  }, []);
  return (
    <div className="row">
      <span>登录时自动启动</span>
      {err ? (
        <span className="field-err">{err}</span>
      ) : (
        <label className="switch">
          <input
            type="checkbox"
            checked={!!on}
            disabled={on === null}
            onChange={async (e) => {
              const want = e.target.checked;
              try {
                setOn(await invoke<boolean>('autostart_set', { enabled: want }));
                setErr(null);
              } catch (x) {
                setErr(String(x));
              }
            }}
          />
          <span />
        </label>
      )}
    </div>
  );
}

function SaveBar({ draft }: { draft: Draft }) {
  if (!draft.dirty && !draft.error) return null;
  return (
    <div className={'savebar' + (draft.error ? ' bad' : '')}>
      <div className="savebar-text">
        {draft.error ? (
          <>
            <Icon name="CircleX" size={15} />
            <span title={draft.error}>{draft.error}</span>
          </>
        ) : draft.validating ? (
          <span>检查中…</span>
        ) : (
          <>
            <Icon name="CircleDot" size={15} />
            <span>有未保存的修改</span>
          </>
        )}
      </div>
      <button onClick={draft.discard} disabled={draft.saving}>
        <Icon name="Undo2" size={13} /> 放弃
      </button>
      <button className="primary" onClick={() => void draft.save()} disabled={!!draft.error || draft.validating || draft.saving}>
        <Icon name="Check" size={13} /> {draft.saving ? '保存中…' : '保存并生效'}
      </button>
    </div>
  );
}

interface UpdateInfo {
  configured: boolean;
  available: boolean;
  version?: string;
  notes?: string;
}

function Updates({ version }: { version: string }) {
  const [state, setState] = useState<'idle' | 'checking' | 'installing'>('idle');
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const check = async () => {
    setState('checking');
    setErr(null);
    try {
      setInfo(await invoke<UpdateInfo>('update_check'));
    } catch (e) {
      setErr(String(e));
    } finally {
      setState('idle');
    }
  };
  const install = async () => {
    setState('installing');
    setErr(null);
    try {
      await invoke('update_install');
    } catch (e) {
      setErr(String(e));
      setState('idle');
    }
  };
  return (
    <>
      <div className="row">
        <span>当前版本</span>
        <b>{version}</b>
      </div>
      <div className="row">
        <span>
          {info?.available ? (
            <>
              可更新到 <b>{info.version}</b>
            </>
          ) : info && !info.configured ? (
            '此版本未配置更新服务'
          ) : info ? (
            '已是最新版本'
          ) : (
            '检查更新'
          )}
        </span>
        {info?.available ? (
          <button className="primary" onClick={install} disabled={state !== 'idle'}>
            <Icon name="Download" size={13} /> {state === 'installing' ? '下载安装中…' : '下载并重启'}
          </button>
        ) : (
          <button onClick={check} disabled={state !== 'idle'}>
            <Icon name="RefreshCw" size={13} /> {state === 'checking' ? '检查中…' : '检查'}
          </button>
        )}
      </div>
      {info?.notes && <pre className="notes">{info.notes}</pre>}
      {err && <div className="field-err" style={{ paddingBottom: 10 }}>{err}</div>}
    </>
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

function Config({ status, draft, scripts }: { status: Status; draft: Draft; scripts: string[] }) {
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const reload = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await invoke('reload_config');
      await draft.reset();
      setMsg('已重新加载');
    } catch {
      // The error itself is shown below via status.config_error.
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="page">
      <h1>配置与脚本</h1>
      {draft.dirty && (
        <div className="notice warn">
          <Icon name="CircleDot" size={14} />
          设置窗口里有未保存的修改。重新加载文件会放弃这些修改。
        </div>
      )}
      <section>
        <h2>配置文件</h2>
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
          <p className="hint">
            手动编辑保存后自动生效；写错时会在这里和通知中提示，并继续使用上一份正确的配置。在设置窗口里保存会重写整个文件：手写的注释不会保留，上一版备份在同目录的 config.yaml.bak。
          </p>
        )}
      </section>
      <section>
        <h2>脚本</h2>
        <div className="card">
          <div className="items">
            {scripts.length === 0 && <p className="hint">scripts 目录里还没有脚本。</p>}
            {scripts.map((s) => (
              <div className="item" key={s}>
                <Icon name="FileCode2" size={16} />
                <span className="item-label">{s}</span>
              </div>
            ))}
          </div>
          <div className="actions">
            <button onClick={() => invoke('reveal_scripts')}>
              <Icon name="FolderOpen" size={14} /> 打开 scripts 目录
            </button>
          </div>
        </div>
        <p className="hint">
          用 JavaScript 或 TypeScript 编写，在轮盘的一格里选「JS / TS 脚本」调用。目录里的 anytools.d.ts 提供编辑器补全，example.ts 是示例。脚本需要的权限（文件、命令、网络）写在开头的 // @permissions 行，第一次运行和每次修改后会请你确认。
        </p>
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
      <section>
        <h2>更新</h2>
        <div className="card rows">
          <Updates version={status.version} />
        </div>
      </section>
      <section>
        <h2>模块</h2>
        <div className="card rows">
          {status.modules.map((m) => (
            <Row key={m.id} label={m.name} value="已启用" />
          ))}
        </div>
      </section>
      <section>
        <h2>诊断</h2>
        <div className="card">
          <div className="actions" style={{ paddingTop: 12 }}>
            <button onClick={() => invoke('reveal_logs')}>
              <Icon name="FolderOpen" size={14} /> 打开日志目录
            </button>
          </div>
        </div>
        <p className="hint">日志与崩溃记录都在这里，只保存在本机。报告问题时可以附上。</p>
      </section>
      <p className="hint">以 GPL-3.0 协议开源。</p>
      <div className="actions">
        <button className="danger" onClick={() => invoke('quit')}>
          退出 anyTools
        </button>
      </div>
    </div>
  );
}
