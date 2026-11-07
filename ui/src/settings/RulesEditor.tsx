import { ChordInput } from './fields';
import { AppInput } from './fields';
import { Icon } from './Icon';
import type { Platform, ProfileCfg, TriggerCfg, WhenCfg, WheelModuleCfg } from './model';
import { forPlatform, isSplit, setForPlatform } from './model';

interface Props {
  cfg: WheelModuleCfg;
  setCfg: (f: (c: WheelModuleCfg) => void) => void;
  platform: Platform;
}

const SEL_OPTIONS: { value: WhenCfg['selection'] | ''; label: string }[] = [
  { value: '', label: '不限' },
  { value: 'files', label: '选中了文件或文件夹' },
  { value: 'file', label: '只选中了文件' },
  { value: 'folder', label: '只选中了文件夹' },
  { value: 'none', label: '没有选中任何东西' },
];

const listText = (v: string | string[] | undefined) => (v === undefined ? '' : Array.isArray(v) ? v.join(', ') : v);
const parseList = (s: string): string | string[] | undefined => {
  const parts = s
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);
  return parts.length === 0 ? undefined : parts.length === 1 ? parts[0] : parts;
};

export function TriggersEditor({ cfg, setCfg, platform }: Props) {
  const wheels = Object.entries(cfg.wheels).map(([k, w]) => ({ key: k, name: w.name ?? k }));
  const setT = (i: number, f: (t: TriggerCfg) => void) => setCfg((c) => f(c.triggers[i]));
  return (
    <section>
      <h2>唤起方式</h2>
      <div className="card">
        {cfg.triggers.map((t, i) => {
          const isKey = t.button === undefined;
          const keys = forPlatform(t.keys, platform) ?? '';
          return (
            <div className="trigger" key={i}>
              <div className="trigger-main">
                <select
                  value={isKey ? 'keys' : t.button}
                  onChange={(e) =>
                    setT(i, (x) => {
                      if (e.target.value === 'keys') {
                        delete x.button;
                        x.keys = x.keys ?? '';
                      } else {
                        delete x.keys;
                        x.button = e.target.value;
                        if (x.tap === 'replay') x.tap = 'none';
                      }
                    })
                  }
                >
                  <option value="keys">键盘快捷键</option>
                  <option value="middle">鼠标中键</option>
                  <option value="back">鼠标后退键</option>
                  <option value="forward">鼠标前进键</option>
                </select>
                {isKey && (
                  <ChordInput
                    value={keys}
                    platform={platform}
                    onChange={(v) => setT(i, (x) => void (x.keys = setForPlatform(x.keys, platform, v, isSplit(x.keys))))}
                  />
                )}
                <button className="icon-btn danger" title="删除" onClick={() => setCfg((c) => void c.triggers.splice(i, 1))}>
                  <Icon name="X" size={14} />
                </button>
              </div>
              <div className="trigger-opts">
                <label>
                  短按时
                  <select value={t.tap ?? 'click'} onChange={(e) => setT(i, (x) => void (x.tap = e.target.value as TriggerCfg['tap']))}>
                    <option value="click">轮盘保持打开，点击选择</option>
                    {isKey && <option value="replay">关闭轮盘，按键照常生效</option>}
                    <option value="none">关闭轮盘</option>
                  </select>
                </label>
                <label>
                  打开
                  <select value={t.wheel ?? ''} onChange={(e) => setT(i, (x) => void (x.wheel = e.target.value || undefined))}>
                    <option value="">按规则自动选择</option>
                    {wheels.map((w) => (
                      <option key={w.key} value={w.key}>
                        固定：{w.name}
                      </option>
                    ))}
                  </select>
                </label>
                {isKey && isSplit(t.keys) && <span className="field-hint">已按平台分别设置；这里修改的是 {platform === 'mac' ? 'macOS' : 'Windows'} 的值</span>}
              </div>
            </div>
          );
        })}
        <div className="actions">
          <button onClick={() => setCfg((c) => void c.triggers.push({ keys: '', tap: 'click' }))}>
            <Icon name="Plus" size={13} /> 添加唤起方式
          </button>
        </div>
      </div>
      <p className="hint">按住唤起、移到一格上松开执行。macOS 上 CapsLock 不能作为按住的唤起键。</p>
    </section>
  );
}

