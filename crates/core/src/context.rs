use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Mac,
    Win,
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Win
        } else {
            Platform::Other
        }
    }
}

/// A point in global screen coordinates (logical points, top-left origin, y down).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x < self.x + self.w && p.y >= self.y && p.y < self.y + self.h
    }
}

/// A display, in global top-left-origin coordinates.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub frame: Rect,
    /// Excludes the menu bar / dock / taskbar.
    pub visible: Rect,
    pub hz: f64,
}

/// An overlay window frame in global top-left-origin coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Refresh rate of the display the frame is on.
    pub hz: f64,
}

impl Frame {
    pub fn center(&self) -> Pt {
        Pt { x: self.x + self.w / 2.0, y: self.y + self.h / 2.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    /// Bundle id on macOS (`com.apple.finder`), executable name on Windows (`explorer.exe`).
    pub id: String,
    pub name: String,
    pub pid: i32,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Selection {
    #[default]
    None,
    Files {
        paths: Vec<PathBuf>,
    },
    Text {
        text: String,
    },
}

impl Selection {
    pub fn files(&self) -> &[PathBuf] {
        match self {
            Selection::Files { paths } => paths,
            _ => &[],
        }
    }
}

/// What the user was doing when a feature was triggered.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Context {
    pub app: Option<AppInfo>,
    pub selection: Selection,
    /// False until the (asynchronous) selection probe finished. Rules that depend on the selection
    /// never match while it is unknown.
    pub selection_known: bool,
    /// The file manager's current folder, when known.
    pub cwd: Option<PathBuf>,
    pub cursor: Pt,
}

impl Context {
    pub fn app_id(&self) -> &str {
        self.app.as_ref().map(|a| a.id.as_str()).unwrap_or("")
    }

    /// The folder file actions should operate in: the file manager's folder, else the parent of the
    /// first selected item.
    pub fn work_dir(&self) -> Option<PathBuf> {
        self.cwd.clone().or_else(|| self.selection.files().first().and_then(|p| p.parent().map(|d| d.to_path_buf())))
    }
}
