//! Built-in actions. Every module can use them through the [`Registry`].

mod basic;
mod file;
mod script;

use anytools_core::Registry;
use std::sync::Arc;

pub fn register_all(reg: &Registry) {
    reg.register(Arc::new(basic::Keystroke));
    reg.register(Arc::new(basic::Text));
    reg.register(Arc::new(basic::Launch));
    reg.register(Arc::new(basic::Open));
    reg.register(Arc::new(basic::Sequence));
    reg.register(Arc::new(script::Script { kind: "shell" }));
    reg.register(Arc::new(script::Script { kind: "applescript" }));
    reg.register(Arc::new(script::Script { kind: "powershell" }));
    for h in file::all() {
        reg.register(h);
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use anytools_core::{ActionSpec, Context, Registry};
    use anytools_platform::mock::Mock;

    pub fn setup() -> (Registry, Mock) {
        let reg = Registry::default();
        super::register_all(&reg);
        (reg, Mock::new())
    }

    pub fn spec(yaml: &str) -> ActionSpec {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    pub fn run(reg: &Registry, mock: &Mock, ctx: &Context, yaml: &str) -> Result<(), String> {
        let s = spec(yaml);
        reg.validate(&s)?;
        reg.run(&s, ctx, &mock.services())
    }
}
