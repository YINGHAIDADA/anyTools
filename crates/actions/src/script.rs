//! External scripts: shell / applescript / powershell. The trigger context is passed as environment
//! variables (`ANYTOOLS_APP`, `ANYTOOLS_CWD`, `ANYTOOLS_SELECTION` = newline-separated paths, …).

use anytools_core::action::params;
use anytools_core::platform::{ScriptLang, ScriptRun};
use anytools_core::{ActionCtx, ActionHandler, Context, Params, PerPlatform, Platform, Registry};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptP {
    #[serde(default)]
    code: Option<PerPlatform<String>>,
    #[serde(default)]
    file: Option<PerPlatform<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Show stdout as a notification when the script succeeds.
    #[serde(default)]
    notify_output: bool,
}

pub struct Script {
    pub kind: &'static str,
}

impl Script {
    fn lang(&self) -> ScriptLang {
        match self.kind {
            "applescript" => ScriptLang::AppleScript,
            "powershell" => ScriptLang::PowerShell,
            _ => ScriptLang::Shell,
        }
    }

    fn native_platform(&self) -> Option<Platform> {
        match self.kind {
            "applescript" => Some(Platform::Mac),
            "powershell" => Some(Platform::Win),
            _ => None,
        }
    }
}

pub fn context_env(ctx: &Context) -> Vec<(String, String)> {
    let mut env = vec![
        ("ANYTOOLS_APP".to_string(), ctx.app_id().to_string()),
        ("ANYTOOLS_APP_NAME".to_string(), ctx.app.as_ref().map(|a| a.name.clone()).unwrap_or_default()),
        ("ANYTOOLS_WINDOW_TITLE".to_string(), ctx.app.as_ref().and_then(|a| a.title.clone()).unwrap_or_default()),
        (
            "ANYTOOLS_SELECTION".to_string(),
            ctx.selection.files().iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n"),
        ),
        ("ANYTOOLS_CWD".to_string(), ctx.work_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()),
    ];
    if let anytools_core::Selection::Text { text } = &ctx.selection {
        env.push(("ANYTOOLS_TEXT".into(), text.clone()));
    }
    env
}

impl ActionHandler for Script {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        let p: ScriptP = params(self.kind, p)?;
        match (&p.code, &p.file) {
            (Some(_), Some(_)) => Err(format!("{} 动作只能指定 code 或 file 其中一个", self.kind)),
            (None, None) => Err(format!("{} 动作需要 code 或 file", self.kind)),
            _ => Ok(()),
        }
    }

    fn available(&self, p: &Params, _: &Context) -> bool {
        if self.native_platform().is_some_and(|n| n != Platform::current()) {
            return false;
        }
        params::<ScriptP>(self.kind, p).is_ok_and(|p| {
            p.code.as_ref().map(|c| c.get().is_some()).unwrap_or(false) || p.file.as_ref().map(|f| f.get().is_some()).unwrap_or(false)
        })
    }

    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: ScriptP = params(self.kind, p)?;
        let (source, is_file) = match (&p.code, &p.file) {
            (Some(c), _) => (c.get(), false),
            (_, Some(f)) => (f.get(), true),
            _ => (None, false),
        };
        let source = source.ok_or_else(|| format!("{} 动作没有为当前平台配置脚本", self.kind))?;
        let env = context_env(cx.context);
        let cwd = cx.context.work_dir();
        let out = cx.services.shell.run_script(&ScriptRun {
            lang: self.lang(),
            source,
            is_file,
            env: &env,
            cwd: cwd.as_deref().filter(|d| d.is_dir()),
            timeout: Duration::from_millis(p.timeout_ms.unwrap_or(30_000).clamp(100, 600_000)),
        })?;
        if out.code != 0 {
            let msg = out.stderr.trim();
            let msg = if msg.is_empty() { out.stdout.trim() } else { msg };
            return Err(format!("脚本退出码 {}{}", out.code, if msg.is_empty() { String::new() } else { format!("：{}", truncate(msg, 300)) }));
        }
        if p.notify_output && !out.stdout.trim().is_empty() {
            cx.services.shell.notify("anyTools", truncate(out.stdout.trim(), 300));
        }
        Ok(())
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use anytools_core::{AppInfo, Selection};
    use anytools_platform::mock::Call;
    use std::path::PathBuf;

    #[test]
    fn validates_code_xor_file() {
        let (reg, _) = setup();
        assert!(reg.validate(&spec("{ type: shell, code: 'echo hi' }")).is_ok());
        assert!(reg.validate(&spec("{ type: shell }")).is_err());
        assert!(reg.validate(&spec("{ type: shell, code: a, file: b }")).is_err());
    }

    #[test]
    fn nonzero_exit_is_an_error_with_stderr() {
        let (reg, mock) = setup();
        mock.state.lock().unwrap().script_output = anytools_core::platform::ScriptOutput { code: 2, stdout: "".into(), stderr: "boom\n".into() };
        let err = run(&reg, &mock, &Context::default(), "{ type: shell, code: 'false' }").unwrap_err();
        assert_eq!(err, "脚本退出码 2：boom");
        assert_eq!(mock.calls(), [Call::Script(ScriptLang::Shell, "false".into())]);
    }

    #[test]
    fn env_carries_context() {
        let ctx = Context {
            app: Some(AppInfo { id: "com.apple.finder".into(), name: "Finder".into(), pid: 1, title: None }),
            selection: Selection::Files { paths: vec![PathBuf::from("/a/x"), PathBuf::from("/a/y")] },
            selection_known: true,
            ..Default::default()
        };
        let env: std::collections::HashMap<_, _> = context_env(&ctx).into_iter().collect();
        assert_eq!(env["ANYTOOLS_APP"], "com.apple.finder");
        assert_eq!(env["ANYTOOLS_SELECTION"], "/a/x\n/a/y");
        assert_eq!(env["ANYTOOLS_CWD"], "/a", "falls back to the selection's parent");
    }

    #[test]
    fn applescript_only_on_mac() {
        let (reg, _) = setup();
        let s = spec("{ type: applescript, code: 'beep' }");
        assert_eq!(reg.available(&s, &Context::default()), cfg!(target_os = "macos"));
    }
}
