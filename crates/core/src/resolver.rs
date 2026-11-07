//! `when:` conditions shared by every module that picks a profile by context.

use crate::context::{Context, Platform, Selection};
use crate::per_platform::{OneOrMany, PerPlatform};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelKind {
    /// Nothing selected (and the selection is known).
    None,
    /// At least one file or folder.
    Files,
    /// Only regular files.
    File,
    /// Only folders.
    Folder,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountRange {
    #[serde(default)]
    pub min: Option<usize>,
    #[serde(default)]
    pub max: Option<usize>,
}

/// The `when:` block as written in config. All given conditions must hold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhenCfg {
    /// Bundle id / exe name / app name; `*` wildcards; case-insensitive.
    #[serde(default)]
    pub app: Option<PerPlatform<OneOrMany>>,
    /// Regex on the front window title.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub selection: Option<SelKind>,
    /// Any selected file has one of these extensions (without the dot).
    #[serde(default)]
    pub ext: Option<OneOrMany>,
    #[serde(default)]
    pub count: Option<CountRange>,
    #[serde(default)]
    pub platform: Option<Platform>,
}

/// A compiled, ready-to-match [`WhenCfg`].
#[derive(Clone, Debug, Default)]
pub struct When {
    /// `None`: no app condition. `Some(empty)`: an app condition exists but not for this platform,
    /// so the rule never matches here.
    app: Option<Vec<String>>,
    title: Option<Regex>,
    selection: Option<SelKind>,
    ext: Option<Vec<String>>,
    count: Option<CountRange>,
    platform: Option<Platform>,
}

