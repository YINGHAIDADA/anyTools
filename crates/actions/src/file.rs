//! `file.*` actions on the file manager selection.

use anytools_core::action::params;
use anytools_core::{ActionCtx, ActionHandler, Context, Params, PerPlatform, Registry};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub fn all() -> Vec<Arc<dyn ActionHandler>> {
    vec![
        Arc::new(OpenTerminal),
        Arc::new(Copy),
        Arc::new(Cut),
        Arc::new(PasteHere),
        Arc::new(CopyPath),
        Arc::new(Zip),
        Arc::new(Trash),
        Arc::new(OpenWith),
        Arc::new(NewFolder),
    ]
}

fn selected(ctx: &Context) -> Result<&[PathBuf], String> {
    let f = ctx.selection.files();
    if f.is_empty() {
        Err("没有选中文件".into())
    } else {
        Ok(f)
    }
}

fn work_dir(ctx: &Context) -> Result<PathBuf, String> {
    ctx.work_dir().filter(|d| d.is_dir()).ok_or_else(|| "不知道当前所在的文件夹".into())
}

fn has_selection(ctx: &Context) -> bool {
    !ctx.selection.files().is_empty()
}

fn no_params(kind: &str, p: &Params) -> Result<(), String> {
    if p.is_empty() {
        Ok(())
    } else {
        Err(format!("动作 {kind} 不接受参数: {}", p.keys().cloned().collect::<Vec<_>>().join(", ")))
    }
}

/// `name.ext` → `name 2.ext`, `name 3.ext`, … (like Finder) until it doesn't exist.
pub fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() && first.symlink_metadata().is_err() {
        return first;
    }
    let p = Path::new(file_name);
    let (stem, ext) = match (p.file_stem().and_then(|s| s.to_str()), p.extension().and_then(|s| s.to_str())) {
        (Some(s), Some(e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (file_name.to_string(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} {n}{ext}")))
        .find(|c| !c.exists() && c.symlink_metadata().is_err())
        .expect("unbounded range")
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalP {
    #[serde(default)]
    terminal: Option<PerPlatform<String>>,
}

pub struct OpenTerminal;

impl ActionHandler for OpenTerminal {
    fn kind(&self) -> &'static str {
        "file.open_terminal"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<TerminalP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        ctx.work_dir().is_some()
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: TerminalP = params(self.kind(), p)?;
        // A single selected folder opens *that* folder; otherwise the current folder.
        let files = cx.context.selection.files();
        let dir = match files {
            [one] if one.is_dir() => one.clone(),
            _ => work_dir(cx.context)?,
        };
        let term = p.terminal.as_ref().and_then(|t| t.get()).map(String::as_str);
        cx.services.shell.open_terminal(&dir, term)
    }
}

// ------------------------------------------------------------------------------------------------

/// Files put on the clipboard by `file.cut`; a later `file.paste_here` of the same files moves them.
static PENDING_CUT: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

pub struct Copy;

impl ActionHandler for Copy {
    fn kind(&self) -> &'static str {
        "file.copy"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        no_params(self.kind(), p)
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        has_selection(ctx)
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        cx.services.files.set_clipboard_files(selected(cx.context)?)?;
        *PENDING_CUT.lock().unwrap() = None;
        Ok(())
    }
}

pub struct Cut;

impl ActionHandler for Cut {
    fn kind(&self) -> &'static str {
        "file.cut"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        no_params(self.kind(), p)
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        has_selection(ctx)
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        let files = selected(cx.context)?;
        // The files are on the clipboard as a normal copy, so pasting in Finder with ⌥⌘V also moves
        // them; pasting with anyTools' "paste here" moves because of the pending-cut marker.
        cx.services.files.set_clipboard_files(files)?;
        *PENDING_CUT.lock().unwrap() = Some(files.to_vec());
        Ok(())
    }
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = src.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(src)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, dst);
        #[cfg(windows)]
        return if src.is_dir() { std::os::windows::fs::symlink_dir(target, dst) } else { std::os::windows::fs::symlink_file(target, dst) };
    }
    if meta.is_dir() {
        std::fs::create_dir(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

fn move_path(src: &Path, dst: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        // Different volume: copy, then remove the original only once the copy succeeded.
        Err(_) => {
            copy_recursive(src, dst)?;
            if src.is_dir() && !src.symlink_metadata()?.file_type().is_symlink() {
                std::fs::remove_dir_all(src)
            } else {
                std::fs::remove_file(src)
            }
        }
    }
}

/// Copies (or moves) `items` into `dir`, never overwriting. Returns the created paths.
pub fn paste_into(dir: &Path, items: &[PathBuf], mv: bool) -> Result<Vec<PathBuf>, String> {
    let dir_canon = dir.canonicalize().map_err(|e| format!("无法访问目标文件夹: {e}"))?;
    let mut created = vec![];
    for src in items {
        let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("无效的文件名: {}", src.display()))?;
        if src.is_dir() {
            if let Ok(sc) = src.canonicalize() {
                if dir_canon.starts_with(&sc) {
                    return Err(format!("不能把「{name}」放进它自己里面"));
                }
            }
        }
        if mv && src.parent().and_then(|p| p.canonicalize().ok()).as_deref() == Some(dir_canon.as_path()) {
            continue; // moving into the folder it is already in: nothing to do
        }
        let dst = unique_path(dir, name);
        let res = if mv { move_path(src, &dst) } else { copy_recursive(src, &dst) };
        res.map_err(|e| format!("{}「{name}」失败: {e}", if mv { "移动" } else { "复制" }))?;
        created.push(dst);
    }
    Ok(created)
}

