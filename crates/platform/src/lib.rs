//! Platform implementations of the [`anytools_core::platform`] traits.

pub mod mock;

#[cfg(target_os = "macos")]
pub mod macos;

/// The services for the current OS.
#[cfg(target_os = "macos")]
pub fn native() -> anytools_core::Services {
    macos::services()
}

/// Windows is not implemented yet (see TECH_DESIGN.md §14: the Windows half of M0 comes first).
/// The mock keeps the rest of the app compiling and testable everywhere.
#[cfg(not(target_os = "macos"))]
pub fn native() -> anytools_core::Services {
    mock::Mock::new().services()
}
