//! Key chords: `"mod+shift+p"`, where `mod` is ⌘ on macOS and Ctrl on Windows.

use crate::context::Platform;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Mods(pub u8);

impl Mods {
    /// ⌘ on macOS, the Windows key on Windows.
    pub const CMD: Mods = Mods(1);
    pub const CTRL: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const SHIFT: Mods = Mods(8);
    pub const NONE: Mods = Mods(0);

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Chord {
    pub mods: Mods,
    /// Normalized key name: `a`–`z`, `0`–`9`, `space`, `enter`, `tab`, `esc`, `delete`, `up`, `f1`, …
    pub key: String,
}

fn modifier(name: &str, p: Platform) -> Option<Mods> {
    Some(match name {
        "mod" => {
            if p == Platform::Mac {
                Mods::CMD
            } else {
                Mods::CTRL
            }
        }
        "cmd" | "command" | "meta" | "super" | "win" => Mods::CMD,
        "ctrl" | "control" => Mods::CTRL,
        "alt" | "opt" | "option" => Mods::ALT,
        "shift" => Mods::SHIFT,
        _ => return None,
    })
}

pub fn normalize_key(name: &str) -> String {
    match name {
        "return" => "enter".into(),
        "escape" => "esc".into(),
        "backspace" => "delete".into(),
        "arrowup" => "up".into(),
        "arrowdown" => "down".into(),
        "arrowleft" => "left".into(),
        "arrowright" => "right".into(),
        other => other.into(),
    }
}

pub fn parse_chord(s: &str, p: Platform) -> Result<Chord, String> {
    let mut mods = Mods::NONE;
    let mut key = None;
    for part in s.split('+').map(|x| x.trim().to_ascii_lowercase()) {
        if part.is_empty() {
            return Err(format!("快捷键格式错误: \"{s}\""));
        }
        if let Some(m) = modifier(&part, p) {
            mods |= m;
        } else if key.replace(normalize_key(&part)).is_some() {
            return Err(format!("快捷键只能有一个主键: \"{s}\""));
        }
    }
    let key = key.ok_or_else(|| format!("快捷键缺少主键: \"{s}\""))?;
    Ok(Chord { mods, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_is_platform_specific() {
        let c = parse_chord("mod+Shift+P", Platform::Mac).unwrap();
        assert_eq!(c, Chord { mods: Mods::CMD | Mods::SHIFT, key: "p".into() });
        let c = parse_chord("mod+shift+p", Platform::Win).unwrap();
        assert_eq!(c.mods, Mods::CTRL | Mods::SHIFT);
    }

    #[test]
    fn aliases_and_errors() {
        assert_eq!(parse_chord("option+return", Platform::Mac).unwrap(), Chord { mods: Mods::ALT, key: "enter".into() });
        assert!(parse_chord("cmd+shift", Platform::Mac).is_err());
        assert!(parse_chord("a+b", Platform::Mac).is_err());
        assert!(parse_chord("cmd++a", Platform::Mac).is_err());
    }
}
