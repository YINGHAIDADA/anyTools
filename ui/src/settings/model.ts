// The wheel config as the editor sees it: a mutable copy of `modules.wheel` from config.yaml.
// Values that may be per-platform are either a string or { mac?, win? }.

export type PP<T> = T | { mac?: T; win?: T };

export interface ActionCfg {
  type: string;
  [k: string]: unknown;
}

export interface ItemCfg {
  id?: string;
  label: string;
  icon?: string;
  action: ActionCfg;
}

export interface WheelCfg {
  name?: string;
  extends?: string;
  items?: ItemCfg[];
  add?: ItemCfg[];
  remove?: string[];
}

export interface WhenCfg {
  app?: PP<string | string[]>;
  title?: string;
  selection?: 'none' | 'files' | 'file' | 'folder' | 'text';
  ext?: string | string[];
  count?: { min?: number; max?: number };
  platform?: 'mac' | 'win';
}

export interface ProfileCfg {
  when?: WhenCfg;
  wheel: string;
}

export interface TriggerCfg {
  keys?: PP<string>;
  button?: string;
  tap?: 'click' | 'replay' | 'none';
  wheel?: string;
}

export interface WheelModuleCfg {
  triggers: TriggerCfg[];
  profiles: ProfileCfg[];
  wheels: Record<string, WheelCfg>;
}

export interface ConfigFile {
  version: number;
  settings: { theme: 'auto' | 'dark' | 'light'; accent: string; terminal?: PP<string> | null; material: string };
  modules: { wheel?: WheelModuleCfg } & Record<string, unknown>;
}

export type Platform = 'mac' | 'win';

/** Reads the value for `p` from a possibly per-platform value. */
export function forPlatform<T>(v: PP<T> | undefined, p: Platform): T | undefined {
  if (v === undefined || v === null) return undefined;
  if (typeof v === 'object' && !Array.isArray(v) && ('mac' in (v as object) || 'win' in (v as object))) {
    return (v as { mac?: T; win?: T })[p];
  }
  return v as T;
}

export function isSplit<T>(v: PP<T> | undefined): v is { mac?: T; win?: T } {
  return typeof v === 'object' && v !== null && !Array.isArray(v) && ('mac' in v || 'win' in v);
}

/** Sets the value for `p`, keeping (or creating) the other platform's value when they differ. */
export function setForPlatform<T>(v: PP<T> | undefined, p: Platform, value: T | undefined, split: boolean): PP<T> | undefined {
  if (!split) return value;
  const cur = isSplit(v) ? { ...v } : v === undefined ? {} : { mac: v as T, win: v as T };
  if (value === undefined || value === '') delete (cur as Record<string, unknown>)[p];
  else (cur as Record<string, T>)[p] = value;
  return Object.keys(cur).length ? cur : undefined;
}

export const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));

// ------------------------------------------------------------------------------------------------
// Action catalog: what the editor offers and which fields each action has.

export type FieldKind = 'chord' | 'text' | 'longtext' | 'app' | 'url' | 'wheel' | 'script' | 'bool' | 'number' | 'code' | 'select';

export interface FieldDef {
  key: string;
  label: string;
  kind: FieldKind;
  perPlatform?: boolean;
  required?: boolean;
  options?: { value: string; label: string }[];
  placeholder?: string;
  hint?: string;
}

export interface ActionDef {
  type: string;
  label: string;
  icon: string;
  group: '常用' | '文件' | '脚本' | '组合';
  fields: FieldDef[];
}

