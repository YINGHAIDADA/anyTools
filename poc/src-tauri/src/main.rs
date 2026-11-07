// Prevents an extra console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod wheel;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("anyTools M0 PoC currently implements macOS only. The Windows half of M0 runs on a Windows machine.");
}

#[cfg(target_os = "macos")]
fn main() {
    app::run();
}

#[cfg(target_os = "macos")]
mod app {
    use crate::mac::{self, ShowStrategy};
    use crate::wheel::{self, Action, Cmd, Controller, FileCtx, Pt, ScreenInfo, GEOM};
    use objc2::MainThreadMarker;
    use serde::Serialize;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

    /// Shared by the tap thread (hot path), the main thread and IPC commands.
    struct Shared {
        ctl: Mutex<Controller>,
        screens: Mutex<Vec<ScreenInfo>>,
        cfg: Mutex<Cfg>,
        log: Mutex<Log>,
        probe_tx: Mutex<Option<Sender<ProbeReq>>>,
        overlay_ready: std::sync::atomic::AtomicBool,
    }

    #[derive(Clone, Serialize, serde::Deserialize)]
    struct Cfg {
        material: String,
        strategy: ShowStrategy,
        probe: bool,
        /// Overlay-side glow renderer: "css" or "webgl".
        glow: String,
    }

    struct ProbeReq {
        seq: u64,
        t0: Instant,
    }

    #[derive(Default)]
    struct Log {
        events: VecDeque<serde_json::Value>,
        file: Option<std::fs::File>,
    }

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static SELFTEST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

