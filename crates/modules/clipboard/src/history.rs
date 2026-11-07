//! The clipboard history: text entries, newest first, persisted as JSON.
//!
//! `History` itself never touches the disk (it sits behind a lock the input hook also takes); callers
//! take a [`History::snapshot`] and write it with [`save`] elsewhere.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Larger copies are not recorded (they'd bloat the file and are rarely pasted again from history).
pub const MAX_ENTRY_BYTES: usize = 256 * 1024;
/// Search looks at this many leading characters of each entry (keeps typing instant).
const SEARCH_CHARS: usize = 4000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub text: String,
    /// Unix time in milliseconds of the (latest) copy.
    pub ts: i64,
    /// Name of the app it was copied in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    next_id: u64,
    entries: Vec<Entry>,
}

#[derive(Debug)]
pub struct History {
    entries: Vec<Entry>,
    /// Lowercased leading text of each entry, for search (same order as `entries`).
    keys: Vec<String>,
    next_id: u64,
    max: usize,
}

fn search_key(text: &str) -> String {
    text.chars().take(SEARCH_CHARS).collect::<String>().to_lowercase()
}

impl History {
    pub fn new(max: usize) -> Self {
        History { entries: vec![], keys: vec![], next_id: 1, max: max.max(1) }
    }

    /// Reads `path`; a missing or unreadable file gives an empty history.
    pub fn load(path: &Path, max: usize) -> Self {
        let mut h = History::new(max);
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Snapshot>(&bytes) {
                Ok(f) => {
                    h.entries = f.entries;
                    h.entries.truncate(h.max);
                    h.next_id = f.next_id.max(h.entries.iter().map(|e| e.id + 1).max().unwrap_or(1));
                    h.keys = h.entries.iter().map(|e| search_key(&e.text)).collect();
                }
                Err(e) => tracing::warn!(error = %e, path = %path.display(), "clipboard: history file unreadable, starting empty"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(error = %e, "clipboard: cannot read history"),
        }
        h
    }

    /// A copy of the state for [`save`] (cheap: no encoding under the caller's lock).
    pub fn snapshot(&self) -> Snapshot {
        Snapshot { version: 1, next_id: self.next_id, entries: self.entries.clone() }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Returns whether entries were dropped.
    pub fn set_max(&mut self, max: usize) -> bool {
        self.max = max.max(1);
        let over = self.entries.len() > self.max;
        self.entries.truncate(self.max);
        self.keys.truncate(self.max);
        over
    }

    /// Records a copy. An identical entry moves to the top instead of being duplicated. Returns
    /// whether anything changed.
    pub fn add(&mut self, text: String, app: Option<String>, ts: i64) -> bool {
        if text.trim().is_empty() || text.len() > MAX_ENTRY_BYTES {
            return false;
        }
        if let Some(pos) = self.entries.iter().position(|e| e.text == text) {
            let mut e = self.entries.remove(pos);
            let k = self.keys.remove(pos);
            e.ts = ts;
            if app.is_some() {
                e.app = app;
            }
            self.entries.insert(0, e);
            self.keys.insert(0, k);
        } else {
            self.keys.insert(0, search_key(&text));
            self.entries.insert(0, Entry { id: self.next_id, text, ts, app });
            self.next_id += 1;
            self.entries.truncate(self.max);
            self.keys.truncate(self.max);
        }
        true
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let Some(pos) = self.entries.iter().position(|e| e.id == id) else { return false };
        self.entries.remove(pos);
        self.keys.remove(pos);
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.keys.clear();
    }

    /// Ids of the entries matching `query`, newest first. Every whitespace-separated term must occur
    /// (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<u64> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.entries
            .iter()
            .zip(&self.keys)
            .filter(|(_, k)| terms.iter().all(|q| k.contains(q.as_str())))
            .map(|(e, _)| e.id)
            .collect()
    }
}

/// Writes a snapshot: atomically (temp file + rename) and readable by the user only, since the
/// history may hold anything.
pub fn save(path: &Path, snap: &Snapshot) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    write_private(path, &bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------------------------------------
// Presentation helpers (shared by the panel and the wheel menu)
// ------------------------------------------------------------------------------------------------

/// One line: whitespace runs collapsed, at most `max` characters (with an ellipsis).
pub fn preview(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    for word in text.split_whitespace() {
        if n > 0 {
            out.push(' ');
            n += 1;
        }
        for c in word.chars() {
            if n >= max {
                out.push('…');
                return out;
            }
            out.push(c);
            n += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Link,
    Color,
    Number,
    Lines,
    Text,
}

pub fn kind(text: &str) -> Kind {
    let t = text.trim();
    let one_word = !t.contains(char::is_whitespace);
    if one_word && (t.starts_with("http://") || t.starts_with("https://")) {
        Kind::Link
    } else if one_word && is_hex_color(t) {
        Kind::Color
    } else if one_word && t.replace([',', '_'], "").parse::<f64>().is_ok() {
        Kind::Number
    } else if t.lines().count() > 1 {
        Kind::Lines
    } else {
        Kind::Text
    }
}

fn is_hex_color(t: &str) -> bool {
    t.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 4 | 6 | 8) && h.chars().all(|c| c.is_ascii_hexdigit()))
}

pub fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Link => "lucide:Link",
        Kind::Color => "lucide:Palette",
        Kind::Number => "lucide:Hash",
        Kind::Lines => "lucide:AlignLeft",
        Kind::Text => "lucide:Type",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_to_the_top_and_caps() {
        let mut h = History::new(3);
        assert!(h.add("a".into(), None, 1));
        h.add("b".into(), Some("Notes".into()), 2);
        h.add("a".into(), None, 3);
        let texts: Vec<&str> = h.entries().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["a", "b"]);
        assert_eq!(h.entries()[0].id, 1, "same entry, moved");
        assert_eq!(h.entries()[0].ts, 3);
        h.add("c".into(), None, 4);
        h.add("d".into(), None, 5);
        assert_eq!(h.len(), 3);
        assert!(h.get(2).is_none(), "oldest dropped");
        assert!(!h.add("   \n".into(), None, 6), "blank copies are ignored");
        assert!(!h.add("x".repeat(MAX_ENTRY_BYTES + 1), None, 6));
    }

