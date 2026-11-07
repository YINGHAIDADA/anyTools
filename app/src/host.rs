//! Application host: builds the core, starts modules, owns the tray, permissions and IPC.

use crate::ui::{notify_settings, TauriUi, OVERLAY, SETTINGS};
use anytools_core::platform::{InputEvent, InputFilter, PermState, Permission};
use anytools_core::{ConfigStore, CoreEvent, EventBus, Host, Module, Registry, Services};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const DEFAULT_CONFIG: &str = include_str!("../default-config.yaml");

pub struct App {
    pub host: Host,
    pub modules: Vec<Arc<dyn Module>>,
    started: AtomicBool,
}

static APP: OnceLock<Arc<App>> = OnceLock::new();

fn app() -> &'static Arc<App> {
    APP.get().expect("app initialised in setup")
}

/// Fans hook events out to modules; the first one that swallows wins.
struct ModuleFilter(Vec<Arc<dyn Module>>);

impl InputFilter for ModuleFilter {
    fn handle(&self, ev: InputEvent, now: Instant) -> bool {
        let mut swallow = false;
        for m in &self.0 {
            // Every module sees every event (so state like "key released" stays consistent).
            swallow |= m.on_input(&ev, now);
        }
        swallow
    }
}

fn perm_name(p: Permission) -> &'static str {
    match p {
        Permission::Accessibility => "accessibility",
        Permission::AutomationFinder => "automation_finder",
    }
}

fn parse_perm(s: &str) -> Result<Permission, String> {
    match s {
        "accessibility" => Ok(Permission::Accessibility),
        "automation_finder" => Ok(Permission::AutomationFinder),
        _ => Err(format!("未知权限: {s}")),
    }
}

impl App {
    fn services(&self) -> &Services {
        &self.host.services
    }

    /// Starts the input hook once Accessibility is granted. Safe to call repeatedly.
    fn try_start_hook(&self) -> Result<(), String> {
        let s = self.services();
        if s.input.running() {
            return Ok(());
        }
        if s.perms.status(Permission::Accessibility) != PermState::Granted {
            return Err("尚未授予辅助功能权限".into());
        }
        s.input.start(Arc::new(ModuleFilter(self.modules.clone())))?;
        tracing::info!("input hook started");
        Ok(())
    }

    fn status(&self) -> Value {
        let s = self.services();
        let perms: serde_json::Map<String, Value> = [Permission::Accessibility, Permission::AutomationFinder]
            .into_iter()
            .map(|p| (perm_name(p).to_string(), json!(s.perms.status(p))))
            .collect();
        #[cfg(target_os = "macos")]
        let secure_input = anytools_platform::macos::secure_input_enabled();
        #[cfg(not(target_os = "macos"))]
        let secure_input = false;
        let cfg = self.host.config.get();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": anytools_core::Platform::current(),
            "hook": s.input.running(),
            "secure_input": secure_input,
            "permissions": perms,
            "config_path": self.host.config.path().display().to_string(),
            "config_error": self.host.config.last_error(),
            "settings": cfg.settings,
            "actions": self.host.registry.kinds(),
            "modules": self.modules.iter().map(|m| json!({
                "id": m.id(),
                "name": m.name(),
                "permissions": m.permissions().iter().map(|p| perm_name(*p)).collect::<Vec<_>>(),
                "status": m.status(),
            })).collect::<Vec<_>>(),
        })
    }
}

// ------------------------------------------------------------------------------------------------
// IPC
// ------------------------------------------------------------------------------------------------

#[tauri::command]
fn status() -> Value {
    app().status()
}

#[tauri::command]
fn request_permission(name: String) -> Result<(), String> {
    let p = parse_perm(&name)?;
    app().services().perms.request(p);
    Ok(())
}

#[tauri::command]
fn open_permission_settings(name: String) -> Result<(), String> {
    let p = parse_perm(&name)?;
    app().services().perms.open_settings(p);
    Ok(())
}

#[tauri::command]
fn start_hook() -> Result<(), String> {
    app().try_start_hook()
}

#[tauri::command]
fn reload_config() -> Result<(), String> {
    app().host.config.reload().map(|_| ())
}

#[tauri::command]
fn open_config() -> Result<(), String> {
    open_in_editor(app().host.config.path())
}

