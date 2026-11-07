import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import { ChordInput } from './fields';
import { Icon } from './Icon';
import { forPlatform, isSplit, setForPlatform, type ClipboardCfg, type Platform } from './model';
import type { Draft } from './useDraft';

// Same as the Rust defaults for a config without a `clipboard` section.
const DEFAULTS: ClipboardCfg = { enabled: true, trigger: { mac: 'ctrl+cmd+v', win: 'ctrl+alt+v' }, max: 200, recent: 8, ignore_apps: [] };

export function ClipboardPage({ draft, platform }: { draft: Draft; platform: Platform }) {
  const c: ClipboardCfg = { ...DEFAULTS, ...(draft.cfg?.modules.clipboard ?? {}) };
  const set = (f: (x: ClipboardCfg) => void) =>
    draft.edit((cfg) => {
      const cur: ClipboardCfg = { ...DEFAULTS, ...(cfg.modules.clipboard ?? {}) };
      f(cur);
      cfg.modules.clipboard = cur;
    });
  const trigger = forPlatform(c.trigger ?? undefined, platform) ?? '';
  const [count, setCount] = useState<number | null>(null);
  const refresh = () => void invoke<{ count: number } | null>('module_call', { module: 'clipboard', msg: { type: 'stats' } }).then((s) => setCount(s?.count ?? null));
  useEffect(refresh, []);
  const mac = platform === 'mac';

  return (
    <div className="page">
      <h1>剪贴板</h1>
      <section>
        <div className="card rows">
          <div className="row">
            <span>记录复制的文字</span>
            <label className="switch">
              <input type="checkbox" checked={c.enabled} onChange={(e) => set((x) => void (x.enabled = e.target.checked))} />
              <span />
            </label>
          </div>
          <div className="row">
            <span>唤起剪贴板面板</span>
            <ChordInput value={trigger} platform={platform} onChange={(v) => set((x) => void (x.trigger = setForPlatform(x.trigger ?? undefined, platform, v || undefined, isSplit(x.trigger ?? undefined)) ?? null))} />
          </div>
          <div className="row">
            <span>最多保留</span>
            <select value={c.max} onChange={(e) => set((x) => void (x.max = Number(e.target.value)))}>
              {[50, 100, 200, 500, 1000, 2000].concat([50, 100, 200, 500, 1000, 2000].includes(c.max) ? [] : [c.max]).map((n) => (
                <option key={n} value={n}>
                  {n} 条
                </option>
              ))}
            </select>
          </div>
          <div className="row">
            <span>轮盘里「剪贴板」显示</span>
            <select value={c.recent} onChange={(e) => set((x) => void (x.recent = Number(e.target.value)))}>
              {Array.from({ length: 11 }, (_, i) => i + 2).map((n) => (
                <option key={n} value={n}>
                  最近 {n} 条
                </option>
              ))}
            </select>
          </div>
          <div className="row">
            <span>不记录这些应用</span>
            <input
              className="wide-input"
              placeholder={mac ? '如 com.apple.keychainaccess' : '如 KeePass.exe'}
              value={c.ignore_apps.join(', ')}
              onChange={(e) => set((x) => void (x.ignore_apps = e.target.value.split(/[,\s]+/).filter(Boolean)))}
            />
          </div>
        </div>
        <p className="hint">
          面板里直接打字即可搜索；↑↓ 选择，回车粘贴到当前应用，⇧回车只复制，{mac ? '⌘' : 'Ctrl+'}1–9 直接粘贴对应行，{mac ? '⌘⌫' : 'Ctrl+Backspace'} 删除一条。也可以在任何轮盘里放一格「最近剪贴板」，划出展开最近几条。
        </p>
      </section>

      <section>
        <h2>隐私</h2>
        <div className="card rows">
          <div className="row">
            <span>已保存 {count === null ? '…' : `${count} 条`}，只存在本机</span>
            <button
              className="danger"
              disabled={!count}
              onClick={async () => {
                await invoke('module_call', { module: 'clipboard', msg: { type: 'clear' } });
                refresh();
              }}
            >
              <Icon name="Trash2" size={13} /> 清空历史
            </button>
          </div>
        </div>
        <p className="hint">只记录文字。密码管理器等应用标记为敏感的内容、anyTools 自己读取选中文字时临时借用的剪贴板都不会记录。历史文件在配置目录的 state/clipboard.json，仅当前用户可读。</p>
      </section>
    </div>
  );
}
