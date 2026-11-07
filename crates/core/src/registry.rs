use crate::action::{ActionCtx, ActionHandler, ActionSpec};
use crate::context::Context;
use crate::platform::Services;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// Global action registry shared by all modules.
#[derive(Default)]
pub struct Registry {
    actions: RwLock<BTreeMap<&'static str, Arc<dyn ActionHandler>>>,
}

impl Registry {
    pub fn register(&self, h: Arc<dyn ActionHandler>) {
        let kind = h.kind();
        if self.actions.write().unwrap().insert(kind, h).is_some() {
            tracing::warn!(kind, "action handler registered twice; the later one wins");
        }
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn ActionHandler>> {
        self.actions.read().unwrap().get(kind).cloned()
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.actions.read().unwrap().keys().copied().collect()
    }

    pub fn validate(&self, spec: &ActionSpec) -> Result<(), String> {
        let h = self.get(&spec.kind).ok_or_else(|| format!("未知的动作类型 \"{}\"", spec.kind))?;
        h.validate(&spec.params, self)
    }

    pub fn available(&self, spec: &ActionSpec, ctx: &Context) -> bool {
        self.get(&spec.kind).is_some_and(|h| h.available(&spec.params, ctx))
    }

    pub fn run(&self, spec: &ActionSpec, ctx: &Context, services: &Services) -> Result<(), String> {
        let h = self.get(&spec.kind).ok_or_else(|| format!("未知的动作类型 \"{}\"", spec.kind))?;
        h.run(&spec.params, &ActionCtx { context: ctx, services, registry: self })
    }
}
