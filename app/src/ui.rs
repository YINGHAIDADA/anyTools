//! The Tauri implementation of [`anytools_core::Ui`]: the overlay window and the settings window.

use anytools_core::{OverlayCmd, Ui};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub struct TauriUi {
    pub app: AppHandle,
}

pub const SETTINGS: &str = "settings";
pub const OVERLAY: &str = "overlay";

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
            .min_inner_size(720.0, 520.0)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .transparent(true)
            // Native vibrancy behind the (transparent) sidebar; the content pane paints its own background.
            .effects(tauri::utils::config::WindowEffectsConfig {
                effects: vec![tauri::window::Effect::Sidebar],
                state: Some(tauri::window::EffectState::FollowsWindowActiveState),
                radius: None,
                color: None,
                interactive: false,
            })
            .build();
        match built {
            Ok(w) => {
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
                OverlayCmd::Show { session, frame, shape, payload } => {
                    let shape = serde_json::from_value(shape).ok();
                    let _ = self.app.run_on_main_thread(move || {
                        // Tell the page first (its work is async in the web process), then reveal natively:
                        // the pre-rendered page means the first visible frame never waits for the web process.
                        let _ = app.emit_to(OVERLAY, "wheel:show", payload);
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
        #[cfg(not(target_os = "macos"))]
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

