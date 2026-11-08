//! The Tauri implementation of [`anytools_core::Ui`]: the overlay window and the settings window.

use anytools_core::{OverlayCmd, Ui};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub struct TauriUi {
    pub app: AppHandle,
}

pub const SETTINGS: &str = "settings";
pub const OVERLAY: &str = "overlay";

/// The native material behind the settings page. macOS uses a vibrancy sidebar; Windows uses Mica, which
/// follows the system light/dark setting the same way the page does.
fn settings_effects() -> tauri::utils::config::WindowEffectsConfig {
    tauri::utils::config::WindowEffectsConfig {
        effects: vec![
            #[cfg(target_os = "macos")]
            tauri::window::Effect::Sidebar,
            #[cfg(windows)]
            tauri::window::Effect::Mica,
        ],
        state: Some(tauri::window::EffectState::FollowsWindowActiveState),
        radius: None,
        color: None,
        interactive: false,
    }
}

/// Opaque fallback where no window material is available, matched to the page's own background.
/// On Windows a non-zero alpha is drawn fully opaque and covers the transparent window.
#[cfg(not(target_os = "macos"))]
fn settings_background() -> tauri::utils::config::Color {
    tauri::utils::config::Color(30, 30, 34, 255)
}

/// Mica needs Windows 11 (build 22000). Checked through `RtlGetVersion`, which reports the real version
/// instead of the compatibility-shimmed one `GetVersionEx` returns.
#[cfg(windows)]
fn windows_mica_available() -> bool {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo { size: 0, major: 0, minor: 0, build: 0, platform: 0, csd: [0; 128] };
    info.size = std::mem::size_of::<OsVersionInfo>() as u32;
    unsafe { RtlGetVersion(&mut info) == 0 && info.build >= 22000 }
}

impl TauriUi {
    pub fn show_settings(app: &AppHandle) {
        #[cfg(target_os = "macos")]
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        if let Some(w) = app.get_webview_window(SETTINGS) {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            return;
        }
        let built = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
            .title("anyTools")
            .inner_size(880.0, 640.0)
            .min_inner_size(720.0, 520.0);
        // Overlay title bars are a macOS window style; other platforms keep the normal title bar.
        #[cfg(target_os = "macos")]
        let built = built.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
        // The page is transparent and expects a native material behind it: vibrancy on macOS, Mica on
        // Windows (it follows the system light/dark setting, like the page does). Without one the window
        // is a translucent sheet with the desktop showing through, so fall back to an opaque background.
        let built = built.transparent(true).effects(settings_effects());
        // No native window material elsewhere, so keep the window opaque instead of showing the desktop.
        #[cfg(not(any(target_os = "macos", windows)))]
        let built = built.transparent(false).background_color(settings_background());
        let built = built.build();
        match built {
            Ok(w) => {
                #[cfg(target_os = "macos")]
                if let Ok(ptr) = w.ns_window() {
                    anytools_platform::macos::overlay::prepare_settings_window(ptr);
                }
                // Mica only exists on Windows 11. Where it isn't available the effect is a no-op and the
                // transparent window shows the desktop, so paint the webview opaque. On Windows any
                // non-zero alpha is drawn fully opaque and covers the transparent window.
                #[cfg(windows)]
                if !windows_mica_available() {
                    let _ = w.set_background_color(Some(settings_background()));
                }
                let _ = w.set_focus();
            }
            Err(e) => tracing::error!(error = %e, "failed to open settings window"),
        }
    }
}

impl Ui for TauriUi {
    fn overlay(&self, cmd: OverlayCmd) {
        #[cfg(target_os = "macos")]
        {
            use anytools_platform::macos::overlay;
            let app = self.app.clone();
            match cmd {
                OverlayCmd::Show { session, frame, shape, channel, payload } => {
                    let shape = serde_json::from_value(shape).ok();
                    let _ = self.app.run_on_main_thread(move || {
                        // Tell the page first (its work is async in the web process), then reveal natively:
                        // the pre-rendered page means the first visible frame never waits for the web process.
                        if !payload.is_null() {
                            let _ = app.emit_to(OVERLAY, channel, payload);
                        }
                        overlay::show(session, frame, shape);
                    });
                }
                OverlayCmd::Shape { session, shape } => {
                    if let Ok(shape) = serde_json::from_value(shape) {
                        let _ = self.app.run_on_main_thread(move || overlay::set_shape(session, shape));
                    }
                }
                OverlayCmd::Hide { session, fade_ms } => {
                    let _ = self.app.run_on_main_thread(move || {
                        overlay::hide(session, fade_ms);
                    });
                }
            }
        }
        // Windows has no glass mask, so the window itself is cut to the wheel's outline. The page still
        // gets the same events; only the native window handling differs.
        #[cfg(windows)]
        {
            use anytools_platform::windows::overlay;
            let app = self.app.clone();
            match cmd {
                OverlayCmd::Show { session, frame, shape, channel, payload } => {
                    let _ = self.app.run_on_main_thread(move || {
                        if !payload.is_null() {
                            let _ = app.emit_to(OVERLAY, channel, payload);
                        }
                        overlay::show(session, frame, Some(shape));
                    });
                }
                OverlayCmd::Shape { session, shape } => {
                    let _ = self.app.run_on_main_thread(move || overlay::set_shape(session, shape));
                }
                OverlayCmd::Hide { session, fade_ms } => {
                    let _ = self.app.run_on_main_thread(move || {
                        overlay::hide(session, fade_ms);
                    });
                }
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = cmd;
        }
    }

    fn overlay_event(&self, channel: &str, payload: Value) {
        let _ = self.app.emit_to(OVERLAY, channel, payload);
    }

    fn settings_event(&self, channel: &str, payload: Value) {
        if self.app.get_webview_window(SETTINGS).is_some() {
            let _ = self.app.emit_to(SETTINGS, channel, payload);
        }
    }

    fn open_settings(&self) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || TauriUi::show_settings(&app));
    }
}

pub fn notify_settings(app: &AppHandle, channel: &str, payload: Value) {
    if app.get_webview_window(SETTINGS).is_some() {
        let _ = app.emit_to(SETTINGS, channel, payload);
    }
}