export const ACTIONS: ActionDef[] = [
  { type: 'keystroke', label: '发送快捷键', icon: 'Keyboard', group: '常用', fields: [{ key: 'keys', label: '快捷键', kind: 'chord', perPlatform: true, required: true }] },
  {
    type: 'text',
    label: '输入文本',
    icon: 'Type',
    group: '常用',
    fields: [
      { key: 'text', label: '文本', kind: 'longtext', required: true },
      {
        key: 'mode',
        label: '方式',
        kind: 'select',
        options: [
          { value: '', label: '逐字输入' },
          { value: 'paste', label: '粘贴（长文本更快，会恢复剪贴板）' },
        ],
      },
    ],
  },
  { type: 'launch', label: '启动应用', icon: 'AppWindow', group: '常用', fields: [{ key: 'target', label: '应用', kind: 'app', perPlatform: true, required: true }] },
  { type: 'open', label: '打开网址或文件', icon: 'ExternalLink', group: '常用', fields: [{ key: 'target', label: '地址 / 路径', kind: 'url', perPlatform: true, required: true, placeholder: 'https://… 或 ~/Documents' }] },
  { type: 'submenu', label: '打开子轮盘', icon: 'LayoutGrid', group: '常用', fields: [{ key: 'wheel', label: '轮盘', kind: 'wheel', required: true }] },
  { type: 'file.open_terminal', label: '在此打开终端', icon: 'SquareTerminal', group: '文件', fields: [{ key: 'terminal', label: '终端（可选）', kind: 'app', perPlatform: true, hint: '留空则用设置中的默认终端' }] },
  { type: 'file.copy', label: '复制文件', icon: 'Copy', group: '文件', fields: [] },
  { type: 'file.cut', label: '剪切文件', icon: 'Scissors', group: '文件', fields: [] },
  { type: 'file.paste_here', label: '粘贴到此', icon: 'ClipboardPaste', group: '文件', fields: [] },
  {
    type: 'file.copy_path',
    label: '复制路径',
    icon: 'Link',
    group: '文件',
    fields: [{ key: 'quote', label: '给路径加引号', kind: 'bool' }],
  },
  { type: 'file.zip', label: '压缩', icon: 'FileArchive', group: '文件', fields: [] },
  { type: 'file.trash', label: '移到废纸篓', icon: 'Trash2', group: '文件', fields: [{ key: 'confirm', label: '删除前确认', kind: 'bool' }] },
  { type: 'file.open_with', label: '用应用打开', icon: 'AppWindow', group: '文件', fields: [{ key: 'app', label: '应用', kind: 'app', perPlatform: true, required: true }] },
  { type: 'file.new_folder', label: '新建文件夹', icon: 'FolderPlus', group: '文件', fields: [{ key: 'name', label: '名称（可选）', kind: 'text', placeholder: '未命名文件夹' }] },
  {
    type: 'script',
    label: 'JS / TS 脚本',
    icon: 'FileCode2',
    group: '脚本',
    fields: [
      { key: 'file', label: '脚本文件', kind: 'script', required: true },
      { key: 'timeout_ms', label: '超时（毫秒）', kind: 'number', placeholder: '30000' },
    ],
  },
  {
    type: 'shell',
    label: 'Shell 命令',
    icon: 'SquareTerminal',
    group: '脚本',
    fields: [
      { key: 'code', label: '命令', kind: 'code', perPlatform: true, required: true },
      { key: 'notify_output', label: '用通知显示输出', kind: 'bool' },
    ],
  },
  { type: 'applescript', label: 'AppleScript', icon: 'Apple', group: '脚本', fields: [{ key: 'code', label: '脚本', kind: 'code', required: true }, { key: 'notify_output', label: '用通知显示输出', kind: 'bool' }] },
  { type: 'powershell', label: 'PowerShell', icon: 'SquareTerminal', group: '脚本', fields: [{ key: 'code', label: '脚本', kind: 'code', required: true }, { key: 'notify_output', label: '用通知显示输出', kind: 'bool' }] },
];

export function actionDef(type: string): ActionDef | undefined {
  return ACTIONS.find((a) => a.type === type);
}

/** A short human description of an action, for item rows. */
export function actionSummary(a: ActionCfg, p: Platform): string {
  const def = actionDef(a.type);
  if (a.type === 'sequence') return `组合动作（${(a.steps as unknown[] | undefined)?.length ?? 0} 步）`;
  if (!def) return a.type;
  const main = def.fields[0];
  const v = main ? forPlatform(a[main.key] as PP<unknown>, p) : undefined;
  if (v === undefined || v === '' || typeof v === 'boolean') return def.label;
  const s = String(v);
  return `${def.label} · ${s.length > 40 ? s.slice(0, 40) + '…' : s}`;
}

export function defaultAction(type: string): ActionCfg {
  const a: ActionCfg = { type };
  if (type === 'file.trash') a.confirm = true;
  return a;
}

export function nextId(items: ItemCfg[], base = 'item'): string {
  const used = new Set(items.map((i) => i.id));
  for (let n = 1; ; n++) if (!used.has(`${base}${n}`)) return `${base}${n}`;
}

export function wheelKey(existing: string[], base = 'wheel'): string {
  const used = new Set(existing);
  for (let n = 1; ; n++) if (!used.has(`${base}${n}`)) return `${base}${n}`;
}

/**
 * Items of a wheel as they appear (inheritance applied). The editor edits `items` for plain wheels
 * and `add` / `remove` for wheels that extend another one.
 */
export function effectiveItems(cfg: WheelModuleCfg, key: string, seen: string[] = []): ItemCfg[] {
  const w = cfg.wheels[key];
  if (!w || seen.includes(key)) return [];
  if (!w.extends) return w.items ?? [];
  let items = effectiveItems(cfg, w.extends, [...seen, key]).filter((it) => !(w.remove ?? []).includes(it.id ?? ''));
  for (const a of w.add ?? []) {
    const i = items.findIndex((it) => it.id !== undefined && it.id === a.id);
    if (i >= 0) items = items.map((it, j) => (j === i ? a : it));
    else items = [...items, a];
  }
  return items;
}
