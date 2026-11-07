import { invoke } from '@tauri-apps/api/core';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Icon } from './Icon';
import type { FieldDef, Platform, PP } from './model';
import { forPlatform, isSplit, setForPlatform } from './model';

// ------------------------------------------------------------------------------------------------
// Chord recorder

const KEY_NAMES: Record<string, string> = {
  ' ': 'space',
  Enter: 'enter',
  Tab: 'tab',
  Escape: 'esc',
  Backspace: 'delete',
  Delete: 'forwarddelete',
  ArrowUp: 'up',
  ArrowDown: 'down',
  ArrowLeft: 'left',
  ArrowRight: 'right',
  Home: 'home',
  End: 'end',
  PageUp: 'pageup',
  PageDown: 'pagedown',
  CapsLock: 'capslock',
};

/** Key name from a KeyboardEvent, using the physical key so ⌥ combos don't produce symbols. */
function keyName(e: KeyboardEvent): string | null {
  if (['Meta', 'Control', 'Alt', 'Shift'].includes(e.key)) return null;
  if (KEY_NAMES[e.key]) return KEY_NAMES[e.key];
  const c = e.code;
  if (/^Key[A-Z]$/.test(c)) return c.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(c)) return c.slice(5);
  if (/^F([1-9]|1[0-9]|20)$/.test(c)) return c.toLowerCase();
  const punct: Record<string, string> = {
    Minus: '-',
    Equal: '=',
    BracketLeft: '[',
    BracketRight: ']',
    Backslash: '\\',
    Semicolon: ';',
    Quote: "'",
    Comma: ',',
    Period: '.',
    Slash: '/',
    Backquote: '`',
    Space: 'space',
  };
  return punct[c] ?? null;
}

export function chordFromEvent(e: KeyboardEvent, p: Platform): string | null {
  const key = keyName(e);
  if (!key) return null;
  const mods: string[] = [];
  if (p === 'mac') {
    if (e.ctrlKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');
    if (e.metaKey) mods.push('cmd');
  } else {
    if (e.ctrlKey) mods.push('ctrl');
    if (e.altKey) mods.push('alt');
    if (e.shiftKey) mods.push('shift');
    if (e.metaKey) mods.push('win');
  }
  return [...mods, key].join('+');
}

const MAC_SYM: Record<string, string> = { cmd: '⌘', mod: '⌘', ctrl: '⌃', alt: '⌥', opt: '⌥', shift: '⇧' };

export function prettyChord(chord: string, p: Platform): string[] {
  return chord.split('+').map((part) => {
    const k = part.trim().toLowerCase();
    if (p === 'mac' && MAC_SYM[k]) return MAC_SYM[k];
    if (k === 'mod') return p === 'mac' ? '⌘' : 'Ctrl';
    if (k === 'space') return 'Space';
    if (k.length === 1) return k.toUpperCase();
    return k.charAt(0).toUpperCase() + k.slice(1);
  });
}

export function ChordInput({ value, onChange, platform, allowModifierOnly = false }: { value: string; onChange: (v: string) => void; platform: Platform; allowModifierOnly?: boolean }) {
  const [rec, setRec] = useState(false);
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!rec) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
        setRec(false);
        return;
      }
      const c = chordFromEvent(e, platform);
      if (c) {
        onChange(c);
        setRec(false);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [rec, platform, onChange]);
  void allowModifierOnly;
  return (
    <div className="chord">
      <button ref={ref} type="button" className={'chord-btn' + (rec ? ' recording' : '')} onClick={() => setRec((r) => !r)} onBlur={() => setRec(false)}>
        {rec ? (
          <span className="chord-hint">按下快捷键…（Esc 取消）</span>
        ) : value ? (
          prettyChord(value, platform).map((k, i) => <kbd key={i}>{k}</kbd>)
        ) : (
          <span className="chord-hint">点击录制</span>
        )}
      </button>
      <input className="chord-text" value={value} placeholder="或直接输入，如 mod+shift+p" onChange={(e) => onChange(e.target.value.trim())} spellCheck={false} />
    </div>
  );
}

// ------------------------------------------------------------------------------------------------
// App picker

interface AppInfo {
  name: string;
  id: string | null;
  path: string;
}

let appsCache: Promise<AppInfo[]> | null = null;
const loadApps = () => (appsCache ??= invoke<AppInfo[]>('list_apps').catch(() => []));

