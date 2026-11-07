//! Script permissions: declared in a header comment, granted per script *content* (sha256).
//!
//! ```js
//! // @permissions fs:~/Documents shell http:api.example.com
//! ```
//!
//! - `fs` (any path), `fs:<dir>` (that directory and below; `~` expands)
//! - `shell`
//! - `http` (any host), `http:<host>` (that host and its subdomains)
//!
//! Keystrokes, clipboard, launching/opening and notifications need no declaration: they are the same
//! things a plain wheel item can already do.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Perms {
    /// `None`: no file access. `Some(empty)`: any path.
    pub fs: Option<Vec<PathBuf>>,
    pub shell: bool,
    /// `None`: no network. `Some(empty)`: any host.
    pub http: Option<Vec<String>>,
}

impl Perms {
    pub fn is_empty(&self) -> bool {
        self.fs.is_none() && !self.shell && self.http.is_none()
    }

    /// One line per permission, for the consent dialog.
    pub fn describe(&self) -> Vec<String> {
        let mut v = vec![];
        match &self.fs {
            Some(dirs) if dirs.is_empty() => v.push("读写任意文件".to_string()),
            Some(dirs) => v.extend(dirs.iter().map(|d| format!("读写 {} 中的文件", d.display()))),
            None => {}
        }
        if self.shell {
            v.push("运行任意命令".into());
        }
        match &self.http {
            Some(h) if h.is_empty() => v.push("访问任意网站".to_string()),
            Some(h) => v.extend(h.iter().map(|h| format!("访问 {h}"))),
            None => {}
        }
        v
    }

    pub fn allows_path(&self, p: &Path) -> bool {
        match &self.fs {
            None => false,
            Some(dirs) if dirs.is_empty() => true,
            Some(dirs) => {
                let p = normalize(p);
                dirs.iter().any(|d| p.starts_with(normalize(d)))
            }
        }
    }

    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match &self.http {
            None => false,
            Some(hs) if hs.is_empty() => true,
            Some(hs) => hs.iter().any(|h| host == *h || host.ends_with(&format!(".{h}"))),
        }
    }
}

fn expand_tilde(p: &str) -> PathBuf {
    match (p.strip_prefix('~'), std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))) {
        (Some(rest), Some(home)) => {
            let mut b = PathBuf::from(home);
            let rest = rest.trim_start_matches(['/', '\\']);
            if !rest.is_empty() {
                b.push(rest);
            }
            b
        }
        _ => PathBuf::from(p),
    }
}

/// `canonicalize` on Windows returns `\\?\C:\...`. Scripts join paths as strings, where that prefix
/// makes the result invalid, so comparisons use the plain form. The prefix is only ever added by the
/// platform, never typed by a script, so dropping it doesn't widen what a script can reach.
#[cfg(windows)]
fn strip_verbatim(p: &Path) -> PathBuf {
    p.to_string_lossy().trim_start_matches(r"\\?\").into()
}

#[cfg(not(windows))]
fn strip_verbatim(p: &Path) -> PathBuf {
    p.to_path_buf()
}

/// Lexical normalization (`a/./b/../c` → `a/c`), plus symlink resolution of the longest existing prefix,
/// so `fs:~/Docs` can't be escaped with `..` or a symlink.
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    // Resolve symlinks for the part that exists.
    let mut existing = out.clone();
    let mut rest = vec![];
    while !existing.as_os_str().is_empty() && existing.symlink_metadata().is_err() {
        match existing.file_name() {
            Some(n) => rest.push(n.to_os_string()),
            None => break,
        }
        existing.pop();
    }
    let mut base = existing.canonicalize().map(|p| strip_verbatim(&p)).unwrap_or(existing);
    for n in rest.into_iter().rev() {
        base.push(n);
    }
    base
}

