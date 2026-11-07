use anytools_core::action::params;
use anytools_core::keys::parse_chord;
use anytools_core::{ActionCtx, ActionHandler, ActionSpec, Context, Params, PerPlatform, Platform, Registry};
use serde::Deserialize;
use std::time::Duration;

/// Resolves a per-platform value or explains that this platform has none.
fn this_platform<'a, T>(kind: &str, v: &'a PerPlatform<T>) -> Result<&'a T, String> {
    v.get().ok_or_else(|| format!("动作 {kind} 没有为当前平台配置值"))
}

fn configured_here<T>(v: &PerPlatform<T>) -> bool {
    v.get().is_some()
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeystrokeP {
    keys: PerPlatform<String>,
}

pub struct Keystroke;

impl ActionHandler for Keystroke {
    fn kind(&self) -> &'static str {
        "keystroke"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        let p: KeystrokeP = params(self.kind(), p)?;
        if let Some(k) = p.keys.get() {
            parse_chord(k, Platform::current())?;
        }
        Ok(())
    }
    fn available(&self, p: &Params, _: &Context) -> bool {
        params::<KeystrokeP>(self.kind(), p).is_ok_and(|p| configured_here(&p.keys))
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: KeystrokeP = params(self.kind(), p)?;
        let chord = parse_chord(this_platform(self.kind(), &p.keys)?, Platform::current())?;
        cx.services.keyboard.send_chord(&chord)
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextP {
    text: String,
    /// `type` (default): synthesize key events. `paste`: put on the clipboard and press ⌘V / Ctrl+V,
    /// then restore the previous clipboard text (better for long text and for apps that drop events).
    #[serde(default)]
    mode: Option<String>,
}

pub struct Text;

impl ActionHandler for Text {
    fn kind(&self) -> &'static str {
        "text"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        let p: TextP = params(self.kind(), p)?;
        match p.mode.as_deref() {
            None | Some("type") | Some("paste") => Ok(()),
            Some(m) => Err(format!("text.mode 只能是 type 或 paste，不是 \"{m}\"")),
        }
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: TextP = params(self.kind(), p)?;
        let s = cx.services;
        if p.mode.as_deref() == Some("paste") {
            // Borrow the clipboard: the text is marked transient (clipboard history skips it), and the
            // previous content comes back byte for byte afterwards.
            let previous = s.files.clipboard_snapshot();
            s.files.set_clipboard_text_transient(&p.text)?;
            s.keyboard.send_chord(&parse_chord("mod+v", Platform::current())?)?;
            // Give the target app time to read the clipboard before restoring it.
            std::thread::sleep(Duration::from_millis(250));
            s.files.clipboard_restore(&previous)
        } else {
            s.keyboard.type_text(&p.text)
        }
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetP {
    target: PerPlatform<String>,
}

pub struct Launch;

impl ActionHandler for Launch {
    fn kind(&self) -> &'static str {
        "launch"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<TargetP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, p: &Params, _: &Context) -> bool {
        params::<TargetP>(self.kind(), p).is_ok_and(|p| configured_here(&p.target))
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: TargetP = params(self.kind(), p)?;
        cx.services.shell.launch(this_platform(self.kind(), &p.target)?)
    }
}

pub struct Open;

impl ActionHandler for Open {
    fn kind(&self) -> &'static str {
        "open"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<TargetP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, p: &Params, ctx: &Context) -> bool {
        params::<TargetP>(self.kind(), p).is_ok_and(|p| {
            // A target that uses {text} needs selected text.
            configured_here(&p.target) && p.target.get().is_some_and(|t| !t.contains("{text}") || selected_text(ctx).is_some())
        })
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: TargetP = params(self.kind(), p)?;
        let target = this_platform(self.kind(), &p.target)?;
        let target = if target.contains("{text}") {
            let text = selected_text(cx.context).ok_or("没有选中文字")?;
            target.replace("{text}", &url_encode(text))
        } else {
            target.clone()
        };
        cx.services.shell.open(&target)
    }
}

fn selected_text(ctx: &Context) -> Option<&str> {
    match &ctx.selection {
        anytools_core::Selection::Text { text } if !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    }
}

/// Percent-encodes everything except unreserved characters (RFC 3986), for `{text}` in URLs.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum Step {
    Delay { delay: u64 },
    Action(ActionSpec),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SequenceP {
    steps: Vec<Step>,
}

/// Runs steps in order; `{ delay: ms }` waits. Stops at the first failing step.
pub struct Sequence;

const MAX_DELAY_MS: u64 = 10_000;

impl ActionHandler for Sequence {
    fn kind(&self) -> &'static str {
        "sequence"
    }
    fn validate(&self, p: &Params, reg: &Registry) -> Result<(), String> {
        let p: SequenceP = params(self.kind(), p)?;
        if p.steps.is_empty() {
            return Err("sequence.steps 不能为空".into());
        }
        for (i, s) in p.steps.iter().enumerate() {
            match s {
                Step::Delay { delay } if *delay > MAX_DELAY_MS => {
                    return Err(format!("sequence 第 {} 步：delay 最大 {MAX_DELAY_MS}ms", i + 1))
                }
                Step::Delay { .. } => {}
                Step::Action(a) => reg.validate(a).map_err(|e| format!("sequence 第 {} 步：{e}", i + 1))?,
            }
        }
        Ok(())
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: SequenceP = params(self.kind(), p)?;
        for (i, s) in p.steps.iter().enumerate() {
            match s {
                Step::Delay { delay } => std::thread::sleep(Duration::from_millis((*delay).min(MAX_DELAY_MS))),
                Step::Action(a) => cx.run(a).map_err(|e| format!("第 {} 步失败：{e}", i + 1))?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil::*;
    use anytools_core::keys::{Chord, Mods};
    use anytools_core::Context;
    use anytools_platform::mock::Call;

    #[test]
    fn keystroke_uses_mod() {
        let (reg, mock) = setup();
        run(&reg, &mock, &Context::default(), "{ type: keystroke, keys: 'mod+shift+p' }").unwrap();
        let want = if cfg!(target_os = "macos") { Mods::CMD } else { Mods::CTRL } | Mods::SHIFT;
        assert_eq!(mock.calls(), [Call::Chord(Chord { mods: want, key: "p".into() })]);
    }

    #[test]
    fn validation_errors_are_readable() {
        let (reg, _) = setup();
        let err = reg.validate(&spec("{ type: keystroke, keys: 'cmd+shift' }")).unwrap_err();
        assert!(err.contains("缺少主键"), "{err}");
        let err = reg.validate(&spec("{ type: keystroke, key: 'a' }")).unwrap_err();
        assert!(err.contains("keystroke") && err.contains("参数有误"), "{err}");
        let err = reg.validate(&spec("{ type: nope }")).unwrap_err();
        assert!(err.contains("未知的动作类型"), "{err}");
    }

    #[test]
    fn per_platform_availability() {
        let (reg, _) = setup();
        let ctx = Context::default();
        let other = if cfg!(target_os = "macos") { "win" } else { "mac" };
        let s = spec(&format!("{{ type: launch, target: {{ {other}: X }} }}"));
        reg.validate(&s).unwrap();
        assert!(!reg.available(&s, &ctx), "no value for this platform → greyed out");
        assert!(reg.available(&spec("{ type: launch, target: Calculator }"), &ctx));
    }

    #[test]
    fn sequence_runs_in_order_and_stops_on_error() {
        let (reg, mock) = setup();
        let ctx = Context::default();
        run(&reg, &mock, &ctx, "{ type: sequence, steps: [ { type: text, text: a }, { delay: 1 }, { type: open, target: 'https://x' } ] }").unwrap();
        assert_eq!(mock.calls(), [Call::Text("a".into()), Call::Open("https://x".into())]);

        let err = reg.validate(&spec("{ type: sequence, steps: [ { type: text, text: a }, { type: keystroke, keys: '' } ] }")).unwrap_err();
        assert!(err.contains("第 2 步"), "{err}");
        assert!(reg.validate(&spec("{ type: sequence, steps: [] }")).is_err());
        assert!(reg.validate(&spec("{ type: sequence, steps: [ { delay: 99999 } ] }")).is_err());
    }

    #[test]
    fn open_substitutes_selected_text() {
        let (reg, mock) = setup();
        let ctx = Context { selection: anytools_core::Selection::Text { text: " 你好 world&x ".into() }, selection_known: true, ..Default::default() };
        run(&reg, &mock, &ctx, "{ type: open, target: 'https://www.google.com/search?q={text}' }").unwrap();
        assert_eq!(mock.calls(), [Call::Open("https://www.google.com/search?q=%E4%BD%A0%E5%A5%BD%20world%26x".into())]);
        let s = spec("{ type: open, target: 'https://x/?q={text}' }");
        assert!(!reg.available(&s, &Context::default()), "needs selected text");
        assert!(reg.available(&s, &ctx));
    }

    #[test]
    fn paste_mode_restores_clipboard() {
        let (reg, mock) = setup();
        mock.state.lock().unwrap().clip_text = Some("old".into());
        run(&reg, &mock, &Context::default(), "{ type: text, text: new, mode: paste }").unwrap();
        let calls = mock.calls();
        assert_eq!(calls[0], Call::ClipText("new".into()));
        assert!(matches!(calls[1], Call::Chord(_)));
        let st = mock.state.lock().unwrap();
        assert_eq!(st.clip_text.as_deref(), Some("old"));
        assert_eq!(st.clip_restored_at, st.clip_changes, "history skips the restore");
    }
}