    fn shared() -> &'static Arc<Shared> {
        SHARED.get().expect("shared state initialised in setup")
    }

    fn ms(d: Duration) -> f64 {
        (d.as_secs_f64() * 1e5).round() / 100.0
    }

    /// Appends a metric event: kept in memory for the panel and written as JSONL for later analysis.
    fn record(kind: &str, mut v: serde_json::Value) {
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        v["kind"] = json!(kind);
        v["ts"] = json!(ts);
        if SELFTEST.load(std::sync::atomic::Ordering::Relaxed) {
            v["selftest"] = json!(true);
        }
        {
            let mut log = shared().log.lock().unwrap();
            if let Some(f) = log.file.as_mut() {
                let _ = writeln!(f, "{v}");
            }
            log.events.push_back(v.clone());
            while log.events.len() > 2000 {
                log.events.pop_front();
            }
        }
        if let Some(app) = APP.get() {
            let _ = app.emit_to("panel", "metric", v);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Input path
    // -----------------------------------------------------------------------------------------

    fn on_tap(ev: mac::TapEvent, now: Instant) -> bool {
        let sh = shared();
        if !sh.overlay_ready.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        let mut out = Vec::new();
        let swallow = {
            let screens = sh.screens.lock().unwrap();
            let placer = |pt: Pt| wheel::place(&screens, pt);
            sh.ctl.lock().unwrap().on_input(ev.input, now, &placer, &mut out)
        };
        let hook_us = now.elapsed().as_secs_f64() * 1e6;
        for cmd in out {
            if let Cmd::Show { seq, .. } = &cmd {
                record("trigger", json!({ "seq": seq, "hook_us": (hook_us * 10.0).round() / 10.0, "hid_to_tap_ms": ev.hid_to_tap_ms }));
            }
            dispatch(cmd);
        }
        swallow
    }

    fn dispatch(cmd: Cmd) {
        let Some(app) = APP.get().cloned() else { return };
        match cmd {
            Cmd::Show { seq, t0, frame, wheel: wid } => {
                let n = wheel::wheel(wid).items.len();
                let app2 = app.clone();
                let _ = app.run_on_main_thread(move || {
                    let mtm = MainThreadMarker::new().expect("main thread");
                    let before = mac::app_is_active(mtm);
                    let queued_ms = ms(t0.elapsed());
                    // Kick the webview first (its work is async in the web process), then reveal the native window.
                    // The default wheel is already rendered, so the first visible frame does not wait for the webview.
                    let _ = app2.emit_to("overlay", "wheel:show", json!({ "seq": seq, "wheel": wid, "frame": frame }));
                    mac::overlay_show(mtm, seq, frame, n);
                    let main_ms = ms(t0.elapsed());
                    let strategy = shared().cfg.lock().unwrap().strategy;
                    record("shown", json!({ "seq": seq, "queued_ms": queued_ms, "native_ms": main_ms, "we_were_active": before, "strategy": strategy, "hz": frame.hz }));
                    // The app must never become active: that is the whole point of the non-activating overlay.
                    let app3 = app2.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = app3.run_on_main_thread(move || {
                            let mtm = MainThreadMarker::new().expect("main thread");
                            let front = mac::frontmost_app();
                            record("focus_check", json!({
                                "seq": seq,
                                "anytools_active": mac::app_is_active(mtm),
                                "frontmost": front.map(|a| a.bundle),
                            }));
                        });
                    });
                });
                // Front app + Finder probe run off the main thread.
                let probe_on = shared().cfg.lock().unwrap().probe;
                std::thread::spawn(move || {
                    let t = Instant::now();
                    let front = mac::frontmost_app();
                    let front_ms = ms(t.elapsed());
                    let is_finder = front.as_ref().is_some_and(|a| a.bundle == "com.apple.finder");
                    if let Some(a) = front.clone() {
                        shared().ctl.lock().unwrap().set_front(seq, a);
                    }
                    record("front", json!({ "seq": seq, "front_ms": front_ms, "app": front }));
                    if is_finder && probe_on {
                        if let Some(tx) = shared().probe_tx.lock().unwrap().as_ref() {
                            let _ = tx.send(ProbeReq { seq, t0 });
                        }
                    }
                });
            }
            Cmd::Hover { seq, idx, angle, dist } => {
                let _ = app.emit_to("overlay", "wheel:hover", json!({ "seq": seq, "idx": idx, "angle": angle, "dist": dist }));
            }
            Cmd::Mode { seq, click } => {
                let _ = app.emit_to("overlay", "wheel:mode", json!({ "seq": seq, "click": click }));
                record("mode", json!({ "seq": seq, "click": click }));
            }
            Cmd::Cancel { seq } => {
                hide(&app, seq, false);
                record("cancel", json!({ "seq": seq }));
            }
            Cmd::Exec { seq, wheel: wid, idx, files, front } => {
                hide(&app, seq, true);
                let item = &wheel::wheel(wid).items[idx];
                let (label, action) = (item.label, item.action);
                std::thread::spawn(move || {
                    let waited = mac::wait_modifiers_released(Duration::from_millis(800));
                    let t = Instant::now();
                    let res = run_action(action, files.as_ref());
                    record("exec", json!({
                        "seq": seq,
                        "label": label,
                        "target": front.map(|a| a.bundle),
                        "waited_mod_ms": ms(waited),
                        "run_ms": ms(t.elapsed()),
                        "ok": res.is_ok(),
                        "error": res.err(),
                    }));
                });
            }
        }
    }

    fn hide(app: &AppHandle, seq: u64, flash: bool) {
        let _ = app.emit_to("overlay", "wheel:hide", json!({ "seq": seq, "flash": flash }));
        let fade_ms = if flash { 140 } else { 90 };
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            mac::overlay_hide_begin(seq, fade_ms as f64 / 1000.0);
            let app3 = app2.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(fade_ms + 20));
                let app4 = app3.clone();
                let _ = app3.run_on_main_thread(move || {
                    let ctl = shared().ctl.lock().unwrap();
                    // Only if no newer session started meanwhile.
                    if ctl.phase == wheel::Phase::Idle && ctl.seq == seq {
                        drop(ctl);
                        mac::overlay_hide_finish(seq);
                        // Now invisible: the overlay resets itself to the pre-rendered default wheel.
                        let _ = app4.emit_to("overlay", "wheel:hidden", json!({ "seq": seq }));
                    }
                });
            });
        });
    }

    fn run_action(action: Action, files: Option<&FileCtx>) -> Result<(), String> {
        let need_files = || files.filter(|f| !f.files.is_empty()).ok_or_else(|| "没有选中文件".to_string());
        let sh = |cmd: &mut std::process::Command| -> Result<(), String> {
            let out = cmd.output().map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        };
        match action {
            Action::Keys(chord) => mac::send_chord(chord),
            Action::Text(t) => {
                mac::type_text(t);
                Ok(())
            }
            Action::Launch(app) => sh(std::process::Command::new("open").args(["-a", app])),
            Action::Url(u) => sh(std::process::Command::new("open").arg(u)),
            Action::AppleScript(src) => sh(std::process::Command::new("osascript").args(["-e", src])),
            Action::TerminalHere => {
                let dir = files
                    .and_then(|f| f.dir.clone().or_else(|| f.files.first().map(|p| parent(p))))
                    .ok_or("没有当前目录")?;
                sh(std::process::Command::new("open").args(["-a", "Terminal", &dir]))
            }
            Action::Cut => {
                // PoC: macOS Finder has no real cut. Copy now; ⌥⌘V in Finder moves instead of copies.
                need_files()?;
                mac::send_chord("cmd+c")
            }
            Action::CopyPath => {
                mac::set_clipboard_text(&need_files()?.files.join("\n"));
                Ok(())
            }
            Action::Zip => {
                let f = need_files()?;
                let dir = f.dir.clone().unwrap_or_else(|| parent(&f.files[0]));
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let dest = format!("{}/Archive-{stamp}.zip", dir.trim_end_matches('/'));
                let mut cmd = std::process::Command::new("zip");
                cmd.current_dir(&dir).args(["-r", "-q", &dest]);
                for p in &f.files {
                    cmd.arg(p.trim_end_matches('/').strip_prefix(&format!("{}/", dir.trim_end_matches('/'))).unwrap_or(p));
                }
                sh(&mut cmd)
            }
            Action::Trash => trash::delete_all(&need_files()?.files).map_err(|e| e.to_string()),
            Action::OpenLab => {
                if let Some(app) = APP.get() {
                    show_panel(app);
                }
                Ok(())
            }
        }
    }

    fn parent(p: &str) -> String {
        std::path::Path::new(p.trim_end_matches('/')).parent().map(|d| d.display().to_string()).unwrap_or_else(|| "/".into())
    }

    fn show_panel(app: &AppHandle) {
        if let Some(w) = app.get_webview_window("panel") {
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
            let _ = w.show();
            let _ = w.set_focus();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Finder probe thread (NSAppleScript is used from exactly one thread)
    // -----------------------------------------------------------------------------------------

    fn start_probe_thread() -> Sender<ProbeReq> {
        let (tx, rx) = mpsc::channel::<ProbeReq>();
        std::thread::Builder::new()
            .name("finder-probe".into())
            .spawn(move || {
                let probe = match mac::FinderProbe::new() {
                    Ok(p) => p,
                    Err(e) => {
                        record("probe", json!({ "error": format!("init: {e}") }));
                        return;
                    }
                };
                while let Ok(mut req) = rx.recv() {
                    // Only the newest request matters.
                    while let Ok(newer) = rx.try_recv() {
                        req = newer;
                    }
                    let queued_ms = ms(req.t0.elapsed());
                    let t = Instant::now();
                    let res = probe.run();
                    let exec_ms = ms(t.elapsed());
                    let total_ms = ms(req.t0.elapsed());
                    let (n_files, err) = match &res {
                        Ok(c) => (c.files.len(), None),
                        Err(e) => (0, Some(e.clone())),
                    };
                    let swapped = match res {
                        Ok(ctx) => {
                            let dir = ctx.dir.clone();
                            let swap = shared().ctl.lock().unwrap().on_probe(req.seq, ctx, Instant::now());
                            if let Some(wid) = swap {
                                let app = APP.get().unwrap().clone();
                                let seq = req.seq;
                                let n = wheel::wheel(wid).items.len();
                                let _ = app.clone().run_on_main_thread(move || mac::overlay_set_shape(n));
                                let _ = app.emit_to("overlay", "wheel:swap", json!({ "seq": seq, "wheel": wid, "dir": dir }));
                            }
                            swap.is_some()
                        }
                        Err(_) => false,
                    };
                    record(
                        "probe",
                        json!({ "seq": req.seq, "queued_ms": queued_ms, "exec_ms": exec_ms, "total_ms": total_ms, "files": n_files, "swapped": swapped, "error": err }),
                    );
                }
            })
            .expect("spawn probe thread");
        tx
    }

    // -----------------------------------------------------------------------------------------
    // IPC commands
    // -----------------------------------------------------------------------------------------

    #[tauri::command]
    fn status() -> serde_json::Value {
        let cfg = shared().cfg.lock().unwrap().clone();
        let screens = shared().screens.lock().unwrap().clone();
        let log_path = log_path();
        json!({
            "accessibility": mac::accessibility_trusted(),
            "tap": mac::tap_running(),
            "cfg": cfg,
            "screens": screens,
            "log_path": log_path.map(|p| p.display().to_string()),
            "debug_build": cfg!(debug_assertions),
        })
    }

    #[tauri::command]
    fn request_accessibility() -> bool {
        mac::request_accessibility()
    }

    #[tauri::command]
    fn start_hook() -> Result<(), String> {
        if !mac::accessibility_trusted() {
            return Err("尚未授予辅助功能权限".into());
        }
        if mac::tap_running() {
            return Ok(());
        }
        if let Err(e) = mac::start_tap(
            Box::new(on_tap),
            Box::new(|why| record("tap_reenabled", json!({ "reason": why }))),
        ) {
            record("tap_error", json!({ "error": e }));
            return Err(e);
        }
        record("tap_started", json!({}));
        Ok(())
    }

    #[tauri::command]
    fn set_cfg(app: AppHandle, cfg: Cfg) {
        let prev = std::mem::replace(&mut *shared().cfg.lock().unwrap(), cfg.clone());
        let _ = app.emit_to("overlay", "cfg", cfg.clone());
        let _ = app.run_on_main_thread(move || {
            if prev.material != cfg.material {
                mac::overlay_set_material(&cfg.material);
            }
            if prev.strategy != cfg.strategy {
                mac::overlay_set_strategy(cfg.strategy);
            }
        });
    }

    #[tauri::command]
    fn events() -> Vec<serde_json::Value> {
        shared().log.lock().unwrap().events.iter().cloned().collect()
    }

    #[tauri::command]
    fn clear_events() {
        shared().log.lock().unwrap().events.clear();
    }

    /// Runs the Finder probe N times back-to-back (no wheel) to measure raw AppleEvent latency.
    #[tauri::command]
    async fn bench_probe(n: u32) -> Result<serde_json::Value, String> {
        tauri::async_runtime::spawn_blocking(move || {
            let probe = mac::FinderProbe::new()?;
            let mut samples = Vec::new();
            let mut last = None;
            for _ in 0..n.max(1) {
                let t = Instant::now();
                let r = probe.run();
                samples.push(ms(t.elapsed()));
                match r {
                    Ok(c) => last = Some(c),
                    Err(e) => return Err(e),
                }
            }
            let r = json!({ "samples": samples, "last": last });
            record("probe_bench", r.clone());
            Ok(r)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Overlay reports when its update for a session was painted (after two rAFs) plus page state.
    #[tauri::command]
    fn overlay_painted(seq: u64, kind: String, js_ms: f64, info: Option<serde_json::Value>) {
        let t0 = shared().ctl.lock().unwrap().session.as_ref().filter(|s| s.seq == seq).map(|s| s.t0);
        if let Some(t0) = t0 {
            let strategy = shared().cfg.lock().unwrap().strategy;
            record(
                "painted",
                json!({ "seq": seq, "what": kind, "since_trigger_ms": ms(t0.elapsed()), "js_ms": js_ms, "strategy": strategy, "info": info }),
            );
        }
    }

    /// All wheel definitions, so the overlay can pre-build every wheel's DOM once at startup.
    #[tauri::command]
    fn wheels() -> serde_json::Value {
        json!({
            "geom": GEOM,
            "wheels": [wheel::wheel_view(wheel::WheelId::Default), wheel::wheel_view(wheel::WheelId::Files)],
        })
    }

    /// Overlay reports frame-time statistics for one wheel session.
    #[tauri::command]
    fn overlay_frames(stats: serde_json::Value) {
        record("frames", stats);
    }

    #[tauri::command]
    fn overlay_error(r#where: String, message: String) {
        record("overlay_error", json!({ "where": r#where, "message": message }));
    }

    #[tauri::command]
    fn overlay_ready() {
        shared().overlay_ready.store(true, std::sync::atomic::Ordering::Release);
        record("overlay_ready", json!({}));
    }

    #[tauri::command]
    fn open_settings_pane(which: String) {
        let url = match which.as_str() {
            "automation" => "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
            _ => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        };
        let _ = std::process::Command::new("open").arg(url).status();
    }

    #[tauri::command]
    fn quit(app: AppHandle) {
        app.exit(0);
    }

    /// Opens and cancels the wheel `n` times per show strategy through the same controller path as the
    /// event tap, without posting any OS input events. Esc cancels, so no wheel action can ever run.
    fn run_selftest(app: AppHandle, n: u32) {
        while !shared().overlay_ready.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
        }
        SELFTEST.store(true, std::sync::atomic::Ordering::Relaxed);
        let tap = |input| {
            on_tap(mac::TapEvent { input, hid_to_tap_ms: None }, Instant::now());
        };
        for strategy in [ShowStrategy::Alpha, ShowStrategy::OrderOut] {
            let mut cfg = shared().cfg.lock().unwrap().clone();
            cfg.strategy = strategy;
            set_cfg(app.clone(), cfg);
            record("selftest_phase", json!({ "strategy": strategy, "n": n }));
            std::thread::sleep(Duration::from_secs(2));
            for _ in 0..n {
                tap(wheel::Input::TriggerDown(mac::cursor_location()));
                std::thread::sleep(Duration::from_millis(300));
                tap(wheel::Input::EscDown);
                tap(wheel::Input::EscUp);
                tap(wheel::Input::TriggerUp);
                // Long enough for WebKit to settle into its hidden-window state before the next show.
                std::thread::sleep(Duration::from_millis(2000));
            }
        }
        let mut cfg = shared().cfg.lock().unwrap().clone();
        cfg.strategy = ShowStrategy::Alpha;
        set_cfg(app, cfg);
        record("selftest_done", json!({}));
        SELFTEST.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn log_path() -> Option<std::path::PathBuf> {
        APP.get()?.path().app_log_dir().ok().map(|d| d.join("m0-metrics.jsonl"))
    }

    fn refresh_screens() {
        if let Some(mtm) = MainThreadMarker::new() {
            *shared().screens.lock().unwrap() = mac::screens(mtm);
        }
    }

    // -----------------------------------------------------------------------------------------
    // App entry
    // -----------------------------------------------------------------------------------------

    pub fn run() {
        let _ = SHARED.set(Arc::new(Shared {
            ctl: Mutex::new(Controller::default()),
            screens: Mutex::new(vec![]),
            cfg: Mutex::new(Cfg { material: "hud".into(), strategy: ShowStrategy::Alpha, probe: true, glow: "css".into() }),
            log: Mutex::new(Log::default()),
            probe_tx: Mutex::new(None),
            overlay_ready: Default::default(),
        }));

        tauri::Builder::default()
            .invoke_handler(tauri::generate_handler![
                status,
                request_accessibility,
                start_hook,
                set_cfg,
                events,
                clear_events,
                bench_probe,
                overlay_painted,
                overlay_frames,
                overlay_ready,
                open_settings_pane,
                quit,
                wheels,
                overlay_error
            ])
            .setup(|app| {
                let handle = app.handle().clone();
                let _ = APP.set(handle.clone());

                if let Some(p) = log_path() {
                    let _ = std::fs::create_dir_all(p.parent().unwrap());
                    shared().log.lock().unwrap().file =
                        std::fs::OpenOptions::new().create(true).append(true).open(&p).ok();
                }

                // Accessory: no Dock icon, never steals activation when the overlay shows.
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);

                refresh_screens();
                let overlay: WebviewWindow = app.get_webview_window("overlay").expect("overlay window");
                let ns_window = overlay.ns_window()? as usize;
                let mtm = MainThreadMarker::new().expect("setup runs on main thread");
                let cfg = shared().cfg.lock().unwrap().clone();
                mac::setup_overlay(mtm, ns_window as *mut _, &cfg.material, cfg.strategy)?;
                overlay.with_webview(|wv| {
                    let ok = mac::unlock_high_refresh(wv.inner());
                    record("high_refresh_spi", json!({ "available": ok }));
                    let occl = mac::disable_occlusion_detection(wv.inner());
                    record("occlusion_spi", json!({ "available": occl }));
                })?;

                *shared().probe_tx.lock().unwrap() = Some(start_probe_thread());

                if mac::accessibility_trusted() {
                    if let Err(e) = start_hook() {
                        record("tap_error", json!({ "error": e }));
                    }
                }

                // Screen layout can change (displays attached, resolution). Cheap enough to poll in a PoC.
                let h2 = handle.clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(Duration::from_secs(3));
                    let _ = h2.run_on_main_thread(refresh_screens);
                });

                match std::env::var("ANYTOOLS_SELFTEST").ok().and_then(|v| v.parse::<u32>().ok()) {
                    // Self-test keeps the app inactive (no panel), like real use with the panel closed.
                    Some(n) => {
                        let h3 = handle.clone();
                        std::thread::spawn(move || run_selftest(h3, n));
                    }
                    None => show_panel(&handle),
                }
                Ok(())
            })
            .on_window_event(|window, event| {
                if window.label() == "panel" {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        // Keep running in the background; Quit from the panel exits.
                        api.prevent_close();
                        let _ = window.hide();
                        let _ = window.app_handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
                    }
                }
            })
            .run(tauri::generate_context!())
            .expect("error while running anyTools PoC");
    }
}
