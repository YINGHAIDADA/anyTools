//! The `modules.wheel` config section and its compiled, validated form.

use anytools_core::keys::{parse_chord, Chord, Mods};
use anytools_core::platform::{Button, Keyboard};
use anytools_core::resolver::{When, WhenCfg};
use anytools_core::{ActionSpec, Context, PerPlatform, Platform, Registry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const MIN_ITEMS: usize = 2;
pub const MAX_ITEMS: usize = 12;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WheelModuleCfg {
    #[serde(default)]
    pub triggers: Vec<TriggerCfg>,
    #[serde(default)]
    pub profiles: Vec<ProfileCfg>,
    #[serde(default)]
    pub wheels: BTreeMap<String, WheelCfg>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TapMode {
    /// A quick tap keeps the wheel open; click to choose.
    #[default]
    Click,
    /// A quick tap closes the wheel and sends the trigger key on, so the key keeps its normal use.
    Replay,
    /// A quick tap just closes the wheel.
    None,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerCfg {
    #[serde(default)]
    pub keys: Option<PerPlatform<String>>,
    /// Mouse button: middle | back | forward.
    #[serde(default)]
    pub button: Option<String>,
    #[serde(default)]
    pub tap: TapMode,
    /// Always open this wheel instead of resolving profiles.
    #[serde(default)]
    pub wheel: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCfg {
    #[serde(default)]
    pub when: Option<WhenCfg>,
    pub wheel: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WheelCfg {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default)]
    pub items: Option<Vec<ItemCfg>>,
    /// With `extends`: items to append, or to replace (same `id`).
    #[serde(default)]
    pub add: Vec<ItemCfg>,
    /// With `extends`: ids of inherited items to drop.
    #[serde(default)]
    pub remove: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemCfg {
    #[serde(default)]
    pub id: Option<String>,
    pub label: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub action: ActionSpec,
}

// ------------------------------------------------------------------------------------------------
// Compiled form
// ------------------------------------------------------------------------------------------------

pub type WheelId = usize;

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Action(ActionSpec),
    Submenu(WheelId),
    /// A menu action: its choices are computed when it opens and shown as a submenu.
    Menu(ActionSpec),
}

impl ItemKind {
    pub fn opens_submenu(&self) -> bool {
        matches!(self, ItemKind::Submenu(_) | ItemKind::Menu(_))
    }
}

#[derive(Clone, Debug)]
pub struct ItemDef {
    pub id: String,
    pub label: String,
    /// `lucide:Name`, `app:<target>` or `file:<path>`.
    pub icon: String,
    pub kind: ItemKind,
}

#[derive(Clone, Debug)]
pub struct WheelDef {
    pub id: String,
    pub name: String,
    pub items: Vec<ItemDef>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TriggerInput {
    Key { code: u32, mods: Mods, chord: Chord },
    Button(Button),
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub input: TriggerInput,
    pub tap: TapMode,
    pub wheel: Option<WheelId>,
}

#[derive(Clone, Debug)]
pub struct Compiled {
    pub wheels: Vec<WheelDef>,
    pub profiles: Vec<(When, WheelId)>,
    pub fallback: WheelId,
    pub triggers: Vec<Trigger>,
    /// Some profile looks at the window title (so it's worth fetching).
    pub needs_title: bool,
    /// Some profile matches on selected text (so it's worth reading it outside file managers).
    pub needs_text: bool,
}

impl Compiled {
    pub fn resolve(&self, ctx: &Context) -> WheelId {
        self.profiles.iter().find(|(w, _)| w.matches(ctx)).map(|(_, id)| *id).unwrap_or(self.fallback)
    }

    pub fn wheel_index(&self, id: &str) -> Option<WheelId> {
        self.wheels.iter().position(|w| w.id == id)
    }
}

fn default_icon(action: &ActionSpec) -> String {
    let target = action.params.get("target").and_then(|t| serde_json::from_value::<PerPlatform<String>>(t.clone()).ok());
    match action.kind.as_str() {
        "submenu" => "lucide:LayoutGrid".into(),
        "launch" => match target.as_ref().and_then(|t| t.get()) {
            Some(t) => format!("app:{t}"),
            None => "lucide:AppWindow".into(),
        },
        "open" => "lucide:ExternalLink".into(),
        "keystroke" => "lucide:Keyboard".into(),
        "text" => "lucide:Type".into(),
        "shell" | "applescript" | "powershell" => "lucide:SquareTerminal".into(),
        "sequence" => "lucide:ListOrdered".into(),
        _ => "lucide:Circle".into(),
    }
}

fn normalize_icon(icon: Option<&str>, action: &ActionSpec) -> String {
    match icon.map(str::trim).filter(|s| !s.is_empty()) {
        None => default_icon(action),
        Some("app:auto") => match default_icon(action) {
            s if s.starts_with("app:") => s,
            _ => "lucide:AppWindow".into(),
        },
        Some(s) if s.starts_with("app:") || s.starts_with("file:") || s.starts_with("lucide:") => s.into(),
        Some(s) => format!("lucide:{s}"),
    }
}

/// Expands `extends` chains into flat item lists.
fn flatten(cfg: &WheelModuleCfg) -> Result<BTreeMap<String, Vec<ItemCfg>>, String> {
    fn go(
        id: &str,
        cfg: &WheelModuleCfg,
        done: &mut BTreeMap<String, Vec<ItemCfg>>,
        visiting: &mut Vec<String>,
    ) -> Result<Vec<ItemCfg>, String> {
        if let Some(v) = done.get(id) {
            return Ok(v.clone());
        }
        if visiting.iter().any(|v| v == id) {
            visiting.push(id.into());
            return Err(format!("轮盘继承出现循环: {}", visiting.join(" → ")));
        }
        let w = cfg.wheels.get(id).ok_or_else(|| format!("轮盘 \"{id}\" 不存在"))?;
        visiting.push(id.into());
        let mut items = match (&w.extends, &w.items) {
            (Some(_), Some(_)) => return Err(format!("轮盘 \"{id}\"：用了 extends 时请用 add / remove 修改继承来的项，不要再写 items")),
            (None, None) => return Err(format!("轮盘 \"{id}\" 需要 items（或 extends）")),
            (None, Some(items)) => {
                if !w.add.is_empty() || !w.remove.is_empty() {
                    return Err(format!("轮盘 \"{id}\"：add / remove 只能和 extends 一起用"));
                }
                items.clone()
            }
            (Some(base), None) => {
                let mut items = go(base, cfg, done, visiting).map_err(|e| {
                    if e.contains("不存在") && !e.contains("循环") {
                        format!("轮盘 \"{id}\" 继承的 \"{base}\" 不存在")
                    } else {
                        e
                    }
                })?;
                for r in &w.remove {
                    let before = items.len();
                    items.retain(|it| it.id.as_deref() != Some(r));
                    if items.len() == before {
                        return Err(format!("轮盘 \"{id}\"：remove 里的 \"{r}\" 在 \"{base}\" 中不存在"));
                    }
                }
                for a in &w.add {
                    match a.id.as_deref().and_then(|aid| items.iter().position(|it| it.id.as_deref() == Some(aid))) {
                        Some(pos) => items[pos] = a.clone(),
                        None => items.push(a.clone()),
                    }
                }
                items
            }
        };
        // Stable ids for items written without one.
        for (i, it) in items.iter_mut().enumerate() {
            if it.id.is_none() {
                it.id = Some(format!("#{i}"));
            }
        }
        visiting.pop();
        done.insert(id.into(), items.clone());
        Ok(items)
    }

    let mut done = BTreeMap::new();
    for id in cfg.wheels.keys() {
        go(id, cfg, &mut done, &mut vec![])?;
    }
    Ok(done)
}

fn compile_trigger(i: usize, t: &TriggerCfg, kb: &dyn Keyboard, p: Platform) -> Result<Option<TriggerInput>, String> {
    let at = format!("第 {} 个触发器", i + 1);
    match (&t.keys, &t.button) {
        (Some(_), Some(_)) => Err(format!("{at}：keys 和 button 只能选一个")),
        (None, None) => Err(format!("{at}：需要 keys 或 button")),
        (Some(keys), None) => {
            let Some(s) = keys.get_for(p) else { return Ok(None) };
            let chord = parse_chord(s, p).map_err(|e| format!("{at}：{e}"))?;
            if p == Platform::Mac && chord.key == "capslock" {
                return Err(format!("{at}：macOS 上 CapsLock 不能作为按住触发键（系统不报告它的抬起）"));
            }
            let code = kb.code_of(&chord.key).ok_or_else(|| format!("{at}：不支持的按键 \"{}\"", chord.key))?;
            Ok(Some(TriggerInput::Key { code, mods: chord.mods, chord }))
        }
        (None, Some(b)) => {
            let button = Button::parse(b).ok_or_else(|| format!("{at}：未知的鼠标按键 \"{b}\"（可用 middle / back / forward）"))?;
            if matches!(button, Button::Left | Button::Right) {
                return Err(format!("{at}：不能用鼠标左键或右键作触发器"));
            }
            if t.tap == TapMode::Replay {
                return Err(format!("{at}：鼠标按键触发器不支持 tap: replay"));
            }
            Ok(Some(TriggerInput::Button(button)))
        }
    }
}

/// Parses and fully validates the section. `reg` checks every action; `kb` maps trigger keys.
pub fn compile(raw: &Value, reg: &Registry, kb: &dyn Keyboard) -> Result<Compiled, String> {
    compile_for(raw, reg, kb, Platform::current())
}

pub fn compile_for(raw: &Value, reg: &Registry, kb: &dyn Keyboard, p: Platform) -> Result<Compiled, String> {
    let cfg: WheelModuleCfg = if raw.is_null() {
        return Err("缺少 modules.wheel 配置".into());
    } else {
        serde_json::from_value(raw.clone()).map_err(|e| format!("modules.wheel 格式错误: {e}"))?
    };
    if cfg.wheels.is_empty() {
        return Err("至少需要定义一个轮盘".into());
    }
    let flat = flatten(&cfg)?;
    let ids: HashMap<&str, WheelId> = flat.keys().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let wheel_ref = |name: &str, what: &str| ids.get(name).copied().ok_or_else(|| format!("{what} 引用的轮盘 \"{name}\" 不存在"));

    let mut wheels = Vec::with_capacity(flat.len());
    for (id, items) in &flat {
        if !(MIN_ITEMS..=MAX_ITEMS).contains(&items.len()) {
            return Err(format!("轮盘 \"{id}\" 有 {} 项，应在 {MIN_ITEMS}–{MAX_ITEMS} 项之间", items.len()));
        }
        let mut seen = HashSet::new();
        let mut defs = Vec::with_capacity(items.len());
        for it in items {
            let item_id = it.id.clone().unwrap_or_default();
            if !seen.insert(item_id.clone()) {
                return Err(format!("轮盘 \"{id}\" 中有重复的项 id \"{item_id}\""));
            }
            let at = format!("轮盘 \"{id}\" 的「{}」", it.label);
            let kind = if it.action.kind == "submenu" {
                let target = it.action.params.get("wheel").and_then(Value::as_str).ok_or_else(|| format!("{at}：submenu 需要 wheel 参数"))?;
                ItemKind::Submenu(wheel_ref(target, &at)?)
            } else {
                reg.validate(&it.action).map_err(|e| format!("{at}：{e}"))?;
                if reg.is_menu(&it.action) {
                    ItemKind::Menu(it.action.clone())
                } else {
                    ItemKind::Action(it.action.clone())
                }
            };
            defs.push(ItemDef { id: item_id, label: it.label.clone(), icon: normalize_icon(it.icon.as_deref(), &it.action), kind });
        }
        let name = cfg.wheels.get(id).and_then(|w| w.name.clone()).unwrap_or_else(|| id.clone());
        wheels.push(WheelDef { id: id.clone(), name, items: defs });
    }

    let mut profiles = Vec::new();
    let mut needs_title = false;
    let mut needs_text = false;
    for (i, pr) in cfg.profiles.iter().enumerate() {
        let at = format!("第 {} 条规则", i + 1);
        let wcfg = pr.when.clone().unwrap_or_default();
        needs_title |= wcfg.title.is_some();
        let when = When::compile_for(&wcfg, p).map_err(|e| format!("{at}：{e}"))?;
        needs_text |= when.uses_text();
        profiles.push((when, wheel_ref(&pr.wheel, &at)?));
    }
    let fallback = ids.get("default").copied().unwrap_or(0);

    let mut triggers = Vec::new();
    for (i, t) in cfg.triggers.iter().enumerate() {
        if let Some(input) = compile_trigger(i, t, kb, p)? {
            if triggers.iter().any(|x: &Trigger| x.input == input) {
                return Err(format!("第 {} 个触发器和前面的重复", i + 1));
            }
            let wheel = match &t.wheel {
                Some(w) => Some(wheel_ref(w, &format!("第 {} 个触发器", i + 1))?),
                None => None,
            };
            triggers.push(Trigger { input, tap: t.tap, wheel });
        }
    }
    if triggers.is_empty() {
        tracing::warn!("wheel: no trigger configured for this platform");
    }

    Ok(Compiled { wheels, profiles, fallback, triggers, needs_title, needs_text })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytools_core::{AppInfo, Selection};
    use anytools_platform::mock::Mock;
    use std::path::PathBuf;

    pub fn compile_yaml(yaml: &str) -> Result<Compiled, String> {
        let reg = Registry::default();
        anytools_actions::register_all(&reg);
        let raw: Value = serde_yaml_ng::from_str(yaml).unwrap();
        compile_for(&raw, &reg, &Mock::new(), Platform::Mac)
    }

    const BASE: &str = r#"
triggers:
  - keys: { mac: "alt+space", win: "capslock" }
profiles:
  - when: { selection: files }
    wheel: files
  - when: { app: { mac: com.microsoft.VSCode } }
    wheel: vscode
wheels:
  default:
    name: 快捷
    items:
      - { id: copy, label: 复制, action: { type: keystroke, keys: "mod+c" } }
      - { id: paste, label: 粘贴, action: { type: keystroke, keys: "mod+v" } }
      - { id: calc, label: 计算器, action: { type: launch, target: Calculator } }
      - { id: more, label: 更多, action: { type: submenu, wheel: tools } }
  vscode:
    extends: default
    remove: [calc]
    add:
      - { id: copy, label: 复制行, action: { type: keystroke, keys: "mod+shift+c" } }
      - { id: palette, label: 命令面板, icon: Command, action: { type: keystroke, keys: "mod+shift+p" } }
  tools:
    items:
      - { label: A, action: { type: text, text: a } }
      - { label: B, action: { type: text, text: b } }
  files:
    items:
      - { label: 路径, action: { type: file.copy_path } }
      - { label: 删除, action: { type: file.trash } }
"#;

    #[test]
    fn compiles_and_merges_extends() {
        let c = compile_yaml(BASE).unwrap();
        let v = &c.wheels[c.wheel_index("vscode").unwrap()];
        let ids: Vec<&str> = v.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["copy", "paste", "more", "palette"], "remove drops, add replaces in place / appends");
        assert_eq!(v.items[0].label, "复制行");
        assert_eq!(v.items[3].icon, "lucide:Command");
        let d = &c.wheels[c.wheel_index("default").unwrap()];
        assert_eq!(d.name, "快捷");
        assert_eq!(d.items[2].icon, "app:Calculator", "launch items default to the app icon");
        assert_eq!(d.items[3].kind, ItemKind::Submenu(c.wheel_index("tools").unwrap()));
        let t = &c.wheels[c.wheel_index("tools").unwrap()];
        assert_eq!(t.items[1].id, "#1");
        assert_eq!(c.fallback, c.wheel_index("default").unwrap());
        assert_eq!(c.triggers.len(), 1);
        assert!(matches!(&c.triggers[0].input, TriggerInput::Key { mods, .. } if *mods == Mods::ALT));
    }

    #[test]
    fn resolves_profiles_in_order() {
        let c = compile_yaml(BASE).unwrap();
        let mut ctx = Context {
            app: Some(AppInfo { id: "com.microsoft.VSCode".into(), ..Default::default() }),
            ..Default::default()
        };
        assert_eq!(c.resolve(&ctx), c.wheel_index("vscode").unwrap());
        ctx.selection = Selection::Files { paths: vec![PathBuf::from("/x")] };
        ctx.selection_known = true;
        assert_eq!(c.resolve(&ctx), c.wheel_index("files").unwrap(), "first matching rule wins");
        assert_eq!(c.resolve(&Context::default()), c.wheel_index("default").unwrap());
    }

    fn err(yaml: &str) -> String {
        compile_yaml(yaml).unwrap_err()
    }

    #[test]
    fn readable_errors() {
        let two = "[ { label: a, action: { type: text, text: a } }, { label: b, action: { type: text, text: b } } ]";
        assert!(err(&format!("wheels: {{ a: {{ extends: b }}, b: {{ extends: a }} }}")).contains("循环"));
        assert!(err(&format!("wheels: {{ a: {{ extends: nope }} }}")).contains("\"nope\" 不存在"));
        assert!(err(&format!("wheels: {{ a: {{ items: {two}, remove: [x] }} }}")).contains("extends"));
        assert!(err(&format!("wheels: {{ a: {{ items: {two} }}, b: {{ extends: a, remove: [zz] }} }}")).contains("zz"));
        assert!(err("wheels: { a: { items: [ { label: x, action: { type: text, text: a } } ] } }").contains("2–12"));
        assert!(err(&format!("wheels: {{ a: {{ items: [ {{ id: x, label: a, action: {{ type: text, text: a }} }}, {{ id: x, label: b, action: {{ type: text, text: b }} }} ] }} }}")).contains("重复"));
        assert!(err("wheels: { a: { items: [ { label: x, action: { type: submenu, wheel: q } }, { label: y, action: { type: text, text: a } } ] } }").contains("\"q\" 不存在"));
        let e = err("wheels: { a: { items: [ { label: 复制, action: { type: keystroke, keys: 'cmd+' } }, { label: y, action: { type: text, text: a } } ] } }");
        assert!(e.contains("「复制」"), "{e}");
        assert!(err(&format!("profiles: [ {{ wheel: zz }} ]\nwheels: {{ a: {{ items: {two} }} }}")).contains("第 1 条规则"));
        assert!(err(&format!("triggers: [ {{ keys: capslock }} ]\nwheels: {{ a: {{ items: {two} }} }}")).contains("CapsLock"));
        assert!(err(&format!("triggers: [ {{ button: left }} ]\nwheels: {{ a: {{ items: {two} }} }}")).contains("左键"));
        assert!(err(&format!("triggers: [ {{ button: middle, tap: replay }} ]\nwheels: {{ a: {{ items: {two} }} }}")).contains("replay"));
        assert!(err(&format!("triggers: [ {{ keys: f13 }}, {{ keys: F13 }} ]\nwheels: {{ a: {{ items: {two} }} }}")).contains("重复"));
        assert!(err("wheels: {}").contains("至少"));
        assert!(err("wheelz: {}").contains("格式错误"));
    }

    #[test]
    fn triggers_for_other_platforms_are_skipped() {
        let c = compile_yaml("triggers: [ { keys: { win: capslock } }, { button: middle, wheel: t } ]\nwheels: { t: { items: [ { label: a, action: { type: text, text: a } }, { label: b, action: { type: text, text: b } } ] } }").unwrap();
        assert_eq!(c.triggers.len(), 1);
        assert_eq!(c.triggers[0].input, TriggerInput::Button(Button::Middle));
        assert_eq!(c.triggers[0].wheel, Some(0));
    }
}
