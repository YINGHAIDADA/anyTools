use crate::config::ConfigFile;
use crate::host::Host;
use crate::platform::{InputEvent, Permission};
use crate::registry::Registry;
use serde_json::Value;
use std::time::Instant;

/// A pluggable feature (wheel, later clipboard history, launcher, …).
///
/// Lifecycle: `register_actions` → `validate` (for every config load) → `start` → `apply_config`
/// (initially and on every valid reload) → `on_input` / `on_overlay` while running.
pub trait Module: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn permissions(&self) -> &'static [Permission] {
        &[]
    }
    /// Contributes action handlers to the global registry.
    fn register_actions(&self, _reg: &Registry) {}
    /// Validates this module's config section without applying it.
    fn validate(&self, _cfg: &ConfigFile, _reg: &Registry) -> Result<(), String> {
        Ok(())
    }
    fn start(&self, host: Host) -> Result<(), String>;
    /// Applies an already validated config.
    fn apply_config(&self, cfg: &ConfigFile);
    /// Raw input from the global hook, on the hook thread. Must be fast. Return `true` to swallow.
    fn on_input(&self, _ev: &InputEvent, _now: Instant) -> bool {
        false
    }
    /// A message from the overlay page addressed to this module; the return value is the reply.
    fn on_overlay(&self, _msg: &Value) -> Value {
        Value::Null
    }
    /// State for the settings / status UI.
    fn status(&self) -> Value {
        Value::Null
    }
}
