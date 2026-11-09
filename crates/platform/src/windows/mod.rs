//! Windows implementation of the platform traits.
//!
//! The input hook is the part the app cannot run without: a low-level keyboard and mouse hook on its
//! own thread. The overlay window, front-app detection, clipboard and key injection back the features
//! that hook feeds.

mod context;
mod input;
pub mod overlay;
mod system;
mod util;

pub use context::refresh_screens;
pub use input::secure_input_enabled;
pub use overlay::{hide, place, set_shape, setup, show};

use anytools_core::Services;
use std::sync::Arc;

pub fn services() -> Services {
    Services {
        input: Arc::new(input::WinInput),
        keyboard: Arc::new(input::WinKeyboard),
        context: Arc::new(context::WinContext),
        shell: Arc::new(system::WinShell),
        files: Arc::new(system::WinFiles),
        perms: Arc::new(system::WinPerms),
        icons: Arc::new(system::WinIcons),
    }
}