    #[test]
    fn search_needs_every_term() {
        let mut h = History::new(10);
        h.add("Hello World".into(), None, 1);
        h.add("hello there".into(), None, 2);
        h.add("你好 世界".into(), None, 3);
        assert_eq!(h.search("").len(), 3);
        assert_eq!(h.search("HELLO").len(), 2);
        assert_eq!(h.search("hello wor").len(), 1);
        assert_eq!(h.search("世界").len(), 1);
        assert!(h.search("nope").is_empty());
    }

    #[test]
    fn persists_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/clipboard.json");
        let mut h = History::load(&path, 10);
        h.add("one".into(), None, 1);
        h.add("Two".into(), None, 2);
        assert!(h.remove(1));
        assert!(!h.remove(1));
        save(&path, &h.snapshot()).unwrap();
        let h2 = History::load(&path, 10);
        assert_eq!(h2.entries().len(), 1);
        assert_eq!(h2.entries()[0].text, "Two");
        assert_eq!(h2.search("two").len(), 1, "search keys are rebuilt on load");
        let mut h2 = h2;
        h2.add("three".into(), None, 3);
        assert_eq!(h2.entries()[0].id, 3, "ids keep counting after a reload");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&path, "garbage").unwrap();
        assert!(History::load(&path, 10).is_empty());
    }

    #[test]
    fn previews_and_kinds() {
        assert_eq!(preview("  a\n\n  b\tc  ", 10), "a b c");
        assert_eq!(preview("abcdef", 3), "abc…");
        assert_eq!(kind("https://x.y/z"), Kind::Link);
        assert_eq!(kind("#ff8800"), Kind::Color);
        assert_eq!(kind("1,234.5"), Kind::Number);
        assert_eq!(kind("a\nb"), Kind::Lines);
        assert_eq!(kind("see https://x"), Kind::Text);
    }
}