pub struct PasteHere;

impl ActionHandler for PasteHere {
    fn kind(&self) -> &'static str {
        "file.paste_here"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        no_params(self.kind(), p)
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        ctx.work_dir().is_some()
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        // Paste into the folder being viewed, or into the one selected folder.
        let dir = match cx.context.selection.files() {
            [one] if one.is_dir() => one.clone(),
            _ => work_dir(cx.context)?,
        };
        let items = cx.services.files.clipboard_files();
        if items.is_empty() {
            return Err("剪贴板里没有文件".into());
        }
        let mut pending = PENDING_CUT.lock().unwrap();
        let mv = pending.as_deref() == Some(items.as_slice());
        paste_into(&dir, &items, mv)?;
        if mv {
            // The originals are gone; a second paste must not try to move them again.
            *pending = None;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyPathP {
    /// Wrap each path in double quotes.
    #[serde(default)]
    quote: bool,
    #[serde(default)]
    separator: Option<String>,
}

pub struct CopyPath;

impl ActionHandler for CopyPath {
    fn kind(&self) -> &'static str {
        "file.copy_path"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<CopyPathP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        has_selection(ctx) || ctx.work_dir().is_some()
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: CopyPathP = params(self.kind(), p)?;
        let files = cx.context.selection.files();
        // Nothing selected: copy the current folder's path.
        let list: Vec<PathBuf> = if files.is_empty() { vec![work_dir(cx.context)?] } else { files.to_vec() };
        let text = list
            .iter()
            .map(|f| {
                let s = f.to_string_lossy();
                let s = if f.is_dir() && s.len() > 1 { s.trim_end_matches('/').to_string() } else { s.into_owned() };
                if p.quote { format!("\"{s}\"") } else { s }
            })
            .collect::<Vec<_>>()
            .join(p.separator.as_deref().unwrap_or("\n"));
        cx.services.files.set_clipboard_text(&text)
    }
}

// ------------------------------------------------------------------------------------------------

pub struct Zip;

impl ActionHandler for Zip {
    fn kind(&self) -> &'static str {
        "file.zip"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        no_params(self.kind(), p)
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        has_selection(ctx)
    }
    fn run(&self, _: &Params, cx: &ActionCtx) -> Result<(), String> {
        let files = selected(cx.context)?;
        let base = files[0].parent().map(Path::to_path_buf).ok_or("无法确定所在文件夹")?;
        let name = match files {
            [one] => format!("{}.zip", one.file_name().and_then(|n| n.to_str()).unwrap_or("Archive")),
            _ => "Archive.zip".to_string(),
        };
        let dest = unique_path(&base, &name);
        cx.services.files.zip(&base, files, &dest)
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrashP {
    #[serde(default = "yes")]
    confirm: bool,
}

fn yes() -> bool {
    true
}

pub struct Trash;

impl ActionHandler for Trash {
    fn kind(&self) -> &'static str {
        "file.trash"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<TrashP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        has_selection(ctx)
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: TrashP = params(self.kind(), p)?;
        let files = selected(cx.context)?;
        if p.confirm {
            let what = match files {
                [one] => format!("「{}」", one.file_name().and_then(|n| n.to_str()).unwrap_or("?")),
                _ => format!("这 {} 个项目", files.len()),
            };
            if !cx.services.shell.confirm("移到废纸篓", &format!("确定要把{what}移到废纸篓吗？")) {
                return Ok(());
            }
        }
        cx.services.files.trash(files)
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenWithP {
    app: PerPlatform<String>,
}

pub struct OpenWith;

impl ActionHandler for OpenWith {
    fn kind(&self) -> &'static str {
        "file.open_with"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        params::<OpenWithP>(self.kind(), p).map(|_| ())
    }
    fn available(&self, p: &Params, ctx: &Context) -> bool {
        has_selection(ctx) && params::<OpenWithP>(self.kind(), p).is_ok_and(|p| p.app.get().is_some())
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: OpenWithP = params(self.kind(), p)?;
        let app = p.app.get().ok_or("file.open_with 没有为当前平台配置应用")?;
        cx.services.shell.open_with(app, selected(cx.context)?)
    }
}

// ------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewFolderP {
    #[serde(default)]
    name: Option<String>,
}

pub struct NewFolder;

impl ActionHandler for NewFolder {
    fn kind(&self) -> &'static str {
        "file.new_folder"
    }
    fn validate(&self, p: &Params, _: &Registry) -> Result<(), String> {
        let p: NewFolderP = params(self.kind(), p)?;
        match p.name.as_deref() {
            Some(n) if n.is_empty() || n.contains('/') || n.contains('\\') || n == "." || n == ".." => {
                Err(format!("文件夹名无效: \"{n}\""))
            }
            _ => Ok(()),
        }
    }
    fn available(&self, _: &Params, ctx: &Context) -> bool {
        ctx.work_dir().is_some()
    }
    fn run(&self, p: &Params, cx: &ActionCtx) -> Result<(), String> {
        let p: NewFolderP = params(self.kind(), p)?;
        let dir = work_dir(cx.context)?;
        let path = unique_path(&dir, p.name.as_deref().unwrap_or("未命名文件夹"));
        std::fs::create_dir(&path).map_err(|e| format!("新建文件夹失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use anytools_core::Selection;
    use anytools_platform::mock::Call;

    fn ctx_with(dir: &Path, files: &[&Path]) -> Context {
        Context {
            cwd: Some(dir.to_path_buf()),
            selection: Selection::Files { paths: files.iter().map(|p| p.to_path_buf()).collect() },
            selection_known: true,
            ..Default::default()
        }
    }

    #[test]
    fn unique_names_like_finder() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(d.path(), "a.txt"), d.path().join("a.txt"));
        std::fs::write(d.path().join("a.txt"), "").unwrap();
        assert_eq!(unique_path(d.path(), "a.txt"), d.path().join("a 2.txt"));
        std::fs::write(d.path().join("a 2.txt"), "").unwrap();
        assert_eq!(unique_path(d.path(), "a.txt"), d.path().join("a 3.txt"));
        std::fs::create_dir(d.path().join("dir")).unwrap();
        assert_eq!(unique_path(d.path(), "dir"), d.path().join("dir 2"));
        std::fs::write(d.path().join(".env"), "").unwrap();
        assert_eq!(unique_path(d.path(), ".env"), d.path().join(".env 2"));
    }

    #[test]
    fn cut_then_paste_moves_once() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        let (src, dst) = (d.path().join("src"), d.path().join("dst"));
        std::fs::create_dir_all(src.join("folder")).unwrap();
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("folder/b.txt"), "B").unwrap();