#[tauri::command]
fn reveal_config() -> Result<(), String> {
    reveal(app().host.config.path())
}

#[tauri::command]
fn read_config() -> Result<String, String> {
    std::fs::read_to_string(app().host.config.path()).map_err(|e| e.to_string())
}

/// The current (valid) config as JSON, for the settings editor.
#[tauri::command]
fn get_config() -> Value {
    json!(*app().host.config.get())
}

fn parse_cfg(cfg: Value) -> Result<anytools_core::ConfigFile, String> {
    serde_json::from_value(cfg).map_err(|e| format!("配置格式错误: {e}"))
}

/// Validates an edited config without saving (same checks as loading the file).
#[tauri::command]
fn validate_config(cfg: Value) -> Result<(), String> {
    let cfg = parse_cfg(cfg)?;
    let a = app();
    for m in &a.modules {
        m.validate(&cfg, &a.host.registry).map_err(|e| format!("[{}] {e}", m.name()))?;
    }
    Ok(())
}

#[tauri::command]
fn save_config(cfg: Value) -> Result<(), String> {
    app().host.config.save(parse_cfg(cfg)?).map(|_| ())
}

#[tauri::command]
fn list_scripts() -> Vec<String> {
    let Some(dir) = SCRIPTS_DIR.get() else { return vec![] };
    let mut out = vec![];
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(base, &p, out);
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("js" | "ts" | "mjs")) && !p.to_string_lossy().ends_with(".d.ts") {
                if let Ok(rel) = p.strip_prefix(base) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[tauri::command]
fn reveal_scripts() -> Result<(), String> {
    let dir = SCRIPTS_DIR.get().ok_or("scripts 目录未初始化")?;
    app().services().shell.open(&dir.to_string_lossy())
}

/// Lists installed applications (for the "launch app" picker).
#[tauri::command]
fn list_apps() -> Vec<Value> {
    let mut out = vec![];
    #[cfg(target_os = "macos")]
    {
        let mut dirs = vec![std::path::PathBuf::from("/Applications"), std::path::PathBuf::from("/System/Applications"), std::path::PathBuf::from("/System/Applications/Utilities"), std::path::PathBuf::from("/Applications/Utilities")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(std::path::PathBuf::from(home).join("Applications"));
        }
        for d in dirs {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "app") {
                    let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let id = std::fs::read(p.join("Contents/Info.plist")).ok().and_then(|b| bundle_id_from_plist(&b));
                    out.push(json!({ "name": name, "id": id, "path": p.to_string_lossy() }));
                }
            }
        }
    }
    out.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
    out.dedup_by(|a, b| a["id"] == b["id"] && !a["id"].is_null());
    out
}

/// Pulls CFBundleIdentifier out of an XML Info.plist (binary plists are skipped; the name still works).
#[cfg(target_os = "macos")]
fn bundle_id_from_plist(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let after = text.split("<key>CFBundleIdentifier</key>").nth(1)?;
    let start = after.find("<string>")? + 8;
    let end = after[start..].find("</string>")?;
    Some(after[start..start + end].trim().to_string())
}

/// A message from the overlay page to a module; returns the module's reply.
#[tauri::command]
fn module_call(module: String, msg: Value) -> Value {
    app().modules.iter().find(|m| m.id() == module).map(|m| m.on_overlay(&msg)).unwrap_or(Value::Null)
}

