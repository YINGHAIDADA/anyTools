//! Actions: things a wheel item (or any future module) can do. Handlers are registered globally in the
//! [`crate::Registry`] and addressed by `type` in config.

use crate::context::Context;
use crate::platform::Services;
use crate::registry::Registry;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type Params = Map<String, Value>;

/// `{ type: keystroke, keys: "mod+c" }` — the handler kind plus its parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: Params,
}

impl ActionSpec {
    pub fn new(kind: &str, params: Value) -> Self {
        let params = match params {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        Self { kind: kind.into(), params }
    }
}

/// Deserializes handler parameters, with a readable error.
pub fn params<T: DeserializeOwned>(kind: &str, p: &Params) -> Result<T, String> {
    serde_json::from_value(Value::Object(p.clone())).map_err(|e| format!("动作 {kind} 的参数有误: {e}"))
}

pub struct ActionCtx<'a> {
    pub context: &'a Context,
    pub services: &'a Services,
    /// For composite actions (`sequence`).
    pub registry: &'a Registry,
}

impl ActionCtx<'_> {
    pub fn run(&self, spec: &ActionSpec) -> Result<(), String> {
        self.registry.run(spec, self.context, self.services)
    }
}

pub trait ActionHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Checked when the config loads, so mistakes surface before the user triggers anything.
    fn validate(&self, _params: &Params, _registry: &Registry) -> Result<(), String> {
        Ok(())
    }
    /// Whether the action makes sense in this context; unavailable items are shown greyed out.
    fn available(&self, _params: &Params, _ctx: &Context) -> bool {
        true
    }
    /// Runs on a worker thread, after the trigger modifiers were released.
    fn run(&self, params: &Params, cx: &ActionCtx) -> Result<(), String>;
}