fn wildcard(pattern: &str, text: &str) -> bool {
    // Iterative glob with `*` only, on lowercase ASCII-insensitive text.
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi].eq_ignore_ascii_case(&t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = ti;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

impl When {
    pub fn compile(cfg: &WhenCfg) -> Result<When, String> {
        Self::compile_for(cfg, Platform::current())
    }

    pub fn compile_for(cfg: &WhenCfg, p: Platform) -> Result<When, String> {
        let title = match &cfg.title {
            Some(t) => Some(Regex::new(t).map_err(|e| format!("title 正则表达式有误: {e}"))?),
            None => None,
        };
        Ok(When {
            app: cfg.app.as_ref().map(|a| a.get_for(p).map(|v| v.iter().map(str::to_string).collect()).unwrap_or_default()),
            title,
            selection: cfg.selection,
            ext: cfg.ext.as_ref().map(|e| e.iter().map(|s| s.trim_start_matches('.').to_ascii_lowercase()).collect()),
            count: cfg.count,
            platform: cfg.platform,
        })
    }

    /// Whether this rule looks at the selection (and so may change once the selection probe returns).
    pub fn uses_selection(&self) -> bool {
        self.selection.is_some() || self.ext.is_some() || self.count.is_some()
    }

    /// Whether this rule needs the selected *text* (worth reading it outside file managers).
    pub fn uses_text(&self) -> bool {
        self.selection == Some(SelKind::Text)
    }

    pub fn matches(&self, ctx: &Context) -> bool {
        self.matches_on(ctx, Platform::current())
    }

    pub fn matches_on(&self, ctx: &Context, p: Platform) -> bool {
        if self.platform.is_some_and(|want| want != p) {
            return false;
        }
        if let Some(pats) = &self.app {
            let Some(app) = &ctx.app else { return false };
            if !pats.iter().any(|pat| wildcard(pat, &app.id) || wildcard(pat, &app.name)) {
                return false;
            }
        }
        if let Some(re) = &self.title {
            if !ctx.app.as_ref().and_then(|a| a.title.as_deref()).is_some_and(|t| re.is_match(t)) {
                return false;
            }
        }
        if self.uses_selection() && !ctx.selection_known {
            return false;
        }
        if let Some(kind) = self.selection {
            let ok = match (&ctx.selection, kind) {
                (Selection::None, SelKind::None) => true,
                (Selection::Files { paths }, SelKind::None) => paths.is_empty(),
                (Selection::Text { text }, SelKind::None) => text.is_empty(),
                (Selection::Files { paths }, SelKind::Files) => !paths.is_empty(),
                (Selection::Files { paths }, SelKind::File) => !paths.is_empty() && paths.iter().all(|p| !p.is_dir()),
                (Selection::Files { paths }, SelKind::Folder) => !paths.is_empty() && paths.iter().all(|p| p.is_dir()),
                (Selection::Text { text }, SelKind::Text) => !text.is_empty(),
                _ => false,
            };
            if !ok {
                return false;
            }
        }
        if let Some(exts) = &self.ext {
            let files = ctx.selection.files();
            let hit = files.iter().any(|f| {
                f.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
            });
            if !hit {
                return false;
            }
        }
        if let Some(c) = self.count {
            let n = ctx.selection.files().len();
            if c.min.is_some_and(|m| n < m) || c.max.is_some_and(|m| n > m) {
                return false;
            }
        }
        true
    }
}

/// First matching rule wins.
pub fn first_match<'a, T>(rules: &'a [(When, T)], ctx: &Context) -> Option<&'a T> {
    rules.iter().find(|(w, _)| w.matches(ctx)).map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AppInfo;
    use std::path::PathBuf;

    fn ctx(app: &str, files: Option<Vec<&str>>) -> Context {
        Context {
            app: Some(AppInfo { id: app.into(), name: "App".into(), pid: 1, title: Some("main.rs — anyTools".into()) }),
            selection_known: files.is_some(),
            selection: files
                .map(|f| Selection::Files { paths: f.into_iter().map(PathBuf::from).collect() })
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    fn when(yaml: &str, p: Platform) -> When {
        When::compile_for(&serde_yaml_ng::from_str(yaml).unwrap(), p).unwrap()
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("com.google.*", "com.google.Chrome"));
        assert!(wildcard("*code*", "com.microsoft.VSCode"));
        assert!(wildcard("CODE.EXE", "Code.exe"));
        assert!(!wildcard("com.apple.*", "com.google.Chrome"));
        assert!(wildcard("*", ""));
    }

    #[test]
    fn app_per_platform() {
        let w = when("app: { mac: com.microsoft.VSCode, win: Code.exe }", Platform::Mac);
        assert!(w.matches_on(&ctx("com.microsoft.VSCode", None), Platform::Mac));
        assert!(!w.matches_on(&ctx("Code.exe", None), Platform::Mac));
        let w = when("app: { win: Code.exe }", Platform::Mac);
        assert!(!w.matches_on(&ctx("Code.exe", None), Platform::Mac), "no mac pattern: never matches on mac");
        let w = when("app: [com.apple.Safari, 'com.google.*']", Platform::Mac);
        assert!(w.matches_on(&ctx("com.google.Chrome", None), Platform::Mac));
    }

    #[test]
    fn selection_rules_wait_for_probe() {
        let w = when("selection: files", Platform::Mac);
        assert!(!w.matches_on(&ctx("com.apple.finder", None), Platform::Mac), "unknown selection never matches");
        assert!(w.matches_on(&ctx("com.apple.finder", Some(vec!["/tmp/a.txt"])), Platform::Mac));
        assert!(!w.matches_on(&ctx("com.apple.finder", Some(vec![])), Platform::Mac));
        assert!(when("selection: none", Platform::Mac).matches_on(&ctx("x", Some(vec![])), Platform::Mac));
    }

    #[test]
    fn ext_count_title() {
        let c = ctx("com.apple.finder", Some(vec!["/tmp/a.PNG", "/tmp/b.txt"]));
        assert!(when("ext: [png, jpg]", Platform::Mac).matches_on(&c, Platform::Mac));
        assert!(!when("ext: pdf", Platform::Mac).matches_on(&c, Platform::Mac));
        assert!(when("count: { min: 2 }", Platform::Mac).matches_on(&c, Platform::Mac));
        assert!(!when("count: { max: 1 }", Platform::Mac).matches_on(&c, Platform::Mac));
        assert!(when("title: '\\.rs\\b'", Platform::Mac).matches_on(&c, Platform::Mac));
        assert!(When::compile(&serde_yaml_ng::from_str("title: '('").unwrap()).is_err());
    }

    #[test]
    fn text_selection() {
        let w = when("selection: text", Platform::Mac);
        assert!(w.uses_text() && w.uses_selection());
        let mut c = Context { selection_known: true, selection: Selection::Text { text: "hello".into() }, ..Default::default() };
        assert!(w.matches_on(&c, Platform::Mac));
        assert!(!when("selection: files", Platform::Mac).matches_on(&c, Platform::Mac));
        c.selection = Selection::Text { text: String::new() };
        assert!(!w.matches_on(&c, Platform::Mac));
        assert!(when("selection: none", Platform::Mac).matches_on(&c, Platform::Mac));
        c.selection_known = false;
        c.selection = Selection::Text { text: "x".into() };
        assert!(!w.matches_on(&c, Platform::Mac), "not before the probe finished");
    }

    #[test]
    fn empty_when_matches_everything() {
        assert!(When::default().matches(&Context::default()));
    }
}