export function RulesEditor({ cfg, setCfg, platform }: Props) {
  const wheels = Object.entries(cfg.wheels).map(([k, w]) => ({ key: k, name: w.name ?? k }));
  const setP = (i: number, f: (p: ProfileCfg) => void) => setCfg((c) => f(c.profiles[i]));
  const setW = (i: number, f: (w: WhenCfg) => void) =>
    setP(i, (p) => {
      p.when = p.when ?? {};
      f(p.when);
      for (const k of Object.keys(p.when) as (keyof WhenCfg)[]) if (p.when[k] === undefined) delete p.when[k];
      if (Object.keys(p.when).length === 0) delete p.when;
    });
  const moveP = (i: number, j: number) =>
    setCfg((c) => {
      const [x] = c.profiles.splice(i, 1);
      c.profiles.splice(j, 0, x);
    });
  return (
    <section>
      <h2>切换规则</h2>
      <div className="card">
        {cfg.profiles.length === 0 && <p className="hint">还没有规则，始终使用默认轮盘。</p>}
        {cfg.profiles.map((p, i) => {
          const w = p.when ?? {};
          const app = forPlatform(w.app as never, platform) as string | string[] | undefined;
          return (
            <div className="rule" key={i}>
              <div className="rule-n">{i + 1}</div>
              <div className="rule-body">
                <div className="rule-line">
                  <span>当前台应用是</span>
                  <AppInput value={listText(app)} onChange={(v) => setW(i, (x) => void (x.app = setForPlatform(x.app as never, platform, parseList(v) as never, isSplit(x.app as never))))} />
                </div>
                <div className="rule-line">
                  <span>且选中内容</span>
                  <select value={w.selection ?? ''} onChange={(e) => setW(i, (x) => void (x.selection = (e.target.value || undefined) as WhenCfg['selection']))}>
                    {SEL_OPTIONS.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </select>
                  <span>扩展名</span>
                  <input className="narrow" placeholder="如 png, jpg" value={listText(w.ext)} onChange={(e) => setW(i, (x) => void (x.ext = parseList(e.target.value)))} />
                </div>
                <div className="rule-line">
                  <span>且窗口标题匹配</span>
                  <input placeholder="正则表达式（可选）" value={w.title ?? ''} onChange={(e) => setW(i, (x) => void (x.title = e.target.value || undefined))} spellCheck={false} />
                </div>
                <div className="rule-line">
                  <span>就打开</span>
                  <select value={p.wheel} onChange={(e) => setP(i, (x) => void (x.wheel = e.target.value))}>
                    {wheels.map((x) => (
                      <option key={x.key} value={x.key}>
                        {x.name}
                      </option>
                    ))}
                  </select>
                </div>
              </div>
              <div className="rule-tools">
                <button className="icon-btn" title="上移" disabled={i === 0} onClick={() => moveP(i, i - 1)}>
                  <Icon name="ArrowUp" size={13} />
                </button>
                <button className="icon-btn" title="下移" disabled={i === cfg.profiles.length - 1} onClick={() => moveP(i, i + 1)}>
                  <Icon name="ArrowDown" size={13} />
                </button>
                <button className="icon-btn danger" title="删除" onClick={() => setCfg((c) => void c.profiles.splice(i, 1))}>
                  <Icon name="X" size={13} />
                </button>
              </div>
            </div>
          );
        })}
        <div className="actions">
          <button onClick={() => setCfg((c) => void c.profiles.push({ when: {}, wheel: 'default' in c.wheels ? 'default' : Object.keys(c.wheels)[0] }))}>
            <Icon name="Plus" size={13} /> 添加规则
          </button>
        </div>
      </div>
      <p className="hint">从上到下匹配，第一条满足的规则生效；都不满足时使用默认轮盘。应用可以写多个（逗号分隔），支持 * 通配符。</p>
    </section>
  );
}
