//! What the host application (Tauri) provides to modules: the overlay window and app-level UI.

use crate::config::ConfigStore;
use crate::context::Frame;
use crate::event::EventBus;
use crate::platform::Services;
use crate::registry::Registry;
use serde_json::Value;
use std::sync::Arc;

/// Commands for the shared, non-activating overlay window.
#[derive(Clone, Debug)]
pub enum OverlayCmd {
    /// Positions the window and fades it in. `shape` describes the glass mask (module-defined JSON,
    /// interpreted by the platform overlay), `payload` goes to the overlay page.
    Show { session: u64, frame: Frame, shape: Value, payload: Value },
    /// Updates the glass mask (e.g. a submenu with a different number of sectors).
    Shape { session: u64, shape: Value },
    /// Fades out. `fade_ms` is the native fade duration.
    Hide { session: u64, fade_ms: u64 },
}

pub trait Ui: Send + Sync {
    fn overlay(&self, cmd: OverlayCmd);
    /// Sends an event to the overlay page (`channel` is the event name).
    fn overlay_event(&self, channel: &str, payload: Value);
    /// Sends an event to the settings window, if open.
    fn settings_event(&self, channel: &str, payload: Value);
    fn open_settings(&self);
}

/// Everything a module gets.
#[derive(Clone)]
pub struct Host {
    pub services: Services,
    pub registry: Arc<Registry>,
    pub bus: Arc<EventBus>,
    pub config: Arc<ConfigStore>,
    pub ui: Arc<dyn Ui>,
}