        let items = [src.join("a.txt").as_path().to_owned(), src.join("folder")];
        let item_refs: Vec<&Path> = items.iter().map(|p| p.as_path()).collect();
        run(&reg, &mock, &ctx_with(&src, &item_refs), "{ type: file.cut }").unwrap();
        run(&reg, &mock, &ctx_with(&dst, &[]), "{ type: file.paste_here }").unwrap();
        assert!(!src.join("a.txt").exists() && !src.join("folder").exists(), "originals moved away");
        assert_eq!(std::fs::read_to_string(dst.join("folder/b.txt")).unwrap(), "B");

        // The clipboard still lists the (now moved) originals; a second paste must not do anything odd.
        let err = run(&reg, &mock, &ctx_with(&dst, &[]), "{ type: file.paste_here }").unwrap_err();
        assert!(err.contains("复制"), "second paste is a copy of missing files and fails cleanly: {err}");
    }

    #[test]
    fn copy_then_paste_never_overwrites() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, "A").unwrap();
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.copy }").unwrap();
        run(&reg, &mock, &ctx_with(d.path(), &[]), "{ type: file.paste_here }").unwrap();
        run(&reg, &mock, &ctx_with(d.path(), &[]), "{ type: file.paste_here }").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "A");
        assert!(d.path().join("a 2.txt").exists() && d.path().join("a 3.txt").exists());
    }

    #[test]
    fn refuses_to_paste_folder_into_itself() {
        let d = tempfile::tempdir().unwrap();
        let folder = d.path().join("f");
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        let err = paste_into(&folder.join("inner"), std::slice::from_ref(&folder), false).unwrap_err();
        assert!(err.contains("它自己里面"), "{err}");
    }

    #[test]
    fn trash_asks_first_and_respects_no() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        mock.state.lock().unwrap().confirm_answer = false;
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.trash }").unwrap();
        assert!(matches!(mock.calls().as_slice(), [Call::Confirm(m)] if m.contains("a.txt")), "{:?}", mock.calls());
        mock.state.lock().unwrap().confirm_answer = true;
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.trash }").unwrap();
        assert_eq!(mock.calls().last(), Some(&Call::Trash(vec![f.clone()])));
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.trash, confirm: false }").unwrap();
        assert_eq!(mock.calls().len(), 4, "no dialog when confirm is off");
    }

    #[test]
    fn copy_path_and_terminal_dir() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let f = d.path().join("a b.txt");
        run(&reg, &mock, &ctx_with(d.path(), &[&f, &sub]), "{ type: file.copy_path, quote: true }").unwrap();
        assert_eq!(mock.state.lock().unwrap().clip_text.as_deref().unwrap(), format!("\"{}\"\n\"{}\"", f.display(), sub.display()));

        run(&reg, &mock, &ctx_with(d.path(), &[&sub]), "{ type: file.open_terminal }").unwrap();
        assert_eq!(mock.calls().last(), Some(&Call::Terminal(sub.clone(), None)), "one selected folder → that folder");
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.open_terminal, terminal: iTerm }").unwrap();
        assert_eq!(mock.calls().last(), Some(&Call::Terminal(d.path().to_path_buf(), Some("iTerm".into()))));
    }

    #[test]
    fn zip_names_and_availability() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("report.pdf");
        run(&reg, &mock, &ctx_with(d.path(), &[&f]), "{ type: file.zip }").unwrap();
        assert!(matches!(mock.calls().last(), Some(Call::Zip(_, _, dest)) if dest.ends_with("report.pdf.zip")));
        let g = d.path().join("x");
        run(&reg, &mock, &ctx_with(d.path(), &[&f, &g]), "{ type: file.zip }").unwrap();
        assert!(matches!(mock.calls().last(), Some(Call::Zip(_, _, dest)) if dest.ends_with("Archive.zip")));
        assert!(!reg.available(&spec("{ type: file.zip }"), &ctx_with(d.path(), &[])));
        assert!(reg.validate(&spec("{ type: file.zip, x: 1 }")).is_err());
    }

    #[test]
    fn new_folder_unique_and_validated() {
        let (reg, mock) = setup();
        let d = tempfile::tempdir().unwrap();
        run(&reg, &mock, &ctx_with(d.path(), &[]), "{ type: file.new_folder }").unwrap();
        run(&reg, &mock, &ctx_with(d.path(), &[]), "{ type: file.new_folder }").unwrap();
        assert!(d.path().join("未命名文件夹").is_dir() && d.path().join("未命名文件夹 2").is_dir());
        assert!(reg.validate(&spec("{ type: file.new_folder, name: '../x' }")).is_err());
    }
}
