import { useMemo, useState } from 'react';
import { hasLucide } from '../shared/icons';
import type { Geom } from '../shared/types';
import { Field, type FieldEnv } from './fields';
import { Icon } from './Icon';
import {
  ACTIONS,
  actionDef,
  actionSummary,
  clone,
  defaultAction,
  effectiveItems,
  nextId,
  wheelKey,
  type ItemCfg,
  type WheelModuleCfg,
} from './model';
import { WheelPreview } from './WheelPreview';

const MAX = 12;
const MIN = 2;

const ICON_SUGGESTIONS = [
  'Copy', 'ClipboardPaste', 'Scissors', 'Crop', 'Camera', 'Search', 'Globe', 'Mail', 'MessageSquare', 'Calendar',
  'CalendarDays', 'Clock', 'Bell', 'Lock', 'Undo2', 'Redo2', 'Save', 'Download', 'Upload', 'Share2', 'Link', 'Bookmark',
  'Star', 'Heart', 'Music', 'Play', 'Pause', 'Volume2', 'Mic', 'Sun', 'Moon', 'Settings', 'Wrench', 'Terminal',
  'SquareTerminal', 'Code', 'GitBranch', 'Bug', 'Folder', 'FolderPlus', 'File', 'FileText', 'FileArchive', 'Trash2',
  'Eye', 'Type', 'Keyboard', 'Command', 'Calculator', 'Palette', 'Image', 'Video', 'Monitor', 'Zap', 'Rocket',
  'Sparkles', 'Wand2', 'Languages', 'Plus', 'X', 'RefreshCw', 'ArrowLeft', 'ArrowRight', 'LayoutGrid', 'AppWindow',
];

interface Props {
  cfg: WheelModuleCfg;
  setCfg: (f: (c: WheelModuleCfg) => void) => void;
  geom: Geom;
  icons: Record<string, string>;
  env: FieldEnv;
}

