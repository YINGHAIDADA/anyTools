//! `config.yaml`: global settings plus one section per module. Hot-reloaded; an invalid file never
//! replaces the last valid config.

use crate::event::{CoreEvent, EventBus};
use crate::per_platform::PerPlatform;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default = "default_theme")]
    pub theme: Theme,
    /// `system` or a CSS color.
    #[serde(default = "default_accent")]
    pub accent: String,
    /// Terminal for "open terminal here": app name / bundle id on macOS, exe on Windows.
    #[serde(default)]
    pub terminal: Option<PerPlatform<String>>,
    /// Glass material on macOS: hud | popover | menu | sidebar | fullscreen-ui | under-window.
    #[serde(default = "default_material")]
    pub material: String,
}

fn default_theme() -> Theme {
    Theme::Dark
}
fn default_accent() -> String {
    "system".into()
}
fn default_material() -> String {
    "hud".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: default_theme(), accent: default_accent(), terminal: None, material: default_material() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    /// Raw per-module sections; each module parses and validates its own.
    #[serde(default)]
    pub modules: BTreeMap<String, Value>,
}

impl ConfigFile {
    pub fn parse(text: &str) -> Result<ConfigFile, String> {
        let cfg: ConfigFile = serde_yaml_ng::from_str(text).map_err(|e| format!("配置文件格式错误: {e}"))?;
        if cfg.version > CONFIG_VERSION {
            return Err(format!("配置文件版本 {} 高于本程序支持的 {CONFIG_VERSION}，请升级 anyTools", cfg.version));
        }
        Ok(cfg)
    }

    pub fn module(&self, id: &str) -> Value {
        self.modules.get(id).cloned().unwrap_or(Value::Null)
    }
}

/// Validates a candidate config (each module parses its section). Returns the first error.
pub type Validator = Box<dyn Fn(&ConfigFile) -> Result<(), String> + Send + Sync>;

