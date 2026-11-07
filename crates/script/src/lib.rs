//! Embedded JS/TS scripts (QuickJS) with a small, permission-checked host API.
//!
//! A script is a file under the config directory's `scripts/` folder. It runs inside an async
//! function, so `await` works at top level and `return` ends it. Each run gets a fresh runtime with
//! a time limit, memory limit and stack limit.

pub mod perms;
pub mod ts;

use anytools_core::action::params;
use anytools_core::keys::parse_chord;
use anytools_core::platform::{ScriptLang, ScriptRun};
use anytools_core::{ActionCtx, ActionHandler, Context, Params, Platform, Registry, Services};
use perms::{Grants, Perms};
use rquickjs::function::{Opt, Rest};
use rquickjs::{CatchResultExt, CaughtError, Ctx, Function, Object, Runtime, Value};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;
const MAX_HTTP_BODY: u64 = 10 * 1024 * 1024;
const MAX_SLEEP: Duration = Duration::from_secs(10);

/// Asks the user to approve a script's permissions (blocking). `true` = approved.
pub type Consent = Arc<dyn Fn(&str, &Perms) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct ScriptHost {
    /// Scripts are resolved relative to this directory and may not leave it.
    pub scripts_dir: PathBuf,
    pub grants: Arc<Grants>,
    pub consent: Consent,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub logs: Vec<String>,
    /// The script's return value, JSON-encoded (`None` for undefined).
    pub result: Option<String>,
}

impl ScriptHost {
    pub fn resolve(&self, file: &str) -> Result<PathBuf, String> {
        let p = Path::new(file);
        if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(format!("脚本路径必须是 scripts/ 下的相对路径：{file}"));
        }
        let full = self.scripts_dir.join(p);
        match full.extension().and_then(|e| e.to_str()) {
            Some("js" | "ts" | "mjs") => Ok(full),
            _ => Err(format!("只支持 .js / .ts 脚本：{file}")),
        }
    }

    /// Loads, checks permissions (asking once per script content) and runs a script.
    pub fn run_file(&self, file: &str, ctx: &Context, services: &Services, timeout: Duration) -> Result<Outcome, String> {
        let path = self.resolve(file)?;
        let source = std::fs::read_to_string(&path).map_err(|e| format!("无法读取脚本 {}: {e}", path.display()))?;
        let perms = perms::parse(&source)?;
        let hash = perms::digest(&source);
        if !self.grants.is_granted(&hash, &perms) {
            if !(self.consent)(file, &perms) {
                return Err("脚本权限未获批准，已取消运行".into());
            }
            self.grants.grant(&hash, &perms);
        }
        let wrapped = ts::wrap(&source);
        let js = if path.extension().is_some_and(|e| e == "ts") { ts::strip_types(&wrapped, &path)? } else { wrapped };
        run_wrapped(&js, file, &perms, ctx, services, timeout)
    }
}

/// Runs an already-approved script body (plain JS). Exposed for tests.
pub fn run(body: &str, name: &str, perms: &Perms, ctx: &Context, services: &Services, timeout: Duration) -> Result<Outcome, String> {
    run_wrapped(&ts::wrap(body), name, perms, ctx, services, timeout)
}

/// Runs JS produced by [`ts::wrap`] (an expression evaluating to the script's promise).
fn run_wrapped(js: &str, name: &str, perms: &Perms, ctx: &Context, services: &Services, timeout: Duration) -> Result<Outcome, String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_LIMIT);
    let deadline = Instant::now() + timeout;
    let timed_out = Arc::new(AtomicBool::new(false));
    let t2 = timed_out.clone();
    rt.set_interrupt_handler(Some(Box::new(move || {
        let over = Instant::now() >= deadline;
        if over {
            t2.store(true, Ordering::Relaxed);
        }
        over
    })));
    let qctx = rquickjs::Context::full(&rt).map_err(|e| e.to_string())?;
    let logs = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    let result = qctx.with(|cx| -> Result<Option<String>, String> {
        install_api(&cx, perms, ctx, services, logs.clone(), deadline).map_err(|e| format!("初始化脚本环境失败: {e}"))?;
        let mut opts = rquickjs::context::EvalOptions::default();
        opts.filename = Some(name.to_string());
        opts.strict = false;
        let promise: rquickjs::Promise = cx.eval_with_options(js, opts).catch(&cx).map_err(|e| describe(e, &timed_out, name))?;
        let v: Value = promise.finish().catch(&cx).map_err(|e| describe(e, &timed_out, name))?;
        if v.is_undefined() {
            Ok(None)
        } else {
            Ok(cx.json_stringify(v).ok().flatten().and_then(|s| s.to_string().ok()))
        }
    })?;
    let logs = std::mem::take(&mut *logs.lock().unwrap());
    Ok(Outcome { logs, result })
}

