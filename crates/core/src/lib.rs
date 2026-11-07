//! anyTools core.
//!
//! Everything here is platform-neutral. Feature modules depend on this crate and on the platform
//! *traits* in [`platform`]; concrete implementations live in `anytools-platform`.

pub mod action;
pub mod config;
pub mod context;
pub mod event;
pub mod host;
pub mod keys;
pub mod module;
pub mod per_platform;
pub mod platform;
pub mod registry;
pub mod resolver;

pub use action::{ActionCtx, ActionHandler, ActionSpec, MenuItem, Params};
pub use config::{ConfigFile, ConfigStore, Settings};
pub use context::{AppInfo, Context, Frame, Platform, Pt, Rect, ScreenInfo, Selection};
pub use event::{CoreEvent, EventBus};
pub use host::{Host, OverlayCmd, Ui};
pub use keys::{Chord, Mods};
pub use module::Module;
pub use per_platform::{OneOrMany, PerPlatform};
pub use platform::Services;
pub use registry::Registry;
pub use resolver::{When, WhenCfg};
