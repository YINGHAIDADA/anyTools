//! Platform implementations of the [`anytools_core::platform`] traits.

pub mod mock;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;

/// The services for the current OS.
#[cfg(target_os = "macos")]
pub fn native() -> anytools_core::Services {
    macos::services()
}

#[cfg(windows)]
pub fn native() -> anytools_core::Services {
    windows::services()
}

/// Platforms without an implementation yet. The mock keeps the rest of the app compiling and testable.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn native() -> anytools_core::Services {
    mock::Mock::new().services()
}