/// Overlay-side timing (first paint, frame stats), logged for diagnostics.
#[tauri::command]
fn overlay_metric(kind: String, data: Value) {
    tracing::info!(target: "metrics", kind, %data, "overlay");
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

fn open_in_editor(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // -t: the default *text* editor (a .yaml would otherwise often open in Xcode).
        let st = std::process::Command::new("/usr/bin/open").arg("-t").arg(path).status().map_err(|e| e.to_string())?;
        if st.success() { Ok(()) } else { Err("无法打开配置文件".into()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        app().services().shell.open(&path.to_string_lossy())
    }
}

fn reveal(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let st = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).status().map_err(|e| e.to_string())?;
        if st.success() { Ok(()) } else { Err("无法显示配置文件".into()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        app().services().shell.open(&path.parent().unwrap_or(path).to_string_lossy())
    }
}

// ------------------------------------------------------------------------------------------------
// Tray
// ------------------------------------------------------------------------------------------------

struct Tray {
    status: MenuItem<tauri::Wry>,
    last: std::sync::Mutex<String>,
}

static TRAY: OnceLock<Tray> = OnceLock::new();

fn status_line(a: &App) -> String {
    let s = a.services();
    if s.perms.status(Permission::Accessibility) != PermState::Granted {
        return "⚠︎ 需要辅助功能权限".into();
    }
    if !s.input.running() {
        return "⚠︎ 快捷键未启用".into();
    }
    #[cfg(target_os = "macos")]
    if anytools_platform::macos::secure_input_enabled() {
        return "⚠︎ 密码输入中，快捷键暂停".into();
    }
    if a.host.config.last_error().is_some() {
        return "⚠︎ 配置文件有错误".into();
    }
    "✓ 运行中".into()
}

fn refresh_tray() {
    let (Some(t), Some(a)) = (TRAY.get(), APP.get()) else { return };
    let line = status_line(a);
    let mut last = t.last.lock().unwrap();
    if *last != line {
        let _ = t.status.set_text(&line);
        *last = line;
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "…", false, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "settings", "设置…", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(app, "edit_config", "编辑配置文件", true, None::<&str>)?,
            &MenuItem::with_id(app, "reload_config", "重新加载配置", true, None::<&str>)?,
            &MenuItem::with_id(app, "reveal_config", "在访达中显示配置文件", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "退出 anyTools", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("anyTools")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "settings" => TauriUi::show_settings(app),
            "edit_config" => report(open_config()),
            "reload_config" => report(reload_config()),
            "reveal_config" => report(reveal_config()),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    let _ = TRAY.set(Tray { status, last: Default::default() });
    refresh_tray();
    Ok(())
}

/// `ANYTOOLS_SELFTEST=n`: opens the wheel `n` times through the same module filter the input hook
/// uses, without posting any OS input. Each round also opens a submenu by crossing the ring, then
/// leaves with Esc, so no action ever runs. Results go to the log (`target: metrics`).
fn selftest(n: u32) {
    std::thread::sleep(Duration::from_secs(3)); // let the overlay page build its wheels
    let a = app();
    let kb = &a.services().keyboard;
    let (space, esc) = (kb.code_of("space").unwrap(), kb.code_of("esc").unwrap());
    let filter = ModuleFilter(a.modules.clone());
    let send = |ev: InputEvent| filter.handle(ev, Instant::now());
    let mods = anytools_core::Mods::ALT;
    tracing::info!(target: "metrics", n, "selftest start");
    for i in 0..n {
        let c = a.services().context.cursor();
        // Where the wheel will actually be centered (pushed in from screen edges).
        let center = anytools_wheel::geometry::place(&a.services().context.screens(), c).center();
        let swallowed = send(InputEvent::KeyDown { code: space, mods, repeat: false, at: c });
        std::thread::sleep(Duration::from_millis(250));
        if i % 2 == 1 {
            // Toward 6 o'clock and across the ring: "应用" (index 4 of 8) is a submenu.
            for r in [60.0, 120.0, 175.0] {
                send(InputEvent::Move { at: anytools_core::Pt { x: center.x, y: center.y + r } });
                std::thread::sleep(Duration::from_millis(60));
            }
            std::thread::sleep(Duration::from_millis(250));
            send(InputEvent::KeyDown { code: esc, mods: Default::default(), repeat: false, at: c });
            send(InputEvent::KeyUp { code: esc, mods: Default::default() });
            std::thread::sleep(Duration::from_millis(150));
        }
        send(InputEvent::KeyDown { code: esc, mods: Default::default(), repeat: false, at: c });
        send(InputEvent::KeyUp { code: esc, mods: Default::default() });
        send(InputEvent::KeyUp { code: space, mods });
        tracing::info!(target: "metrics", round = i, swallowed, "selftest round");
        std::thread::sleep(Duration::from_millis(1500));
    }
    tracing::info!(target: "metrics", "selftest done");
}

#[cfg(target_os = "macos")]
fn apply_overlay_look(app: &AppHandle, s: &anytools_core::Settings) {
    let theme = match s.theme {
        anytools_core::config::Theme::Dark => "dark",
        anytools_core::config::Theme::Light => "light",
        anytools_core::config::Theme::Auto => "auto",
    };
    let material = s.material.clone();
    let _ = app.run_on_main_thread(move || {
        anytools_platform::macos::overlay::set_material(&material);
        anytools_platform::macos::overlay::set_theme(theme);
    });
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        app().services().shell.notify("anyTools", &e);
    }
}

// ------------------------------------------------------------------------------------------------
// Startup
// ------------------------------------------------------------------------------------------------

fn init_logging(dir: &std::path::Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    std::fs::create_dir_all(dir).ok()?;
    let file = tracing_appender::rolling::daily(dir, "anytools.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_env("ANYTOOLS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter).with(fmt::layer().with_writer(writer).with_ansi(false));
    if cfg!(debug_assertions) {
        registry.with(fmt::layer().with_writer(std::io::stderr)).init();
    } else {
        registry.init();
    }
    Some(guard)
}

static LOG_GUARD: OnceLock<Option<tracing_appender::non_blocking::WorkerGuard>> = OnceLock::new();
static SCRIPTS_DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

const EXAMPLE_SCRIPT: &str = r#"// 示例脚本：把选中文件的路径和大小复制到剪贴板。
// 在轮盘里用 { type: script, file: example.ts } 调用。脚本以 async 函数运行，可以直接 await / return。
// 需要的权限写在下面这行；第一次运行（以及每次修改后）会请你确认。
// @permissions shell

if (context.selection.length === 0) {
  ui.notify("没有选中文件");
  return;
}
const lines: string[] = [];
for (const p of context.selection) {
  const cmd = context.platform === "win" ? `(Get-Item '${p}').Length` : `stat -f%z '${p}'`;
  const size = shell.exec(cmd).stdout.trim();
  lines.push(`${p}\t${size} B`);
}
clipboard.writeText(lines.join("\n"));
ui.notify(`已复制 ${lines.length} 个文件的信息`);
"#;

/// Prepares `scripts/` (with the API typings and an example) and the consent flow.
fn script_host(config_dir: &std::path::Path, services: &Services) -> anytools_script::ScriptHost {
    let dir = config_dir.join("scripts");
    let _ = std::fs::create_dir_all(&dir);
    // Always refresh the typings so they match this version.
    let _ = std::fs::write(dir.join("anytools.d.ts"), anytools_script::DTS);
    let example = dir.join("example.ts");
    if !example.exists() {
        let _ = std::fs::write(example, EXAMPLE_SCRIPT);
    }
    let shell = services.shell.clone();
    anytools_script::ScriptHost {
        scripts_dir: dir,
        grants: Arc::new(anytools_script::perms::Grants::load(config_dir.join("state/script-grants.json"))),
        consent: Arc::new(move |file, perms| {
            let lines = perms.describe().iter().map(|l| format!("• {l}")).collect::<Vec<_>>().join("\n");
            shell.confirm(
                "允许脚本运行？",
                &format!("脚本「{file}」请求以下权限：\n\n{lines}\n\n只允许你信任的脚本。脚本内容改动后会再次询问。"),
            )
        }),
    }
}

fn setup(app_handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let paths = app_handle.path();
    let log_dir = paths.app_log_dir()?;
    let _ = LOG_GUARD.set(init_logging(&log_dir));
    crate::m4::install_crash_handler(&log_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), updates = crate::m4::update_endpoint().is_some(), "anyTools starting");

    #[cfg(target_os = "macos")]
    app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory)?;

    let services = anytools_platform::native();
    let registry = Arc::new(Registry::default());
    anytools_actions::register_all(&registry);
    let bus = Arc::new(EventBus::default());
    let config_dir = paths.app_config_dir()?;
    let config = ConfigStore::open(config_dir.join("config.yaml"), DEFAULT_CONFIG, bus.clone())?;
    registry.register(Arc::new(anytools_script::ScriptAction { host: script_host(&config_dir, &services) }));
    let _ = SCRIPTS_DIR.set(config_dir.join("scripts"));

    let modules: Vec<Arc<dyn Module>> = vec![Arc::new(anytools_wheel::WheelModule::new(services.keyboard.clone()))];
    for m in &modules {
        m.register_actions(&registry);
        let (m2, reg) = (m.clone(), registry.clone());
        config.add_validator(Box::new(move |cfg| m2.validate(cfg, &reg).map_err(|e| format!("[{}] {e}", m2.name()))));
    }
    // An invalid user config keeps the built-in default active; the error is reported below.
    let initial_error = config.reload().err();

    let ui = Arc::new(TauriUi { app: app_handle.clone() });
    let host = Host { services: services.clone(), registry: registry.clone(), bus: bus.clone(), config: config.clone(), ui };
    for m in &modules {
        m.start(host.clone()).map_err(|e| format!("模块 {} 启动失败: {e}", m.name()))?;
        m.apply_config(&config.get());
    }
    let a = Arc::new(App { host, modules, started: AtomicBool::new(false) });
    let _ = APP.set(a.clone());

    // Config reloads → modules, overlay, settings, tray.
    {
        let handle = app_handle.clone();
        bus.subscribe(move |ev| {
            let a = app();
            match ev {
                CoreEvent::ConfigLoaded { .. } => {
                    let cfg = a.host.config.get();
                    for m in &a.modules {
                        m.apply_config(&cfg);
                    }
                    a.host.ui.overlay_event("settings", json!(cfg.settings));
                    #[cfg(target_os = "macos")]
                    apply_overlay_look(&handle, &cfg.settings);
                    if a.started.load(Ordering::Relaxed) {
                        a.services().shell.notify("anyTools", "配置已更新");
                    }
                }
                CoreEvent::ConfigError { message } => {
                    a.services().shell.notify("anyTools 配置文件有误（未生效）", message);
                }
                CoreEvent::ActionFinished { .. } | CoreEvent::PermissionChanged => {}
            }
            notify_settings(&handle, "core", json!(ev));
            refresh_tray();
        });
    }

    // Overlay window → native glass popup.
    #[cfg(target_os = "macos")]
    {
        let overlay = app_handle.get_webview_window(OVERLAY).ok_or("overlay window missing")?;
        let ns_window = overlay.ns_window()? as usize;
        let material = config.get().settings.material.clone();
        let settings = config.get().settings.clone();
        let handle = app_handle.clone();
        overlay.with_webview(move |wv| {
            let mtm = objc2::MainThreadMarker::new().expect("webview callbacks run on the main thread");
            match anytools_platform::macos::overlay::setup(mtm, ns_window as *mut _, wv.inner(), &material) {
                Ok(spi) => tracing::info!(occlusion_spi = spi, "overlay ready"),
                Err(e) => tracing::error!(error = %e, "overlay setup failed"),
            }
            apply_overlay_look(&handle, &settings);
        })?;
    }

    config.watch()?;
    build_tray(app_handle)?;

    if let Some(n) = std::env::var("ANYTOOLS_SELFTEST").ok().and_then(|v| v.parse::<u32>().ok()) {
        std::thread::spawn(move || selftest(n));
        a.started.store(true, Ordering::Relaxed);
        return Ok(());
    }

    let needs_onboarding = a.try_start_hook().is_err();
    if needs_onboarding || initial_error.is_some() {
        TauriUi::show_settings(app_handle);
    }
    if let Some(e) = initial_error {
        a.services().shell.notify("anyTools 配置文件有误，已使用默认配置", &e);
    }
    let crashes = crate::m4::new_crash_reports(&log_dir, &config_dir.join("state/crash-seen"));
    if !crashes.is_empty() {
        tracing::warn!(n = crashes.len(), "crash reports since last start");
        a.services().shell.notify("anyTools 上次异常退出", "崩溃记录已保存在日志目录（设置 › 关于 › 打开日志目录）");
    }

    // Background: start the hook as soon as permission is granted; keep screens and tray fresh.
    let handle = app_handle.clone();
    std::thread::Builder::new().name("housekeeping".into()).spawn(move || {
        let mut hook_was = app().services().input.running();
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            let a = app();
            if !a.services().input.running() {
                let _ = a.try_start_hook();
            }
            let hook = a.services().input.running();
            if hook != hook_was {
                hook_was = hook;
                a.host.bus.emit(CoreEvent::PermissionChanged);
            }
            #[cfg(target_os = "macos")]
            let _ = handle.run_on_main_thread(|| {
                if let Some(mtm) = objc2::MainThreadMarker::new() {
                    anytools_platform::macos::overlay::refresh_screens(mtm);
                }
            });
            refresh_tray();
        }
    })?;
    #[cfg(not(target_os = "macos"))]
    let _ = handle;

    a.started.store(true, Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anytools_core::{ConfigFile, Platform};

    /// The config users get on first run must be valid on every platform.
    /// What the settings editor does: load the config as JSON, edit it, send it back. The JSON
    /// round trip must be lossless, or saving from the editor would silently change the config.
    #[test]
    fn editor_json_round_trip_is_lossless() {
        let cfg = ConfigFile::parse(DEFAULT_CONFIG).unwrap();
        let json = serde_json::to_value(&cfg).unwrap();
        let back: ConfigFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, cfg);
        // And through YAML (what save() writes) as well.
        let yaml = serde_yaml_ng::to_string(&back).unwrap();
        assert_eq!(ConfigFile::parse(&yaml).unwrap(), cfg);
        // An edit as the editor makes it: rename a wheel item, add a profile.
        let mut edited = json;
        edited["modules"]["wheel"]["wheels"]["default"]["items"][0]["label"] = serde_json::json!("复制（改）");
        edited["modules"]["wheel"]["profiles"].as_array_mut().unwrap().push(serde_json::json!({ "when": { "app": "com.apple.Safari" }, "wheel": "browser" }));
        let e: ConfigFile = serde_json::from_value(edited).unwrap();
        let reg = Registry::default();
        anytools_actions::register_all(&reg);
        let kb = anytools_platform::mock::Mock::new();
        let c = anytools_wheel::config::compile_for(&e.module("wheel"), &reg, &kb, Platform::Mac).unwrap();
        assert_eq!(c.wheels[c.fallback].items[0].label, "复制（改）");
        assert_eq!(c.profiles.len(), 5);
    }

    /// `cargo test -p anytools dump_default_config -- --ignored` writes the default config as the
    /// editor receives it (for previewing the settings UI in a plain browser).
    #[test]
    #[ignore]
    fn dump_default_config() {
        let cfg = ConfigFile::parse(DEFAULT_CONFIG).unwrap();
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/.preview/config.json");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    }

    #[test]
    fn default_config_is_valid() {
        let cfg = ConfigFile::parse(DEFAULT_CONFIG).unwrap();
        let reg = Registry::default();
        anytools_actions::register_all(&reg);
        let kb = anytools_platform::mock::Mock::new();
        for p in [Platform::Mac, Platform::Win] {
            let c = anytools_wheel::config::compile_for(&cfg.module("wheel"), &reg, &kb, p)
                .unwrap_or_else(|e| panic!("default config invalid on {p:?}: {e}"));
            assert_eq!(c.triggers.len(), 1, "{p:?}: one trigger");
            assert_eq!(c.wheels[c.fallback].id, "default");
        }
        // The real keyboard too (key names must map to real key codes).
        #[cfg(target_os = "macos")]
        {
            let native = anytools_platform::native();
            anytools_wheel::config::compile(&cfg.module("wheel"), &reg, native.keyboard.as_ref()).unwrap();
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| TauriUi::show_settings(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            status,
            request_permission,
            open_permission_settings,
            start_hook,
            reload_config,
            open_config,
            reveal_config,
            read_config,
            get_config,
            validate_config,
            save_config,
            list_scripts,
            reveal_scripts,
            list_apps,
            crate::m4::autostart_get,
            crate::m4::autostart_set,
            crate::m4::reveal_logs,
            crate::m4::update_check,
            crate::m4::update_install,
            module_call,
            overlay_metric,
            quit
        ])
        .setup(|app| {
            setup(app.handle()).map_err(|e| {
                tracing::error!(error = %e, "startup failed");
                e
            })
        })
        .on_window_event(|window, event| {
            if window.label() == SETTINGS {
                if let tauri::WindowEvent::Destroyed = event {
                    // Back to a menu-bar-only app once the settings window is gone.
                    #[cfg(target_os = "macos")]
                    let _ = window.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building anyTools")
        .run(|_app, event| {
            // Closing the settings window must not quit a menu-bar app.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}