fn describe(e: CaughtError, timed_out: &AtomicBool, name: &str) -> String {
    if timed_out.load(Ordering::Relaxed) {
        return format!("脚本 {name} 运行超时，已终止");
    }
    match e {
        CaughtError::Exception(ex) => {
            let msg = ex.message().unwrap_or_default();
            // The wrapper adds no lines, so line numbers in the stack match the user's file.
            let at = ex.stack().and_then(|s| s.lines().find(|l| l.contains(name)).map(|l| l.trim().to_string()));
            match at {
                Some(at) => format!("{msg}（{at}）"),
                None => msg,
            }
        }
        CaughtError::Value(v) => format!("脚本抛出了 {:?}", v),
        CaughtError::Error(rquickjs::Error::Allocation) => "脚本内存超出限制（64MB）".into(),
        CaughtError::Error(e) => e.to_string(),
    }
}

// ------------------------------------------------------------------------------------------------
// Host API
// ------------------------------------------------------------------------------------------------

fn throw(cx: &Ctx, msg: impl AsRef<str>) -> rquickjs::Error {
    rquickjs::Exception::throw_message(cx, msg.as_ref())
}

fn denied(what: &str, decl: &str) -> String {
    format!("没有权限{what}（在脚本开头声明 // @permissions {decl}）")
}

fn ctx_value<'js>(cx: &Ctx<'js>, ctx: &Context) -> rquickjs::Result<Value<'js>> {
    let json = serde_json::json!({
        "app": ctx.app.as_ref().map(|a| serde_json::json!({ "id": a.id, "name": a.name, "pid": a.pid, "title": a.title })),
        "selection": ctx.selection.files().iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "text": match &ctx.selection { anytools_core::Selection::Text { text } => Some(text.as_str()), _ => None },
        "cwd": ctx.work_dir().map(|p| p.to_string_lossy().into_owned()),
        "platform": Platform::current(),
    });
    cx.json_parse(json.to_string())
}