export function AppInput({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [apps, setApps] = useState<AppInfo[]>([]);
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState('');
  useEffect(() => {
    if (open && !apps.length) void loadApps().then(setApps);
  }, [open, apps.length]);
  const shown = useMemo(() => {
    const s = q.trim().toLowerCase();
    return (s ? apps.filter((a) => a.name.toLowerCase().includes(s) || a.id?.toLowerCase().includes(s)) : apps).slice(0, 60);
  }, [apps, q]);
  const current = apps.find((a) => a.id === value || a.name === value || a.path === value);
  return (
    <div className="app-input">
      <div className="app-row">
        <input value={value} onChange={(e) => onChange(e.target.value)} placeholder="bundle id、应用名或路径" spellCheck={false} />
        <button type="button" onClick={() => setOpen((o) => !o)}>
          <Icon name="Search" size={13} /> 选择
        </button>
      </div>
      {current && <div className="field-hint">{current.name}</div>}
      {open && (
        <div className="picker">
          <input autoFocus placeholder="搜索应用" value={q} onChange={(e) => setQ(e.target.value)} />
          <div className="picker-list">
            {shown.map((a) => (
              <button
                type="button"
                key={a.path}
                onClick={() => {
                  onChange(a.id ?? a.name);
                  setOpen(false);
                  setQ('');
                }}
              >
                <span>{a.name}</span>
                <small>{a.id ?? ''}</small>
              </button>
            ))}
            {!shown.length && <div className="field-hint">{apps.length ? '没有匹配的应用' : '加载中…'}</div>}
          </div>
        </div>
      )}
    </div>
  );
}

// ------------------------------------------------------------------------------------------------
// Generic field with optional per-platform values

export interface FieldEnv {
  platform: Platform;
  wheels: { key: string; name: string }[];
  scripts: string[];
}

function SingleInput({ def, value, onChange, env }: { def: FieldDef; value: unknown; onChange: (v: unknown) => void; env: FieldEnv }) {
  const str = value === undefined || value === null ? '' : String(value);
  switch (def.kind) {
    case 'chord':
      return <ChordInput value={str} onChange={onChange} platform={env.platform} />;
    case 'app':
      return <AppInput value={str} onChange={(v) => onChange(v || undefined)} />;
    case 'bool':
      return (
        <label className="switch">
          <input type="checkbox" checked={value === true} onChange={(e) => onChange(e.target.checked)} />
          <span />
        </label>
      );
    case 'number':
      return <input type="number" value={str} placeholder={def.placeholder} onChange={(e) => onChange(e.target.value === '' ? undefined : Number(e.target.value))} />;
    case 'longtext':
      return <textarea rows={3} value={str} placeholder={def.placeholder} onChange={(e) => onChange(e.target.value)} />;
    case 'code':
      return <textarea className="code" rows={4} value={str} placeholder={def.placeholder} onChange={(e) => onChange(e.target.value)} spellCheck={false} />;
    case 'wheel':
      return (
        <select value={str} onChange={(e) => onChange(e.target.value)}>
          <option value="">选择轮盘…</option>
          {env.wheels.map((w) => (
            <option key={w.key} value={w.key}>
              {w.name}
            </option>
          ))}
        </select>
      );
    case 'script':
      return (
        <select value={str} onChange={(e) => onChange(e.target.value)}>
          <option value="">选择 scripts/ 中的脚本…</option>
          {[...new Set([...env.scripts, ...(str ? [str] : [])])].map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
      );
    case 'select':
      return (
        <select value={str} onChange={(e) => onChange(e.target.value || undefined)}>
          {def.options?.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      );
    default:
      return <input value={str} placeholder={def.placeholder} onChange={(e) => onChange(e.target.value)} spellCheck={false} />;
  }
}

export function Field({ def, value, onChange, env }: { def: FieldDef; value: unknown; onChange: (v: unknown) => void; env: FieldEnv }) {
  const split = def.perPlatform && isSplit(value as PP<unknown>);
  const p = env.platform;
  const other: Platform = p === 'mac' ? 'win' : 'mac';
  const otherName = other === 'mac' ? 'macOS' : 'Windows';
  return (
    <div className="field">
      <div className="field-label">
        <span>
          {def.label}
          {def.required && <i className="req">*</i>}
        </span>
        {def.perPlatform && (
          <label className="split-toggle" title="两个平台用不同的值">
            <input
              type="checkbox"
              checked={!!split}
              onChange={(e) => {
                const cur = forPlatform(value as PP<unknown>, p);
                onChange(e.target.checked ? setForPlatform(cur, p, cur, true) : cur);
              }}
            />
            按平台分别设置
          </label>
        )}
      </div>
      {split ? (
        <div className="split">
          {(['mac', 'win'] as Platform[]).map((pl) => (
            <div key={pl} className="split-row">
              <span className="split-name">{pl === 'mac' ? 'macOS' : 'Windows'}</span>
              <SingleInput def={def} value={forPlatform(value as PP<unknown>, pl)} env={{ ...env, platform: pl }} onChange={(v) => onChange(setForPlatform(value as PP<unknown>, pl, v, true))} />
            </div>
          ))}
        </div>
      ) : (
        <SingleInput def={def} value={forPlatform(value as PP<unknown>, p)} env={env} onChange={onChange} />
      )}
      {def.hint && <div className="field-hint">{def.hint}</div>}
      {def.perPlatform && !split && <div className="field-hint">同一个值用于 macOS 和 Windows。勾选上方选项可为 {otherName} 单独设置。</div>}
    </div>
  );
}
