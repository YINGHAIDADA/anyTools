//! macOS implementation.

mod context;
mod ffi;
mod input;
pub mod overlay;
mod system;

pub use input::secure_input_enabled;

use anytools_core::Services;
use std::sync::Arc;

pub fn services() -> Services {
    Services {
        input: Arc::new(input::MacInput),
        keyboard: Arc::new(input::MacKeyboard),
        context: Arc::new(context::MacContext),
        shell: Arc::new(system::MacShell),
        files: Arc::new(system::MacFiles),
        perms: Arc::new(system::MacPerms),
        icons: Arc::new(system::MacIcons),
    }
}