fn fmt_args<'js>(cx: &Ctx<'js>, args: &[Value<'js>]) -> String {
    args.iter()
        .map(|v| {
            if let Some(s) = v.as_string() {
                s.to_string().unwrap_or_default()
            } else {
                cx.json_stringify(v.clone()).ok().flatten().and_then(|s| s.to_string().ok()).unwrap_or_else(|| format!("{v:?}"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn install_api<'js>(
    cx: &Ctx<'js>,
    perms: &Perms,
    ctx: &Context,
    s: &Services,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
    deadline: Instant,
) -> rquickjs::Result<()> {
    let g = cx.globals();
    g.set("context", ctx_value(cx, ctx)?)?;

    // console / log
    let console = Object::new(cx.clone())?;
    for level in ["log", "info", "warn", "error"] {
        let logs = logs.clone();
        let lvl = level.to_string();
        console.set(
            level,
            Function::new(cx.clone(), move |cx: Ctx<'js>, args: Rest<Value<'js>>| {
                let line = fmt_args(&cx, &args.0);
                tracing::info!(target: "script", level = %lvl, "{line}");
                let mut l = logs.lock().unwrap();
                if l.len() < 1000 {
                    l.push(if lvl == "log" { line } else { format!("[{lvl}] {line}") });
                }
            })?,
        )?;
    }
    g.set("console", console.clone())?;
    g.set("log", console)?;

    // sleep(ms): bounded by both MAX_SLEEP and the script deadline.
    g.set(
        "sleep",
        Function::new(cx.clone(), move |ms: f64| {
            let want = Duration::from_millis(ms.max(0.0) as u64).min(MAX_SLEEP);
            let left = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(want.min(left));
        })?,
    )?;

    // keys
    let keys = Object::new(cx.clone())?;
    let kb = s.keyboard.clone();
    keys.set(
        "send",
        Function::new(cx.clone(), move |cx: Ctx<'js>, chord: String| -> rquickjs::Result<()> {
            let c = parse_chord(&chord, Platform::current()).map_err(|e| throw(&cx, e))?;
            kb.send_chord(&c).map_err(|e| throw(&cx, e))
        })?,
    )?;
    let kb = s.keyboard.clone();
    keys.set(
        "type",
        Function::new(cx.clone(), move |cx: Ctx<'js>, text: String| -> rquickjs::Result<()> { kb.type_text(&text).map_err(|e| throw(&cx, e)) })?,
    )?;
    let (kb, f) = (s.keyboard.clone(), s.files.clone());
    keys.set(
        "paste",
        Function::new(cx.clone(), move |cx: Ctx<'js>, text: String| -> rquickjs::Result<()> {
            // Borrows the clipboard: marked transient (history skips it), restored afterwards.
            let prev = f.clipboard_snapshot();
            f.set_clipboard_text_transient(&text).map_err(|e| throw(&cx, e))?;
            let v = parse_chord("mod+v", Platform::current()).map_err(|e| throw(&cx, e))?;
            kb.send_chord(&v).map_err(|e| throw(&cx, e))?;
            std::thread::sleep(Duration::from_millis(250));
            f.clipboard_restore(&prev).map_err(|e| throw(&cx, e))
        })?,
    )?;
    g.set("keys", keys)?;

    // clipboard
    let clip = Object::new(cx.clone())?;
    let f = s.files.clone();
    clip.set("readText", Function::new(cx.clone(), move || f.clipboard_text())?)?;
    let f = s.files.clone();
    clip.set(
        "writeText",
        Function::new(cx.clone(), move |cx: Ctx<'js>, t: String| -> rquickjs::Result<()> { f.set_clipboard_text(&t).map_err(|e| throw(&cx, e)) })?,
    )?;
    let f = s.files.clone();
    clip.set(
        "readFiles",
        Function::new(cx.clone(), move || f.clipboard_files().iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>())?,
    )?;
    let f = s.files.clone();
    clip.set(
        "writeFiles",
        Function::new(cx.clone(), move |cx: Ctx<'js>, paths: Vec<String>| -> rquickjs::Result<()> {
            let p: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
            f.set_clipboard_files(&p).map_err(|e| throw(&cx, e))
        })?,
    )?;
    g.set("clipboard", clip)?;

    // app
    let app = Object::new(cx.clone())?;
    let sh = s.shell.clone();
    app.set(
        "launch",
        Function::new(cx.clone(), move |cx: Ctx<'js>, t: String| -> rquickjs::Result<()> { sh.launch(&t).map_err(|e| throw(&cx, e)) })?,
    )?;
    let sh = s.shell.clone();
    app.set(
        "open",
        Function::new(cx.clone(), move |cx: Ctx<'js>, t: String| -> rquickjs::Result<()> { sh.open(&t).map_err(|e| throw(&cx, e)) })?,
    )?;
    let probe = s.context.clone();
    app.set(
        "frontmost",
        Function::new(cx.clone(), move |cx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
            let a = probe.frontmost();
            cx.json_parse(serde_json::to_string(&a).unwrap_or_else(|_| "null".into()))
        })?,
    )?;
    g.set("app", app)?;

    // ui
    let ui = Object::new(cx.clone())?;
    let sh = s.shell.clone();
    ui.set(
        "notify",
        Function::new(cx.clone(), move |cx: Ctx<'js>, msg: Value<'js>, title: Opt<String>| {
            let text = fmt_args(&cx, &[msg]);
            sh.notify(title.0.as_deref().unwrap_or("anyTools"), &text);
        })?,
    )?;
    let sh = s.shell.clone();
    ui.set(
        "confirm",
        Function::new(cx.clone(), move |msg: String, title: Opt<String>| sh.confirm(title.0.as_deref().unwrap_or("anyTools"), &msg))?,
    )?;
    g.set("ui", ui)?;

    // fs (permission-checked)
    let fs = Object::new(cx.clone())?;
    let check = {
        let perms = perms.clone();
        move |cx: &Ctx<'js>, p: &str| -> rquickjs::Result<PathBuf> {
            let path = PathBuf::from(p);
            if !path.is_absolute() {
                return Err(throw(cx, format!("fs 需要绝对路径：{p}")));
            }
            if !perms.allows_path(&path) {
                return Err(throw(cx, denied(&format!("访问 {p}"), &format!("fs:{}", path.parent().unwrap_or(&path).display()))));
            }
            Ok(path)
        }
    };
    let c = check.clone();
    fs.set(
        "read",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String| -> rquickjs::Result<String> {
            let path = c(&cx, &p)?;
            std::fs::read_to_string(&path).map_err(|e| throw(&cx, format!("读取 {p} 失败: {e}")))
        })?,
    )?;
    let c = check.clone();
    fs.set(
        "write",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String, text: String| -> rquickjs::Result<()> {
            let path = c(&cx, &p)?;
            std::fs::write(&path, text).map_err(|e| throw(&cx, format!("写入 {p} 失败: {e}")))
        })?,
    )?;
    let c = check.clone();
    fs.set(
        "append",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String, text: String| -> rquickjs::Result<()> {
            use std::io::Write;
            let path = c(&cx, &p)?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(text.as_bytes()))
                .map_err(|e| throw(&cx, format!("追加 {p} 失败: {e}")))
        })?,
    )?;
    let c = check.clone();
    fs.set(
        "exists",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String| -> rquickjs::Result<bool> { Ok(c(&cx, &p)?.exists()) })?,
    )?;
    let c = check.clone();
    fs.set(
        "list",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String| -> rquickjs::Result<Vec<String>> {
            let path = c(&cx, &p)?;
            let mut v: Vec<String> = std::fs::read_dir(&path)
                .map_err(|e| throw(&cx, format!("列出 {p} 失败: {e}")))?
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect();
            v.sort();
            Ok(v)
        })?,
    )?;
    let c = check.clone();
    fs.set(
        "mkdir",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String| -> rquickjs::Result<()> {
            let path = c(&cx, &p)?;
            std::fs::create_dir_all(&path).map_err(|e| throw(&cx, format!("创建 {p} 失败: {e}")))
        })?,
    )?;
    let (c, files) = (check, s.files.clone());
    fs.set(
        "trash",
        Function::new(cx.clone(), move |cx: Ctx<'js>, p: String| -> rquickjs::Result<()> {
            let path = c(&cx, &p)?;
            files.trash(&[path]).map_err(|e| throw(&cx, e))
        })?,
    )?;
    g.set("fs", fs)?;

    // shell.exec (permission-checked)
    let shell = Object::new(cx.clone())?;
    let (allowed, sh, deadline2) = (perms.shell, s.shell.clone(), deadline);
    shell.set(
        "exec",
        Function::new(cx.clone(), move |cx: Ctx<'js>, cmd: String, opts: Opt<Object<'js>>| -> rquickjs::Result<Value<'js>> {
            if !allowed {
                return Err(throw(&cx, denied("运行命令", "shell")));
            }
            let cwd: Option<String> = opts.0.as_ref().and_then(|o| o.get("cwd").ok());
            let left = deadline2.saturating_duration_since(Instant::now());
            let timeout = opts.0.as_ref().and_then(|o| o.get::<_, f64>("timeout").ok()).map(|ms| Duration::from_millis(ms as u64)).unwrap_or(left).min(left);
            let lang = if Platform::current() == Platform::Win { ScriptLang::PowerShell } else { ScriptLang::Shell };
            let out = sh
                .run_script(&ScriptRun { lang, source: &cmd, is_file: false, env: &[], cwd: cwd.as_deref().map(Path::new), timeout })
                .map_err(|e| throw(&cx, e))?;
            cx.json_parse(serde_json::to_string(&out).unwrap_or_default())
        })?,
    )?;
    g.set("shell", shell)?;

    // http.fetch (permission-checked, synchronous under the hood; returns a plain object)
    let http = Object::new(cx.clone())?;
    let perms_h = perms.clone();
    http.set(
        "fetch",
        Function::new(cx.clone(), move |cx: Ctx<'js>, url: String, init: Opt<Object<'js>>| -> rquickjs::Result<Value<'js>> {
            let host = url
                .split_once("://")
                .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("").split(':').next().unwrap_or("").to_string())
                .filter(|h| !h.is_empty())
                .ok_or_else(|| throw(&cx, format!("无效的 URL：{url}")))?;
            if !url.starts_with("https://") && !url.starts_with("http://") {
                return Err(throw(&cx, "只支持 http(s) 地址"));
            }
            if !perms_h.allows_host(&host) {
                return Err(throw(&cx, denied(&format!("访问 {host}"), &format!("http:{host}"))));
            }
            let get = |k: &str| init.0.as_ref().and_then(|o| o.get::<_, Option<String>>(k).ok().flatten());
            let method = get("method").unwrap_or_else(|| "GET".into()).to_ascii_uppercase();
            let body = get("body");
            let headers: Vec<(String, String)> = init
                .0
                .as_ref()
                .and_then(|o| o.get::<_, Option<Object>>("headers").ok().flatten())
                .map(|h| h.props::<String, String>().filter_map(|r| r.ok()).collect())
                .unwrap_or_default();
            let left = deadline.saturating_duration_since(Instant::now());
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(left.min(Duration::from_secs(30))))
                .http_status_as_error(false)
                .build()
                .into();
            let res = match method.as_str() {
                "GET" | "DELETE" | "HEAD" => {
                    let mut r = match method.as_str() {
                        "DELETE" => agent.delete(&url),
                        "HEAD" => agent.head(&url),
                        _ => agent.get(&url),
                    };
                    for (k, v) in &headers {
                        r = r.header(k, v);
                    }
                    r.call()
                }
                "POST" | "PUT" | "PATCH" => {
                    let mut r = match method.as_str() {
                        "PUT" => agent.put(&url),
                        "PATCH" => agent.patch(&url),
                        _ => agent.post(&url),
                    };
                    for (k, v) in &headers {
                        r = r.header(k, v);
                    }
                    r.send(body.unwrap_or_default().as_bytes())
                }
                m => return Err(throw(&cx, format!("不支持的请求方法 {m}"))),
            };
            let mut res = res.map_err(|e| throw(&cx, format!("请求失败: {e}")))?;
            let status = res.status().as_u16();
            let text = res.body_mut().with_config().limit(MAX_HTTP_BODY).read_to_string().map_err(|e| throw(&cx, format!("读取响应失败: {e}")))?;
            let json = serde_json::json!({ "status": status, "ok": (200..300).contains(&status), "text": text });
            let v = cx.json_parse(json.to_string())?;
            // Convenience: res.json() parses the body.
            if let Some(o) = v.as_object() {
                let body = text_json(&cx, o)?;
                o.set("json", body)?;
            }
            Ok(v)
        })?,
    )?;
    g.set("http", http)?;
    Ok(())
}