export function WheelEditor({ cfg, setCfg, geom, icons, env }: Props) {
  const keys = Object.keys(cfg.wheels);
  const [sel, setSel] = useState<string>(keys.includes('default') ? 'default' : keys[0]);
  const [item, setItem] = useState<number | null>(null);
  const key = keys.includes(sel) ? sel : keys[0];
  const w = cfg.wheels[key];
  const items = useMemo(() => effectiveItems(cfg, key), [cfg, key]);
  const inherited = !!w?.extends;

  /** Edits the item list. Wheels that inherit are converted to their own copy first (simplest model). */
  const editItems = (f: (items: ItemCfg[]) => ItemCfg[]) =>
    setCfg((c) => {
      const cur = effectiveItems(c, key);
      const wc = c.wheels[key];
      wc.items = f(clone(cur));
      delete wc.extends;
      delete wc.add;
      delete wc.remove;
    });

  const usedBy = (k: string) => {
    const n = cfg.profiles.filter((p) => p.wheel === k).length;
    const subs = Object.entries(cfg.wheels).filter(([wk]) => effectiveItems(cfg, wk).some((it) => it.action.type === 'submenu' && it.action.wheel === k)).length;
    return [k === 'default' ? '默认' : '', n ? `${n} 条规则` : '', subs ? `${subs} 个子入口` : ''].filter(Boolean).join(' · ');
  };

  if (!w) return null;
  const preview = { key, name: w.name ?? key, items: items.map((it, i) => ({ id: it.id ?? `#${i}`, label: it.label, icon: iconOf(it), submenu: it.action.type === 'submenu' })) };

  return (
    <div className="editor">
      <div className="wheel-list">
        {keys.map((k) => (
          <button key={k} className={k === key ? 'active' : ''} onClick={() => (setSel(k), setItem(null))}>
            <div className="wl-name">{cfg.wheels[k].name ?? k}</div>
            <div className="wl-meta">
              {effectiveItems(cfg, k).length} 项{usedBy(k) ? ` · ${usedBy(k)}` : ''}
            </div>
          </button>
        ))}
        <button
          className="add"
          onClick={() => {
            const k = wheelKey(keys);
            setCfg((c) => {
              c.wheels[k] = {
                name: '新轮盘',
                items: [
                  { id: 'item1', label: '复制', icon: 'Copy', action: { type: 'keystroke', keys: 'mod+c' } },
                  { id: 'item2', label: '粘贴', icon: 'ClipboardPaste', action: { type: 'keystroke', keys: 'mod+v' } },
                ],
              };
            });
            setSel(k);
            setItem(null);
          }}
        >
          <Icon name="Plus" size={14} /> 新建轮盘
        </button>
      </div>

      <div className="wheel-pane">
        <div className="wheel-head">
          <input className="title-input" value={w.name ?? ''} placeholder={key} onChange={(e) => setCfg((c) => void (c.wheels[key].name = e.target.value || undefined))} />
          <span className="key-tag" title="在配置文件中引用这个轮盘用的名字">{key}</span>
          <div className="spacer" />
          <button
            className="danger"
            disabled={key === 'default' || !!usedBy(key)}
            title={key === 'default' ? '默认轮盘不能删除' : usedBy(key) ? '先移除引用它的规则或子入口' : ''}
            onClick={() => {
              setCfg((c) => void delete c.wheels[key]);
              setSel('default');
              setItem(null);
            }}
          >
            <Icon name="Trash2" size={13} /> 删除轮盘
          </button>
        </div>
        {inherited && (
          <div className="notice info">
            <Icon name="Layers" size={14} />
            这个轮盘继承自「{cfg.wheels[w.extends!]?.name ?? w.extends}」。在这里修改会把它变成独立的一份，之后不再跟随「{cfg.wheels[w.extends!]?.name ?? w.extends}」的变化。
          </div>
        )}

        <div className="wheel-body">
          <div className="preview-col">
            <WheelPreview wheel={preview} geom={geom} appIcons={icons} size={250} selected={item} onSelect={setItem} />
            <div className="preview-actions">
              <button disabled={items.length >= MAX} onClick={() => (editItems((its) => [...its, newItem(its)]), setItem(items.length))}>
                <Icon name="Plus" size={13} /> 添加一格
              </button>
              <span className="field-hint">
                {items.length} / {MAX} 格
              </span>
            </div>
          </div>

          <div className="items-col">
            {item === null || !items[item] ? (
              <div className="items">
                {items.map((it, i) => (
                  <div className="item row" key={(it.id ?? '') + i} onClick={() => setItem(i)}>
                    <span className="item-n">{i + 1}</span>
                    <Icon name={iconOf(it)} size={18} appIcons={icons} />
                    <div className="item-text">
                      <span className="item-label">{it.label}</span>
                      <small>{actionSummary(it.action, env.platform)}</small>
                    </div>
                    <div className="row-tools" onClick={(e) => e.stopPropagation()}>
                      <button className="icon-btn" title="上移" disabled={i === 0} onClick={() => editItems((its) => move(its, i, i - 1))}>
                        <Icon name="ArrowUp" size={13} />
                      </button>
                      <button className="icon-btn" title="下移" disabled={i === items.length - 1} onClick={() => editItems((its) => move(its, i, i + 1))}>
                        <Icon name="ArrowDown" size={13} />
                      </button>
                    </div>
                  </div>
                ))}
                <p className="hint">顺序从 12 点钟方向开始顺时针排列。点一行或点预览中的一格进行编辑。</p>
              </div>
            ) : (
              <ItemEditor
                key={`${key}:${item}`}
                item={items[item]}
                index={item}
                count={items.length}
                env={env}
                icons={icons}
                onBack={() => setItem(null)}
                onChange={(next) => editItems((its) => its.map((x, j) => (j === item ? next : x)))}
                onDelete={() => {
                  editItems((its) => its.filter((_, j) => j !== item));
                  setItem(null);
                }}
                onDuplicate={() => {
                  editItems((its) => [...its.slice(0, item + 1), { ...clone(its[item]), id: nextId(its) }, ...its.slice(item + 1)]);
                  setItem(item + 1);
                }}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function move<T>(arr: T[], from: number, to: number): T[] {
  const a = [...arr];
  const [x] = a.splice(from, 1);
  a.splice(to, 0, x);
  return a;
}

function newItem(items: ItemCfg[]): ItemCfg {
  return { id: nextId(items), label: '新的一格', icon: 'Sparkles', action: { type: 'keystroke', keys: '' } };
}

/** The icon the overlay would show (mirrors the Rust defaults in crates/modules/wheel/src/config.rs). */
export function iconOf(it: ItemCfg): string {
  const i = it.icon?.trim();
  if (i && i !== 'app:auto') return i.includes(':') ? i : `lucide:${i}`;
  const a = it.action;
  const target = typeof a.target === 'string' ? a.target : (a.target as { mac?: string } | undefined)?.mac;
  if (a.type === 'launch' && target) return `app:${target}`;
  const def = actionDef(a.type);
  return `lucide:${def?.icon ?? 'Circle'}`;
}

function ItemEditor(props: {
  item: ItemCfg;
  index: number;
  count: number;
  env: FieldEnv;
  icons: Record<string, string>;
  onChange: (i: ItemCfg) => void;
  onBack: () => void;
  onDelete: () => void;
  onDuplicate: () => void;
}) {
  const { item, env, icons, onChange } = props;
  const def = actionDef(item.action.type);
  const [iconQ, setIconQ] = useState('');
  const iconName = item.icon?.startsWith('lucide:') ? item.icon.slice(7) : item.icon ?? '';
  const iconOk = !iconName || iconName.startsWith('app:') || iconName.startsWith('file:') || hasLucide(iconName);
  const set = (patch: Partial<ItemCfg>) => onChange({ ...item, ...patch });
  const setAction = (k: string, v: unknown) => {
    const a = { ...item.action };
    if (v === undefined || v === '') delete a[k];
    else a[k] = v;
    onChange({ ...item, action: a });
  };
  const groups = [...new Set(ACTIONS.map((a) => a.group))];
  return (
    <div className="item-editor">
      <div className="ie-head">
        <button className="icon-btn" onClick={props.onBack} title="返回列表">
          <Icon name="ArrowLeft" size={15} />
        </button>
        <b>第 {props.index + 1} 格</b>
        <div className="spacer" />
        <button onClick={props.onDuplicate} disabled={props.count >= MAX}>
          <Icon name="Copy" size={13} /> 复制
        </button>
        <button className="danger" onClick={props.onDelete} disabled={props.count <= MIN} title={props.count <= MIN ? `至少保留 ${MIN} 格` : ''}>
          <Icon name="Trash2" size={13} /> 删除
        </button>
      </div>

      <div className="field">
        <div className="field-label">
          <span>
            名称<i className="req">*</i>
          </span>
        </div>
        <input value={item.label} onChange={(e) => set({ label: e.target.value })} />
      </div>

      <div className="field">
        <div className="field-label">
          <span>图标</span>
          <span className="field-hint">lucide 图标名，或 app:&lt;应用&gt;</span>
        </div>
        <div className="icon-row">
          <span className="icon-preview">
            <Icon name={iconOf(item)} size={20} appIcons={icons} />
          </span>
          <input value={item.icon ?? ''} placeholder="留空则自动" onChange={(e) => set({ icon: e.target.value || undefined })} spellCheck={false} />
        </div>
        {!iconOk && <div className="field-err">找不到图标「{iconName}」，将显示为圆圈</div>}
        <input className="icon-search" placeholder="搜索图标…" value={iconQ} onChange={(e) => setIconQ(e.target.value)} />
        <div className="icon-grid">
          {ICON_SUGGESTIONS.filter((n) => !iconQ || n.toLowerCase().includes(iconQ.toLowerCase())).map((n) => (
            <button key={n} className={iconName === n ? 'active' : ''} title={n} onClick={() => set({ icon: n })}>
              <Icon name={n} size={16} />
            </button>
          ))}
          {iconQ && hasLucide(capitalize(iconQ)) && !ICON_SUGGESTIONS.includes(capitalize(iconQ)) && (
            <button title={capitalize(iconQ)} onClick={() => set({ icon: capitalize(iconQ) })}>
              <Icon name={capitalize(iconQ)} size={16} />
            </button>
          )}
        </div>
      </div>

      <div className="field">
        <div className="field-label">
          <span>动作</span>
        </div>
        <select value={item.action.type} onChange={(e) => onChange({ ...item, action: defaultAction(e.target.value) })}>
          {groups.map((g) => (
            <optgroup key={g} label={g}>
              {ACTIONS.filter((a) => a.group === g).map((a) => (
                <option key={a.type} value={a.type}>
                  {a.label}
                </option>
              ))}
            </optgroup>
          ))}
          {!def && <option value={item.action.type}>{item.action.type}（只能在配置文件中编辑）</option>}
        </select>
      </div>

      {def ? (
        def.fields.map((f) => <Field key={f.key} def={f} value={item.action[f.key]} env={env} onChange={(v) => setAction(f.key, v)} />)
      ) : (
        <pre className="raw">{JSON.stringify(item.action, null, 2)}</pre>
      )}
    </div>
  );
}

function capitalize(s: string): string {
  return s
    .trim()
    .split(/[-_\s]+/)
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join('');
}
