use serde::Serialize;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoreEvent {
    ConfigLoaded { path: String },
    /// The config on disk is invalid; the previous valid config stays active.
    ConfigError { message: String },
    PermissionChanged,
    ActionFinished { source: String, label: String, kind: String, ok: bool, error: Option<String>, ms: f64 },
}

type Subscriber = Arc<dyn Fn(&CoreEvent) + Send + Sync>;

/// Synchronous fan-out. Subscribers must be quick; offload real work to a thread.
#[derive(Default)]
pub struct EventBus {
    subs: RwLock<Vec<Subscriber>>,
}

impl EventBus {
    pub fn subscribe(&self, f: impl Fn(&CoreEvent) + Send + Sync + 'static) {
        self.subs.write().unwrap().push(Arc::new(f));
    }

    pub fn emit(&self, ev: CoreEvent) {
        // Clone the list so a subscriber may subscribe/emit without deadlocking.
        let subs = self.subs.read().unwrap().clone();
        for s in subs {
            s(&ev);
        }
    }
}