pub struct ConfigStore {
    path: PathBuf,
    current: RwLock<Arc<ConfigFile>>,
    validators: Mutex<Vec<Validator>>,
    last_error: Mutex<Option<String>>,
    bus: Arc<EventBus>,
    // Keeps the file watcher alive.
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl ConfigStore {
    /// Creates the store. If `path` does not exist, `default_text` is written there first.
    pub fn open(path: PathBuf, default_text: &str, bus: Arc<EventBus>) -> Result<Arc<ConfigStore>, String> {
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("无法创建配置目录: {e}"))?;
            }
            std::fs::write(&path, default_text).map_err(|e| format!("无法写入默认配置: {e}"))?;
        }
        // Start from the built-in default so there is always a valid config, then try the file.
        let fallback = ConfigFile::parse(default_text).map_err(|e| format!("内置默认配置无效: {e}"))?;
        Ok(Arc::new(ConfigStore {
            path,
            current: RwLock::new(Arc::new(fallback)),
            validators: Mutex::new(vec![]),
            last_error: Mutex::new(None),
            bus,
            watcher: Mutex::new(None),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> Arc<ConfigFile> {
        self.current.read().unwrap().clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn add_validator(&self, v: Validator) {
        self.validators.lock().unwrap().push(v);
    }

    /// Reads, parses and validates the file. On success it becomes current; on failure the previous
    /// config stays and the error is reported.
    pub fn reload(&self) -> Result<Arc<ConfigFile>, String> {
        let res = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("无法读取配置文件 {}: {e}", self.path.display()))
            .and_then(|t| ConfigFile::parse(&t))
            .and_then(|cfg| {
                for v in self.validators.lock().unwrap().iter() {
                    v(&cfg)?;
                }
                Ok(cfg)
            });
        match res {
            Ok(cfg) => {
                let cfg = Arc::new(cfg);
                *self.current.write().unwrap() = cfg.clone();
                *self.last_error.lock().unwrap() = None;
                tracing::info!(path = %self.path.display(), "config loaded");
                self.bus.emit(CoreEvent::ConfigLoaded { path: self.path.display().to_string() });
                Ok(cfg)
            }
            Err(e) => {
                tracing::warn!(error = %e, "config rejected; keeping the previous one");
                *self.last_error.lock().unwrap() = Some(e.clone());
                self.bus.emit(CoreEvent::ConfigError { message: e.clone() });
                Err(e)
            }
        }
    }

    /// Validates `cfg` exactly like a reload would, then writes it to disk (keeping a `.bak` of the
    /// previous file) and makes it current. YAML comments in the old file are not preserved.
    pub fn save(&self, cfg: ConfigFile) -> Result<Arc<ConfigFile>, String> {
        if cfg.version > CONFIG_VERSION {
            return Err(format!("配置版本 {} 高于支持的 {CONFIG_VERSION}", cfg.version));
        }
        for v in self.validators.lock().unwrap().iter() {
            v(&cfg)?;
        }
        let text = format!(
            "# anyTools 配置文件（由设置窗口保存；手写的注释不会保留，上一版在同目录的 config.yaml.bak）\n{}",
            serde_yaml_ng::to_string(&cfg).map_err(|e| format!("无法生成配置文件: {e}"))?
        );
        // Round-trip check: what we write must load back to the same config.
        if ConfigFile::parse(&text)? != cfg {
            return Err("内部错误：生成的配置无法原样读回".into());
        }
        if self.path.exists() {
            let _ = std::fs::copy(&self.path, self.path.with_extension("yaml.bak"));
        }
        let tmp = self.path.with_extension("yaml.tmp");
        std::fs::write(&tmp, &text).map_err(|e| format!("写入配置失败: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("写入配置失败: {e}"))?;
        // The watcher will also see this write and reload the identical file; harmless.
        let cfg = Arc::new(cfg);
        *self.current.write().unwrap() = cfg.clone();
        *self.last_error.lock().unwrap() = None;
        tracing::info!(path = %self.path.display(), "config saved");
        self.bus.emit(CoreEvent::ConfigLoaded { path: self.path.display().to_string() });
        Ok(cfg)
    }

    /// Watches the config file and reloads (debounced) when it changes.
    pub fn watch(self: &Arc<Self>) -> Result<(), String> {
        use notify::{RecursiveMode, Watcher};
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let file_name = self.path.file_name().map(|n| n.to_os_string());
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if ev.paths.iter().any(|p| p.file_name().map(|n| n.to_os_string()) == file_name) {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| e.to_string())?;
        // Watch the directory: editors often replace the file instead of writing it in place.
        let dir = self.path.parent().ok_or("配置文件没有父目录")?;
        watcher.watch(dir, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
        *self.watcher.lock().unwrap() = Some(watcher);

        let me = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("config-watch".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    // Debounce bursts of events from a single save.
                    while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
                    let Some(store) = me.upgrade() else { break };
                    let _ = store.reload();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "version: 1\nmodules:\n  demo: { n: 1 }\n";

    #[test]
    fn writes_default_and_keeps_last_good() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.yaml");
        let bus = Arc::new(EventBus::default());
        let errors = Arc::new(Mutex::new(vec![]));
        let e2 = errors.clone();
        bus.subscribe(move |ev| {
            if let CoreEvent::ConfigError { message } = ev {
                e2.lock().unwrap().push(message.clone());
            }
        });
        let store = ConfigStore::open(path.clone(), DEFAULT, bus).unwrap();
        assert!(path.exists(), "default config is written on first run");
        store.add_validator(Box::new(|c| {
            let n = c.module("demo")["n"].as_i64().unwrap_or(0);
            if n > 0 { Ok(()) } else { Err(format!("n must be positive, got {n}")) }
        }));
        assert_eq!(store.reload().unwrap().module("demo")["n"], 1);

        std::fs::write(&path, "version: 1\nmodules:\n  demo: { n: 0 }\n").unwrap();
        assert!(store.reload().is_err());
        assert_eq!(store.get().module("demo")["n"], 1, "invalid config does not replace the good one");
        assert!(store.last_error().unwrap().contains("positive"));

        std::fs::write(&path, "version: 1\nmodules: [").unwrap();
        assert!(store.reload().unwrap_err().contains("格式错误"));
        assert_eq!(errors.lock().unwrap().len(), 2);

        std::fs::write(&path, "version: 1\nmodules:\n  demo: { n: 5 }\n").unwrap();
        assert_eq!(store.reload().unwrap().module("demo")["n"], 5);
        assert!(store.last_error().is_none());
    }

    #[test]
    fn save_validates_backs_up_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let store = ConfigStore::open(path.clone(), DEFAULT, Arc::new(EventBus::default())).unwrap();
        store.add_validator(Box::new(|c| if c.module("demo")["n"] == 0 { Err("n=0".into()) } else { Ok(()) }));
        store.reload().unwrap();

        let mut c = (*store.get()).clone();
        c.modules.insert("demo".into(), serde_json::json!({ "n": 0 }));
        assert_eq!(store.save(c.clone()).unwrap_err(), "n=0");
        assert!(std::fs::read_to_string(&path).unwrap().contains("n: 1"), "rejected save leaves the file alone");

        c.modules.insert("demo".into(), serde_json::json!({ "n": 3, "label": "中文: 冒号 # 井号" }));
        c.settings.accent = "#7c5cff".into();
        store.save(c.clone()).unwrap();
        assert_eq!(*store.get(), c);
        assert!(dir.path().join("config.yaml.bak").exists());
        // A fresh store reading the written file gets the same config.
        assert_eq!(*store.reload().unwrap(), c);
    }

    #[test]
    fn rejects_unknown_top_level_keys_and_future_versions() {
        assert!(ConfigFile::parse("version: 1\nmodulez: {}\n").is_err());
        assert!(ConfigFile::parse("version: 99\n").unwrap_err().contains("升级"));
    }

    #[test]
    fn hot_reload_picks_up_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let store = ConfigStore::open(path.clone(), DEFAULT, Arc::new(EventBus::default())).unwrap();
        store.reload().unwrap();
        store.watch().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(&path, "version: 1\nmodules:\n  demo: { n: 7 }\n").unwrap();
        for _ in 0..50 {
            if store.get().module("demo")["n"] == 7 {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("config change was not picked up");
    }
}
