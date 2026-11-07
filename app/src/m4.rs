//! Distribution features: launch at login, crash reports, updates.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

// ------------------------------------------------------------------------------------------------
// Launch at login (macOS 13+: SMAppService registers the app bundle itself as a login item)
// ------------------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod login {
    use objc2_service_management::{SMAppService, SMAppServiceStatus};

    pub fn enabled() -> Result<bool, String> {
        let s = unsafe { SMAppService::mainAppService().status() };
        Ok(s == SMAppServiceStatus::Enabled)
    }

    pub fn set(on: bool) -> Result<bool, String> {
        let svc = unsafe { SMAppService::mainAppService() };
        let r = unsafe { if on { svc.registerAndReturnError() } else { svc.unregisterAndReturnError() } };
        if let Err(e) = r {
            return Err(format!("设置开机启动失败：{}", e.localizedDescription()));
        }
        let s = unsafe { svc.status() };
        if on && s == SMAppServiceStatus::RequiresApproval {
            // The user turned anyTools off in System Settings › Login Items before; only they can re-allow it.
            unsafe { SMAppService::openSystemSettingsLoginItems() };
            return Err("需要在「系统设置 › 通用 › 登录项」中允许 anyTools".into());
        }
        Ok(s == SMAppServiceStatus::Enabled)
    }
}

#[cfg(not(target_os = "macos"))]
mod login {
    pub fn enabled() -> Result<bool, String> {
        Err("当前平台暂不支持".into())
    }
    pub fn set(_on: bool) -> Result<bool, String> {
        Err("当前平台暂不支持".into())
    }
}

#[tauri::command]
pub fn autostart_get() -> Result<bool, String> {
    login::enabled()
}

#[tauri::command]
pub fn autostart_set(enabled: bool) -> Result<bool, String> {
    let r = login::set(enabled);
    tracing::info!(enabled, result = ?r, "launch at login");
    r
}

// ------------------------------------------------------------------------------------------------
// Crash reports
// ------------------------------------------------------------------------------------------------

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Writes `crash-<unix time>.log` (message, location, backtrace) for every panic, then runs the
/// default hook. Reports stay on this machine.
pub fn install_crash_handler(log_dir: &Path) {
    let _ = LOG_DIR.set(log_dir.to_path_buf());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-string panic payload)".into());
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("?").to_string();
        let bt = std::backtrace::Backtrace::force_capture();
        let report = format!(
            "anyTools {} crash report\ntime: {}\nos: {} {}\nthread: {thread}\nat: {at}\nmessage: {msg}\n\nbacktrace:\n{bt}\n",
            env!("CARGO_PKG_VERSION"),
            now_secs(),
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        tracing::error!(%thread, %at, %msg, "panic");
        if let Some(dir) = LOG_DIR.get() {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(format!("crash-{}.log", now_secs())), report);
        }
        prev(info);
    }));
}

/// Crash reports written since the last time we looked (remembered in `state_file`). Prunes to the
/// 20 newest reports.
pub fn new_crash_reports(log_dir: &Path, state_file: &Path) -> Vec<PathBuf> {
    let seen: u64 = std::fs::read_to_string(state_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    let mut reports: Vec<(u64, PathBuf)> = std::fs::read_dir(log_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let t = name.strip_prefix("crash-")?.strip_suffix(".log")?.parse().ok()?;
            Some((t, e.path()))
        })
        .collect();
    reports.sort();
    while reports.len() > 20 {
        let _ = std::fs::remove_file(&reports.remove(0).1);
    }
    let newest = reports.last().map(|r| r.0).unwrap_or(seen);
    if newest > seen {
        if let Some(dir) = state_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(state_file, newest.to_string());
    }
    reports.into_iter().filter(|r| r.0 > seen).map(|r| r.1).collect()
}

#[tauri::command]
pub fn reveal_logs() -> Result<(), String> {
    let dir = LOG_DIR.get().ok_or("日志目录未初始化")?;
    let st = std::process::Command::new(if cfg!(target_os = "macos") { "/usr/bin/open" } else { "explorer" })
        .arg(dir)
        .status()
        .map_err(|e| e.to_string())?;
    if st.success() || cfg!(windows) {
        Ok(())
    } else {
        Err("无法打开日志目录".into())
    }
}

// ------------------------------------------------------------------------------------------------
// Updates (tauri-plugin-updater; artifacts are minisign-signed, the public key is in tauri.conf.json)
// ------------------------------------------------------------------------------------------------

/// The update manifest URL is baked in at build time (CI sets it from the GitHub repository), so
/// local builds simply have updates turned off.
pub fn update_endpoint() -> Option<&'static str> {
    option_env!("ANYTOOLS_UPDATE_URL").filter(|u| u.starts_with("https://"))
}

fn updater(app: &AppHandle) -> Result<Option<tauri_plugin_updater::Updater>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let Some(url) = update_endpoint() else { return Ok(None) };
    let url = url.parse().map_err(|e| format!("更新地址无效: {e}"))?;
    let u = app.updater_builder().endpoints(vec![url]).map_err(|e| e.to_string())?.build().map_err(|e| e.to_string())?;
    Ok(Some(u))
}

#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Value, String> {
    let Some(u) = updater(&app)? else { return Ok(json!({ "configured": false, "available": false })) };
    match u.check().await.map_err(|e| format!("检查更新失败：{e}"))? {
        Some(up) => Ok(json!({ "configured": true, "available": true, "version": up.version, "notes": up.body })),
        None => Ok(json!({ "configured": true, "available": false })),
    }
}

#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let u = updater(&app)?.ok_or("此版本未配置更新服务")?;
    let up = u.check().await.map_err(|e| format!("检查更新失败：{e}"))?.ok_or("已是最新版本")?;
    tracing::info!(version = %up.version, "installing update");
    // The signature is verified by the plugin before anything is replaced.
    up.download_and_install(|_, _| {}, || {}).await.map_err(|e| format!("安装更新失败：{e}"))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_reports_are_reported_once_and_pruned() {
        let d = std::env::temp_dir().join(format!("anytools-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let state = d.join("state/seen");
        assert!(new_crash_reports(&d, &state).is_empty());
        for t in 1..=25u64 {
            std::fs::write(d.join(format!("crash-{t}.log")), "x").unwrap();
        }
        std::fs::write(d.join("anytools.log"), "not a crash").unwrap();
        let first = new_crash_reports(&d, &state);
        assert_eq!(first.len(), 20, "pruned to the 20 newest");
        assert!(!d.join("crash-1.log").exists());
        assert!(new_crash_reports(&d, &state).is_empty(), "each report is surfaced once");
        std::fs::write(d.join("crash-99.log"), "x").unwrap();
        assert_eq!(new_crash_reports(&d, &state), vec![d.join("crash-99.log")]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
