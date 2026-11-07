//! Values that may differ per platform: `keys: "mod+c"` or `keys: { mac: "cmd+c", win: "ctrl+c" }`.

use crate::context::Platform;
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Split<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub win: Option<T>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PerPlatform<T> {
    Split(Split<T>),
    Same(T),
}

/// Only a map whose keys are all `mac`/`win` is the split form. (A derived untagged enum would also
/// accept a 2-element *list* as `Split`, since serde lets structs deserialize from sequences.)
impl<'de, T: DeserializeOwned> Deserialize<'de> for PerPlatform<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        if let Value::Object(m) = &v {
            if !m.is_empty() && m.keys().all(|k| k == "mac" || k == "win") {
                let get = |k: &str| -> Result<Option<T>, D::Error> {
                    m.get(k).map(|x| serde_json::from_value(x.clone()).map_err(D::Error::custom)).transpose()
                };
                return Ok(PerPlatform::Split(Split { mac: get("mac")?, win: get("win")? }));
            }
        }
        serde_json::from_value(v).map(PerPlatform::Same).map_err(D::Error::custom)
    }
}

impl<T> PerPlatform<T> {
    pub fn get(&self) -> Option<&T> {
        self.get_for(Platform::current())
    }

    pub fn get_for(&self, p: Platform) -> Option<&T> {
        match self {
            PerPlatform::Same(v) => Some(v),
            PerPlatform::Split(s) => match p {
                Platform::Mac => s.mac.as_ref(),
                Platform::Win => s.win.as_ref(),
                Platform::Other => None,
            },
        }
    }
}

/// `"a"` or `["a", "b"]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let s: &[String] = match self {
            OneOrMany::One(s) => std::slice::from_ref(s),
            OneOrMany::Many(v) => v,
        };
        s.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_forms() {
        let same: PerPlatform<String> = serde_json::from_str(r#""mod+c""#).unwrap();
        assert_eq!(same.get_for(Platform::Mac).unwrap(), "mod+c");
        assert_eq!(same.get_for(Platform::Win).unwrap(), "mod+c");

        let split: PerPlatform<String> = serde_json::from_str(r#"{"mac":"cmd+shift+4"}"#).unwrap();
        assert_eq!(split.get_for(Platform::Mac).unwrap(), "cmd+shift+4");
        assert_eq!(split.get_for(Platform::Win), None);

        assert!(serde_json::from_str::<PerPlatform<String>>(r#"{"linux":"x"}"#).is_err());

        // A two-element list is a list, not { mac, win }.
        let list: PerPlatform<OneOrMany> = serde_json::from_str(r#"["a","b"]"#).unwrap();
        assert_eq!(list.get_for(Platform::Mac).unwrap().iter().collect::<Vec<_>>(), ["a", "b"]);
        let split: PerPlatform<OneOrMany> = serde_json::from_str(r#"{"mac":["a","b"],"win":"c"}"#).unwrap();
        assert_eq!(split.get_for(Platform::Win).unwrap().iter().collect::<Vec<_>>(), ["c"]);
    }

    #[test]
    fn one_or_many() {
        let one: OneOrMany = serde_json::from_str(r#""a""#).unwrap();
        let many: OneOrMany = serde_json::from_str(r#"["a","b"]"#).unwrap();
        assert_eq!(one.iter().collect::<Vec<_>>(), ["a"]);
        assert_eq!(many.iter().collect::<Vec<_>>(), ["a", "b"]);
    }
}