fn text_json<'js>(cx: &Ctx<'js>, res: &Object<'js>) -> rquickjs::Result<Function<'js>> {
    let text: String = res.get("text")?;
    Function::new(cx.clone(), move |cx: Ctx<'js>| -> rquickjs::Result<Value<'js>> { cx.json_parse(text.clone()) })
}

// ------------------------------------------------------------------------------------------------
// The `script` action
// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptP {
    file: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct ScriptAction {
    pub host: ScriptHost,
}

impl ActionHandler for ScriptAction {
    fn kind(&self) -> &'static str {
        "script"
    }

    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        let p: ScriptP = params(self.kind(), p)?;
        self.host.resolve(&p.file).map(|_| ())
    }

    fn available(&self, p: &Params, _: &Context) -> bool {
        params::<ScriptP>(self.kind(), p).ok().and_then(|p| self.host.resolve(&p.file).ok()).is_some_and(|f| f.is_file())
    }

    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: ScriptP = params(self.kind(), p)?;
        let timeout = p.timeout_ms.map(Duration::from_millis).unwrap_or(DEFAULT_TIMEOUT).clamp(Duration::from_millis(100), Duration::from_secs(600));
        let out = self.host.run_file(&p.file, cx.context, cx.services, timeout)?;
        tracing::info!(file = %p.file, logs = out.logs.len(), "script finished");
        Ok(())
    }
}

/// TypeScript declarations for the API, written next to user scripts for editor completion.
pub const DTS: &str = include_str!("anytools.d.ts");

#[cfg(test)]
mod tests;