/// Parses `@permissions` from the script's leading comment block.
pub fn parse(source: &str) -> Result<Perms, String> {
    let mut perms = Perms::default();
    for line in source.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("#!") {
            continue;
        }
        let Some(c) = t.strip_prefix("//") else { break };
        let Some(list) = c.trim().strip_prefix("@permissions") else { continue };
        for tok in list.split_whitespace() {
            match tok.split_once(':') {
                None if tok == "fs" => perms.fs = Some(vec![]),
                None if tok == "shell" => perms.shell = true,
                None if tok == "http" => perms.http = Some(vec![]),
                Some(("fs", dir)) if !dir.is_empty() => {
                    let d = expand_tilde(dir);
                    if !d.is_absolute() {
                        return Err(format!("权限 fs:{dir} 必须是绝对路径或以 ~ 开头"));
                    }
                    match &mut perms.fs {
                        Some(v) if v.is_empty() => {} // already "any path"
                        Some(v) => v.push(d),
                        None => perms.fs = Some(vec![d]),
                    }
                }
                Some(("http", host)) if !host.is_empty() && !host.contains('/') => {
                    let h = host.to_ascii_lowercase();
                    match &mut perms.http {
                        Some(v) if v.is_empty() => {}
                        Some(v) => v.push(h),
                        None => perms.http = Some(vec![h]),
                    }
                }
                _ => return Err(format!("无法识别的权限声明 \"{tok}\"（可用：fs、fs:<目录>、shell、http、http:<域名>）")),
            }
        }
    }
    Ok(perms)
}

pub fn digest(source: &str) -> String {
    let mut h = Sha256::new();
    h.update(source.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Remembered consent: script content hash → the permissions that were approved for it.
pub struct Grants {
    path: PathBuf,
    map: Mutex<BTreeMap<String, Perms>>,
}

impl Grants {
    pub fn load(path: PathBuf) -> Grants {
        let map = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Grants { path, map: Mutex::new(map) }
    }

    pub fn is_granted(&self, hash: &str, perms: &Perms) -> bool {
        perms.is_empty() || self.map.lock().unwrap().get(hash) == Some(perms)
    }

    pub fn grant(&self, hash: &str, perms: &Perms) {
        let mut m = self.map.lock().unwrap();
        m.insert(hash.to_string(), perms.clone());
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&*m) {
            // Write-then-rename so a crash never leaves a truncated file.
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header() {
        let abs = if cfg!(windows) { "C:/tmp/x" } else { "/tmp/x" };
        let p = parse(&format!("#!/usr/bin/env anytools\n// 日报\n// @permissions fs:{abs} shell\n// @permissions http:API.example.com\nlet a = 1;\n// @permissions http\n")).unwrap();
        assert_eq!(p.fs, Some(vec![PathBuf::from(abs)]));
        assert!(p.shell);
        assert_eq!(p.http, Some(vec!["api.example.com".to_string()]), "stops at the first non-comment line");
        assert!(parse("// @permissions net").is_err());
        assert!(parse("// @permissions fs:relative/dir").is_err());
        assert!(parse("const x = 1").unwrap().is_empty());
        assert_eq!(parse("// @permissions fs fs:/a").unwrap().fs, Some(vec![]), "fs (any) wins");
    }

    #[test]
    fn path_and_host_checks() {
        let d = tempfile::tempdir().unwrap();
        let allowed = d.path().join("ok");
        std::fs::create_dir(&allowed).unwrap();
        let p = Perms { fs: Some(vec![allowed.clone()]), ..Default::default() };
        assert!(p.allows_path(&allowed.join("a/b.txt")));
        assert!(!p.allows_path(&allowed.join("../secret")), "no .. escape");
        assert!(!p.allows_path(&d.path().join("okay")), "prefix of the name is not enough");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.path(), allowed.join("link")).unwrap();
            assert!(!p.allows_path(&allowed.join("link/secret")), "no symlink escape");
        }
        let h = Perms { http: Some(vec!["example.com".into()]), ..Default::default() };
        assert!(h.allows_host("example.com") && h.allows_host("API.example.com"));
        assert!(!h.allows_host("evilexample.com"));
        assert!(!Perms::default().allows_host("example.com"));
    }

    #[test]
    fn grants_are_per_content() {
        let d = tempfile::tempdir().unwrap();
        let g = Grants::load(d.path().join("g.json"));
        let p = Perms { shell: true, ..Default::default() };
        let h1 = digest("a");
        assert!(!g.is_granted(&h1, &p));
        g.grant(&h1, &p);
        assert!(g.is_granted(&h1, &p));
        assert!(!g.is_granted(&digest("b"), &p), "edited script needs consent again");
        let more = Perms { shell: true, http: Some(vec![]), ..Default::default() };
        assert!(!g.is_granted(&h1, &more), "more permissions need consent again");
        assert!(Grants::load(d.path().join("g.json")).is_granted(&h1, &p), "persisted");
        assert!(g.is_granted(&digest("c"), &Perms::default()), "no permissions: nothing to ask");
    }
}
