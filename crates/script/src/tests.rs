use super::*;
use anytools_core::{AppInfo, Selection};
use anytools_platform::mock::{Call, Mock};
use std::sync::atomic::AtomicUsize;

fn ctx() -> Context {
    Context {
        app: Some(AppInfo { id: "com.apple.finder".into(), name: "Finder".into(), pid: 7, title: None }),
        selection: Selection::Files { paths: vec!["/tmp/a.txt".into()] },
        selection_known: true,
        cwd: Some("/tmp".into()),
        ..Default::default()
    }
}

fn go(js: &str, perms: Perms) -> (Result<Outcome, String>, Mock) {
    let mock = Mock::new();
    let r = run(js, "t.js", &perms, &ctx(), &mock.services(), Duration::from_secs(5));
    (r, mock)
}

#[test]
fn api_basics() {
    let (r, mock) = go(
        r#"
        console.log("app", context.app.id, context.selection.length, context.platform);
        keys.send("mod+shift+p");
        keys.type("hi");
        clipboard.writeText("x");
        app.open("https://example.com");
        ui.notify({ a: 1 });
        await null;
        return { n: clipboard.readText().length };
        "#,
        Perms::default(),
    );
    let out = r.unwrap();
    let platform = if cfg!(target_os = "macos") { "mac" } else if cfg!(target_os = "windows") { "win" } else { "other" };
    assert_eq!(out.logs, [format!("app com.apple.finder 1 {platform}")]);
    assert_eq!(out.result.as_deref(), Some(r#"{"n":1}"#));
    let calls = mock.calls();
    assert!(matches!(&calls[0], Call::Chord(c) if c.key == "p"));
    assert_eq!(calls[1], Call::Text("hi".into()));
    assert_eq!(calls[3], Call::Open("https://example.com".into()));
    assert_eq!(calls[4], Call::Notify("anyTools".into(), r#"{"a":1}"#.into()));
}

#[test]
fn permissions_are_enforced() {
    // A path that is absolute on every platform, so the check reaches the permission and not the syntax.
    let abs = std::env::temp_dir().join("hosts");
    let abs = abs.to_string_lossy().replace('\\', "\\\\");
    let (r, _) = go(&format!("fs.read({abs:?})"), Perms::default());
    let e = r.unwrap_err();
    assert!(e.contains("没有权限") && e.contains("@permissions fs:"), "{e}");
    let (r, _) = go("shell.exec('echo hi')", Perms::default());
    assert!(r.unwrap_err().contains("@permissions shell"));
    let (r, _) = go("http.fetch('https://example.com/x')", Perms::default());
    assert!(r.unwrap_err().contains("http:example.com"));
    let (r, _) = go("http.fetch('https://evil.com/x')", Perms { http: Some(vec!["example.com".into()]), ..Default::default() });
    assert!(r.unwrap_err().contains("evil.com"));
    let (r, _) = go("http.fetch('file:///etc/passwd')", Perms { http: Some(vec![]), ..Default::default() });
    assert!(r.is_err());
}

#[test]
fn fs_within_granted_dir() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    let p = Perms { fs: Some(vec![dir.clone()]), ..Default::default() };
    let js = format!(
        r#"
        const d = {dir:?};
        fs.mkdir(d + "/sub");
        fs.write(d + "/sub/a.txt", "one");
        fs.append(d + "/sub/a.txt", "+two");
        if (!fs.exists(d + "/sub/a.txt")) throw new Error("missing");
        return [fs.read(d + "/sub/a.txt"), fs.list(d + "/sub")];
        "#,
        dir = dir.to_string_lossy()
    );
    let (r, _) = go(&js, p.clone());
    assert_eq!(r.unwrap().result.as_deref(), Some(r#"["one+two",["a.txt"]]"#));
    let (r, _) = go(&format!("fs.read({:?} + '/../x')", dir.to_string_lossy()), p);
    assert!(r.unwrap_err().contains("没有权限"), "no escape via ..");
}

#[test]
fn shell_exec_uses_platform_shell() {
    let (r, mock) = go("return shell.exec('echo hi').code", Perms { shell: true, ..Default::default() });
    assert_eq!(r.unwrap().result.as_deref(), Some("0"));
    assert!(matches!(&mock.calls()[0], Call::Script(_, s) if s == "echo hi"));
}

#[test]
fn errors_timeouts_and_memory() {
    let (r, _) = go("throw new Error('boom')", Perms::default());
    assert!(r.unwrap_err().starts_with("boom"));
    let (r, _) = go("await Promise.reject(new TypeError('nope'))", Perms::default());
    assert!(r.unwrap_err().contains("nope"));

    let mock = Mock::new();
    let t = Instant::now();
    let r = run("while (true) {}", "loop.js", &Perms::default(), &ctx(), &mock.services(), Duration::from_millis(300));
    assert!(r.unwrap_err().contains("超时") && t.elapsed() < Duration::from_secs(3));

    let (r, _) = go("const a = []; while (true) a.push('x'.repeat(1 << 20));", Perms::default());
    let e = r.unwrap_err();
    assert!(e.contains("内存") || e.to_lowercase().contains("memory"), "{e}");
}

#[test]
fn sleep_respects_deadline() {
    let mock = Mock::new();
    let t = Instant::now();
    let r = run("sleep(5000); return 1", "s.js", &Perms::default(), &ctx(), &mock.services(), Duration::from_millis(200));
    // sleep is cut short at the deadline; the script may or may not finish, but must not hang.
    let _ = r;
    assert!(t.elapsed() < Duration::from_secs(2));
}

fn host(dir: &Path, approve: bool) -> (ScriptHost, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let a2 = asked.clone();
    let h = ScriptHost {
        scripts_dir: dir.to_path_buf(),
        grants: Arc::new(Grants::load(dir.join(".grants.json"))),
        consent: Arc::new(move |_, _| {
            a2.fetch_add(1, Ordering::SeqCst);
            approve
        }),
    };
    (h, asked)
}

#[test]
fn consent_once_per_content_and_ts() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.ts"), "// @permissions shell\nconst n: number = 2;\nreturn n * 21;\n").unwrap();
    let (h, asked) = host(d.path(), true);
    let mock = Mock::new();
    let out = h.run_file("a.ts", &ctx(), &mock.services(), DEFAULT_TIMEOUT).unwrap();
    assert_eq!(out.result.as_deref(), Some("42"));
    h.run_file("a.ts", &ctx(), &mock.services(), DEFAULT_TIMEOUT).unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 1, "approved once");
    std::fs::write(d.path().join("a.ts"), "// @permissions shell\nreturn 1;\n").unwrap();
    h.run_file("a.ts", &ctx(), &mock.services(), DEFAULT_TIMEOUT).unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2, "edited → asked again");

    std::fs::write(d.path().join("b.js"), "return 1").unwrap();
    h.run_file("b.js", &ctx(), &mock.services(), DEFAULT_TIMEOUT).unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2, "no permissions → no question");

    let (deny, _) = host(d.path(), false);
    std::fs::write(d.path().join("c.js"), "// @permissions fs\nreturn 1").unwrap();
    assert!(deny.run_file("c.js", &ctx(), &mock.services(), DEFAULT_TIMEOUT).unwrap_err().contains("未获批准"));
}

#[test]
fn script_paths_are_confined() {
    let d = tempfile::tempdir().unwrap();
    let (h, _) = host(d.path(), true);
    assert!(h.resolve("../x.js").is_err());
    let outside = std::env::temp_dir().join("x.js");
    assert!(h.resolve(&outside.to_string_lossy()).is_err(), "absolute paths stay outside scripts/");
    assert!(h.resolve("x.py").is_err());
    assert!(h.resolve("sub/x.ts").is_ok());
}

#[test]
fn action_validates_and_runs() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("hello.js"), "keys.type('hello')").unwrap();
    let (h, _) = host(d.path(), true);
    let reg = Registry::default();
    reg.register(Arc::new(ScriptAction { host: h }));
    let spec: anytools_core::ActionSpec = serde_yaml_ng::from_str("{ type: script, file: hello.js }").unwrap();
    reg.validate(&spec).unwrap();
    assert!(reg.available(&spec, &ctx()));
    let mock = Mock::new();
    reg.run(&spec, &ctx(), &mock.services()).unwrap();
    assert_eq!(mock.calls(), [Call::Text("hello".into())]);
    let missing: anytools_core::ActionSpec = serde_yaml_ng::from_str("{ type: script, file: nope.js }").unwrap();
    reg.validate(&missing).unwrap();
    assert!(!reg.available(&missing, &ctx()), "missing file → greyed out");
    assert!(reg.validate(&serde_yaml_ng::from_str("{ type: script, file: ../x.js }").unwrap()).is_err());
}
