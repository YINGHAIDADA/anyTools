# anyTools 技术方案

> 状态：v0.1 讨论稿 · 2026-11-07
> 范围：整体架构 + 首个功能「快捷轮盘」

---

## 0. 已定决策

| 项 | 决定 | 说明 |
|---|---|---|
| 主体技术栈 | **Tauri 2 + Rust** | Rust 负责核心逻辑和系统集成，界面用 WebView，系统相关部分写原生代码 |
| 前端框架 | **React + TypeScript** | 轮盘浮层和设置页各打一个入口 |
| 内置脚本语言 | **JavaScript / TypeScript**（内嵌 QuickJS） | 同时支持 AppleScript、PowerShell、Shell |
| 最低系统版本 | **macOS 13+ / Windows 11 22H2+** | macOS 同时出 Apple Silicon 和 Intel 版本（Universal 包）；Windows 首发 x64 |
| 默认触发键 | **macOS：按住 ⌥Space；Windows：按住 CapsLock** | 短按 CapsLock 时照常切换大小写（原样发回）；用户可在设置里改 |
| 配置格式 | **YAML** | 设置页保存时注释会丢失（§9） |
| 开源协议 | **GPL-3.0** | 引入的第三方依赖必须与 GPL-3.0 兼容（MIT/Apache-2.0/BSD 都可以） |

> 选 Win11 22H2+ 是因为从这个版本起可以用 `DWMWA_SYSTEMBACKDROP_TYPE` 设置系统毛玻璃（Acrylic/Mica），而且资源管理器多标签页也是这个版本引入的，必须处理（见 §6.5）。

---

## 1. 目标与范围

### 1.1 产品目标
- 做一个常驻后台的日常效率工具。**架构层面要能方便地加新功能**，轮盘只是第一个功能模块。
- 同一份配置在 macOS 和 Windows 上都能用，差异只出现在少数平台专属字段里。
- 界面要好看：毛玻璃、边缘流动辉光、顺滑的动效。
- 唤起要快：从按下到看见轮盘 **≤ 50ms（p95）**，并且全程不抢前台应用的焦点。

### 1.2 第一版（v1）包含
- 快捷轮盘：按住唤起、松开执行；也支持点一下唤起、再点击选择。
- 三类轮盘：默认轮盘、选中文件时的文件轮盘、按前台应用匹配的轮盘。
- 动作：发送快捷键、输入文本、启动应用、打开文件或网址、运行 Shell / AppleScript / PowerShell / JS 脚本、内置文件操作、打开子轮盘、按顺序执行多个动作。
- 设置界面：可视化编辑轮盘、匹配规则、触发键，以及权限引导。
- 托盘或菜单栏图标、开机自启、自动更新。

### 1.3 第一版不做
- 第三方原生插件（WASM），只预留接口。
- 云同步、账号体系、使用数据上报（默认不收集任何数据）。
- Linux（架构上不排除，以后再做）。

---

## 2. 技术选型

### 2.1 后端（Rust）

| 用途 | 选型 |
|---|---|
| 应用框架 | `tauri` 2.x |
| 异步运行时 | `tokio` |
| macOS 系统调用 | `objc2` + `objc2-app-kit` / `objc2-foundation`，`core-graphics`，`core-foundation` |
| macOS 不抢焦点的面板 | `tauri-nspanel`（把 Tauri 窗口转换成 NSPanel） |
| Windows 系统调用 | `windows`（windows-rs，官方 crate） |
| 系统毛玻璃 | `window-vibrancy` 打底，形状裁剪自己实现 |
| 脚本引擎 | `rquickjs` |
| 剥离 TS 类型 | `oxc`（只去掉类型标注，不做打包） |
| 配置 | `serde` + YAML 解析库（`serde_yaml` 已停更，候选 `serde_yaml_ng` / `saphyr`，技术验证阶段确定）；用 `schemars` 生成 JSON Schema |
| 配置热加载 | `notify` |
| 移入废纸篓/回收站 | `trash` |
| 剪贴板 | `arboard`（文本）；文件剪贴板自己实现（要写剪切标记） |
| 日志与耗时统计 | `tracing` + `tracing-appender` |
| 生成前端 TS 类型 | `specta` + `tauri-specta` |
| Tauri 插件 | updater、single-instance、autostart、notification、dialog、log |

### 2.2 前端

| 用途 | 选型 |
|---|---|
| 构建 | Vite，分 `overlay`、`settings` 两个入口 |
| 轮盘浮层 | React + 原生 SVG + CSS；辉光层如果有性能问题就换成 WebGL（`ogl`，很轻）。**不引入组件库**，保证体积小、启动快 |
| 设置页 | React + Tailwind + Radix UI（或 shadcn/ui），拖拽排序用 `dnd-kit`，脚本编辑器用 Monaco（按需加载） |
| 状态管理 | `zustand` |
| 动效 | CSS 弹簧曲线（`linear()` 缓动）；设置页用 `motion` |

---

## 3. 总体架构

### 3.1 分层

```
┌──────────────────────────────────────────────────────────────────┐
│ UI 层 (React/TS, WebView)                                         │
│   overlay  : 轮盘渲染（只负责画，不做判断）                            │
│   settings : 配置编辑 / 权限引导 / 脚本编辑                            │
├──────────────────────────────────────────────────────────────────┤
│ Feature Modules（功能模块，可插拔）                                   │
│   wheel  | 规划中: clipboard-history · launcher · ocr · translate … │
├──────────────────────────────────────────────────────────────────┤
│ Core (Rust)                                                       │
│   ModuleHost · Registry(Trigger/Action/ContextProvider/Settings)  │
│   EventBus · ConfigStore(热加载/迁移) · Resolver · ScriptHost       │
├──────────────────────────────────────────────────────────────────┤
│ Platform Abstraction (traits)                                     │
│   InputHook · Injector · OverlayWindow · ContextProbe ·           │
│   FileOps · AppOps · Permissions · Icons                          │
│      ├── macos/   CGEventTap · NSPanel · NSVisualEffectView · AE  │
│      └── windows/ WH_*_LL · SendInput · DWM/Accent · Shell COM    │
└──────────────────────────────────────────────────────────────────┘
```

依赖只能自上而下：模块只依赖 Core 和平台层的 trait，不能直接碰平台的具体实现。这样可以用假实现（mock）来测试模块逻辑。

### 3.2 进程与线程模型

只有一个进程，分几个线程：

| 线程 | 职责 | 约束 |
|---|---|---|
| 主线程 | Tauri 事件循环、窗口操作（macOS 的 AppKit 调用必须在主线程） | 不能执行任何阻塞操作 |
| 钩子线程 | macOS 跑 CGEventTap 的 CFRunLoop；Windows 跑低级钩子的消息循环 | 回调 **< 1ms**，只查表、转发事件 |
| tokio 线程池 | 上下文探测、动作执行、文件 IO | — |
| 脚本线程 | 执行 QuickJS 脚本，每次执行创建独立的 Context，有超时可中断 | 与 UI 隔离，脚本卡死不影响唤起轮盘 |

两个 WebView：
- **overlay**：程序启动时就创建好并隐藏，常驻内存，唤起时只需要移动位置再显示。
- **settings**：打开设置时才创建，关闭就销毁，省内存。

### 3.3 统一抽象：Trigger → Context → Resolver → UI → Action

```
按键/鼠标 ──► Trigger 命中 ──► 并行: 显示UI + 探测 Context
                                   │
                          Resolver(规则, Context) ──► 选出 Wheel/Profile
                                   │
                     用户选择 ──► Action(参数, Context) ──► 执行 / 反馈
```

- **Trigger**：负责“什么时候唤起”。包括按住、单击、双击修饰键、鼠标侧键或中键按住、长按某个键等。
- **Context**：唤起那一刻的环境信息，比如前台应用、窗口标题、选中的文件、当前目录、鼠标位置和所在显示器。
- **Resolver**：按顺序匹配规则，选出要用的轮盘（或其他模块的配置）。
- **Action**：全局动作注册表，所有模块共用，带参数 Schema，可以声明在什么条件下可用。

以后加新模块（比如剪贴板历史），只需要注册新的 Trigger 和 UI，现有的 Action、Context、Resolver 都能直接用。

---

## 4. Core 设计

### 4.1 Module

```rust
pub trait Module: Send + Sync {
    fn manifest(&self) -> ModuleManifest;              // id、名称、版本、需要的系统权限
    fn register(&self, reg: &mut Registry) -> Result<()>; // 注册 triggers/actions/providers/设置 schema
    fn start(&self, ctx: ModuleCtx) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn on_event(&self, ev: &CoreEvent, ctx: &ModuleCtx) {}
}
```

- 模块编译进主程序（v1 不支持动态加载），可以在设置里单独开关。
- `ModuleCtx` 提供：读写配置、发事件、调用 Action、拿到平台服务、操作窗口。
- 每个模块的配置存在 `modules.<id>` 下面，Schema 由模块自己提供，设置页根据 Schema 自动生成表单；也可以给模块写专门的设置页面。

### 4.2 Registry

```rust
pub struct Registry {
    triggers:  TriggerTable,                          // 钩子线程要同步查询，用 ArcSwap 做无锁读取
    actions:   HashMap<&'static str, Arc<dyn ActionHandler>>,
    providers: Vec<Arc<dyn ContextProvider>>,         // 按应用提供上下文（Finder、资源管理器……）
    settings:  Vec<SettingsContribution>,
}
```

### 4.3 Action

```rust
#[async_trait]
pub trait ActionHandler: Send + Sync {
    fn kind(&self) -> &'static str;              // "keystroke" | "launch" | "file.trash" ...
    fn params_schema(&self) -> schemars::Schema; // 设置页据此生成表单
    fn platforms(&self) -> PlatformSet;
    fn available(&self, ctx: &Context) -> bool;  // 条件不满足时在轮盘上显示为灰色
    async fn run(&self, params: &Value, ctx: &Context) -> Result<ActionOutcome>;
}
```

内置动作：

| kind | 说明 |
|---|---|
| `keystroke` | 发送快捷键，支持 `mod`（macOS 上是 Cmd，Windows 上是 Ctrl） |
| `text` | 输入一段文本（逐字注入 Unicode，或者先写剪贴板再粘贴并恢复原剪贴板） |
| `launch` | 启动或切换到某个应用（已经在运行就切到前台） |
| `open` | 用默认程序打开文件、目录或网址 |
| `shell` / `applescript` / `powershell` | 运行外部脚本，可以带 Context 变量 |
| `script` | 运行内置 JS/TS 脚本（§8） |
| `file.*` | 内置文件操作（§6.6） |
| `submenu` | 打开子轮盘 |
| `sequence` | 按顺序执行多个动作，可以在中间加延时 |

执行失败时，用系统通知或轮盘位置上的小提示告诉用户，并写入日志。

### 4.4 Context

```rust
pub struct Context {
    pub app: AppInfo,               // id(bundle id / exe 名), name, pid, window_title
    pub selection: Selection,       // None | Files { paths, dir } | Text(String)  (Text 在 v2 做)
    pub cwd: Option<PathBuf>,       // 文件管理器当前所在目录
    pub cursor: PhysicalPoint,
    pub monitor: MonitorInfo,       // 工作区、缩放比例
    pub platform: Platform,
    pub trigger: TriggerId,
}
```

`ContextProvider` 按应用扩展：v1 内置 Finder 和资源管理器（含桌面）两个；以后可以加 Path Finder、ForkLift、Directory Opus、VSCode（通过 VSCode 扩展通信）、终端（读取当前目录）等。

### 4.5 EventBus

用 `tokio::sync::broadcast` 实现，事件带类型：`TriggerFired`、`ContextReady`、`ConfigChanged`、`ActionFinished`、`PermissionChanged` 等。模块之间只通过事件和 Registry 交互。

---

## 5. 平台抽象层

```rust
pub trait InputHook {
    /// filter 在钩子线程里同步调用，决定这个事件放行还是吞掉，必须是 O(1) 的查表
    fn start(&self, filter: Arc<dyn InputFilter>, sink: Sender<InputEvent>) -> Result<HookHandle>;
}
pub trait Injector {
    fn send_chord(&self, chord: &KeyChord, held: &HeldModifiers) -> Result<()>;
    fn type_text(&self, text: &str) -> Result<()>;
    fn replay(&self, raw: &[RawInput]) -> Result<()>;   // 短按时把吞掉的原始按键重新发出去
}
pub trait OverlayWindow {
    fn show_at(&self, center: PhysicalPoint, shape: &Shape) -> Result<()>;
    fn set_shape(&self, shape: &Shape) -> Result<()>;   // 换子轮盘时更新毛玻璃的形状
    fn hide(&self) -> Result<()>;
}
#[async_trait]
pub trait ContextProbe {
    async fn foreground(&self) -> Result<AppInfo>;
    async fn file_selection(&self, app: &AppInfo, budget: Duration) -> Result<Option<FileSelection>>;
}
pub trait FileOps { fn trash(..); fn set_clipboard_files(.., op: Copy|Cut); fn open_terminal(dir, term); .. }
pub trait Permissions { fn status(&self, p: Permission) -> PermState; fn request(&self, p: Permission); }
pub trait Icons { fn app_icon(&self, id: &str, size: u32) -> Result<PngBytes>; fn file_icon(..); }
```

### 5.1 两个平台的实现对照

| 能力 | macOS 13+ | Windows 11 |
|---|---|---|
| 全局键鼠钩子 | `CGEventTapCreate`（在 Session 层，可以修改或吞掉事件），需要「辅助功能」权限 | `WH_KEYBOARD_LL` + `WH_MOUSE_LL` |
| 钩子被系统停用 | 收到 `kCGEventTapDisabledByTimeout` 后立即重新启用 | 回调超时系统会悄悄摘掉钩子 → 用心跳检测，发现失效就重新安装 |
| 区分自己发出的事件 | 写入 `kCGEventSourceUserData` 标记 | 在 `dwExtraInfo` 写标记，并检查 `LLKHF_INJECTED` |
| 注入按键 | `CGEventPost`，用 `CGEventSetFlags` 精确指定修饰键 | `SendInput`，必要时先补发仍按着的修饰键的抬起事件 |
| 不抢焦点的浮层 | NSPanel + `nonactivatingPanel`，放在 `.popUpMenu` 层级，`canJoinAllSpaces` | `WS_EX_NOACTIVATE \| WS_EX_TOOLWINDOW \| WS_EX_TOPMOST`，用 `SW_SHOWNOACTIVATE` 显示 |
| 毛玻璃 | `NSVisualEffectView` 加 `maskImage`（§7.2） | DWM Acrylic 或 Accent 加窗口区域裁剪，另有截屏模糊方案（§7.2） |
| 前台应用 | `NSWorkspace.frontmostApplication`，窗口标题通过 AX 获取 | `GetForegroundWindow` → `QueryFullProcessImageNameW` |
| 选中文件 | 向 Finder 发 AppleEvents（§6.5） | Shell COM `IShellWindows`（§6.5） |
| 应用图标 | `NSWorkspace.icon(forFile:)` | `SHGetFileInfo` / `IShellItemImageFactory` |
| 开机自启 | `SMAppService`（macOS 13 起提供） | 写入注册表 `Run` 项 |
| 需要的权限 | 辅助功能（必需）、自动化-Finder（文件轮盘需要）、屏幕录制（只有截屏模糊方案需要） | 无需授权；但给管理员窗口发按键会被系统拦截（§10） |

---

## 6. 轮盘模块设计

### 6.1 交互状态机

```
            key down(命中trigger, 吞键)
   Idle ───────────────────────────────► Armed ──(立即)──► Shown
    ▲                                      │                 │ 鼠标移动 → 更新选中
    │         松开 & 时长<tap_ms & 未移出死区 │                 │
    │◄──────── (replay原键 或 进入点击模式) ─┘                 │
    │                                                        │
    │ Esc / 右键 / 在死区内松开 → Cancel                         │
    │◄───────────────────────────────────────────────────────┤
    │                                   松开 & 有选中 → Execute │
    │◄─────────── hide() → 执行 Action ◄──────────────────────┘
```

- **按住模式**（默认）：按住唤起，移动鼠标选择，松开执行。
- **点击模式**：短按后轮盘不关闭，鼠标左键点击执行，右键或 Esc 取消。
- **短按的处理**（`tap_ms`，默认 180ms）：可以配置成三种行为之一——①把原按键原样发出去，不影响这个键的本来用途；②进入点击模式；③什么都不做。
- 轮盘显示期间，钩子吞掉 Esc 和鼠标点击，避免它们传给下面的应用。

### 6.2 选择算法

- 选中判断**全部在 Rust 里做**（只有一个判断来源，保证松开时执行的就是用户看到的那一格）。UI 只负责画。
- 用鼠标相对于**轮盘圆心**的向量计算角度，然后算出扇区：`index = floor(((θ - θ0) mod 2π) / (2π/n))`。如果轮盘贴边时被挪动过，圆心就不在按下的位置，所以必须以圆心为准。
- 中心有一块死区（默认半径 36px），在死区里没有选中项，松开就是取消。
- 扇区边界有约 3° 的滞回区间，鼠标在边界附近时选中项不会来回跳。
- 鼠标移出轮盘范围后仍然按角度选择（甩一下就能选中）；鼠标位置由钩子跟踪，不依赖窗口边界。
- 子轮盘：在 `submenu` 那一格上越过外圈，或者停留 300ms，就展开子轮盘；中心变成「返回」。
- 每个轮盘 2–12 格，建议不超过 8 格。
- 鼠标位置以约 120Hz 的频率节流，通过 Tauri `Channel` 推给 UI，内容是 `{hover, angle, dist}`，UI 用它画辉光跟随鼠标的效果。

### 6.3 唤起时序与延迟预算

```
t0  key down（钩子线程，<1ms 内决定吞掉）
    ├─► 主线程: 计算位置（贴边时挪回屏幕内）→ set_shape → show_at        目标 ≤ 30ms
    └─► tokio: ContextProbe.foreground()  (~1ms)
               └─ 前台是文件管理器 → file_selection(预算 80ms)
t0+~16ms  overlay 先用「按前台应用匹配」的结果渲染
t0+≤80ms  选中文件的结果回来 → 重新匹配 → 如果换成文件轮盘就做 120ms 交叉淡入淡出
          超出预算还没回来 → 保持当前轮盘，后台查询结果作废
```

优化手段：overlay 常驻内存；AppleScript 只编译一次后缓存；COM 接口对象复用；应用图标在启动时预加载并缓存成 PNG data URL。

### 6.4 规则匹配（Resolver）

- 按顺序匹配，第一条满足条件的规则生效。可以用的条件：
  - `app`：bundle id 或 exe 名，支持通配符，可以按平台分别写
  - `title`：窗口标题正则
  - `selection`：`none | files | folder | text`
  - `ext`：选中文件的扩展名
  - `count`：选中数量范围
  - `platform`
- 轮盘可以 `extends` 另一个轮盘：在基础轮盘上增加、删除或替换某几格（按格子的 `id`），这样按应用定制时不用从头复制一份。

### 6.5 选中文件探测

**macOS（Finder）**
- 用缓存的 `NSAppleScript`（或直接构造 AppleEvent）获取 `selection` 和 `insertion location`（前台窗口所在目录）。桌面获得焦点时，selection 里就是桌面上选中的项。
- 在 Info.plist 里声明 `NSAppleEventsUsageDescription`，签名权限里加 `com.apple.security.automation.apple-events`。第一次使用时系统会弹出「自动化」授权框。
- 一次查询约 50–150ms，所以要按 §6.3 的预算异步执行。

**Windows（资源管理器）**
- 前台窗口类名是 `CabinetWClass` 时：遍历 `IShellWindows`，找到对应前台窗口的那个。
  - **Win11 多标签页**：一个窗口下有多个 `ShellTabWindowClass`。要找到当前**可见**的那个标签，再和 `IShellBrowser` → `IOleWindow::GetWindow` 返回的窗口句柄比对，才能拿到正确标签页的 `IFolderView2`。
- 前台是桌面（`Progman` / `WorkerW`）时：用 `FindWindowSW(SWC_DESKTOP)` 获取桌面的视图。
- 用 `IFolderView2::GetSelection` 拿选中项，用 `IPersistFolder2` 拿当前目录。
- 所有 COM 调用放在专门的 STA 线程里执行，不阻塞钩子线程。

### 6.6 文件轮盘预置操作

| 操作 | macOS | Windows |
|---|---|---|
| 在此打开终端 | `open -a <Terminal\|iTerm\|Warp> <dir>`，终端可配置 | `wt.exe -d <dir>`，可换成 pwsh 或 cmd |
| 复制 | 把文件 URL 写入剪贴板 | 写入 `CF_HDROP` 格式 |
| 剪切 | 写入剪贴板并做「待移动」标记，在 Finder 里用 ⌥⌘V 完成移动；轮盘里的「粘贴到此」也会按移动处理 | 写入 `CF_HDROP` + `Preferred DropEffect = MOVE`，资源管理器原生支持 |
| 粘贴到此 | 在当前目录执行复制或移动（内部实现） | 同左，或者直接发 Ctrl+V |
| 删除 | 移到废纸篓（`trash`），可以设置删除前确认 | 用 `IFileOperation` + `FOFX_RECYCLEONDELETE` 移到回收站 |
| 复制路径 | 每个路径一行 | 同左（可选是否加引号） |
| 压缩 | `ditto -c -k` | `tar -a -c -f x.zip`（Win11 自带 tar） |
| 用…打开 | 子轮盘：常用应用列表 | 同左 |

### 6.7 配置示例

```yaml
version: 1

settings:
  theme: auto            # auto | dark | light
  accent: system         # system | "#7C5CFF"
  terminal: { mac: iterm, win: wt }

modules:
  wheel:
    triggers:
      - id: main
        kind: hold
        keys: { mac: "alt+space", win: "capslock" }
        tap: replay               # replay | click-mode | none
    profiles:
      - when: { selection: files }
        wheel: files
      - when: { app: { mac: com.microsoft.VSCode, win: Code.exe } }
        wheel: vscode
      - wheel: default

    wheels:
      default:
        items:
          - { id: shot, label: 截图, icon: sf:crop, action: { type: keystroke, keys: { mac: "cmd+shift+4", win: "win+shift+s" } } }
          - { id: term, label: 终端, icon: app:auto, action: { type: launch, target: { mac: com.googlecode.iterm2, win: wt.exe } } }
          - { id: tools, label: 工具, icon: lucide:wrench, action: { type: submenu, wheel: tools } }
          - { id: daily, label: 日报, icon: lucide:file-text, action: { type: script, file: scripts/daily.ts } }
      vscode:
        extends: default
        add:
          - { id: palette, label: 命令面板, action: { type: keystroke, keys: "mod+shift+p" } }
        remove: [shot]
      files:
        items:
          - { id: term,  label: 打开终端, action: { type: file.open_terminal } }
          - { id: cut,   label: 剪切,     action: { type: file.cut } }
          - { id: copy,  label: 复制,     action: { type: file.copy } }
          - { id: paste, label: 粘贴到此, action: { type: file.paste_here } }
          - { id: path,  label: 复制路径, action: { type: file.copy_path } }
          - { id: zip,   label: 压缩,     action: { type: file.zip } }
          - { id: del,   label: 删除,     action: { type: file.trash, confirm: true } }
```

---

## 7. 视觉设计与实现

### 7.1 视觉层次（从下到上）

1. **系统原生毛玻璃**：模糊窗口后面的桌面内容（§7.2）。
2. **色调层**：深色模式 `rgba(20,20,28,.35)`，浅色模式 `rgba(255,255,255,.30)`，再叠一层 2–3% 的噪点纹理，避免大面积纯色显得廉价。
3. **扇区**：用 SVG path 画，扇区间留 2–3px 的缝，圆角约 10px；画一条 1px 的内高光描边（上亮下暗，有立体感）。
4. **高亮与辉光**：选中的扇区外沿有一圈流动的辉光（§7.3），扇区本身轻微提亮，并向外移动约 4px。
5. **中心圆**：显示当前选中项的图标、名称和快捷键提示；没有选中时显示当前轮盘的名称或「返回」。

尺寸（逻辑像素）：外半径 150，内半径 56，图标 28，窗口 420×420（给辉光留出边距）。

### 7.2 毛玻璃方案

CSS 的 `backdrop-filter` 模糊不到窗口后面的桌面，必须用系统能力。轮盘的形状统一由 Rust 里的 `WheelGeometry` 计算，毛玻璃的裁剪形状和 UI 画的扇区用同一份数据，保证完全对齐。

**macOS**
- 在 WebView 下面垫一层 `NSVisualEffectView`（`material = .hudWindow` 或 `.popover`，`blendingMode = .behindWindow`，`state = .active`），用 `maskImage` 裁成圆形。
- 形状变化（比如切换子轮盘）时重新生成遮罩；遮罩图按屏幕缩放比例生成，边缘自带抗锯齿。
- Tauri 需要开启 `macOSPrivateApi` 才能使用透明窗口。我们本来就不上 App Store，没有影响。

**Windows 11**（技术验证阶段比较以下两种，选效果好的）
- **方案 A：系统 Acrylic**。用 `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW`，或者 `SetWindowCompositionAttribute(ACCENT_ENABLE_ACRYLICBLURBEHIND)`，再用 `SetWindowRgn` 裁成圆形。缺点是窗口区域裁剪没有抗锯齿，要靠 UI 画的 1px 描边和辉光把边缘盖住。
- **方案 B：截屏模糊**。唤起前（浮层还隐藏时）用 `BitBlt` 或 Windows.Graphics.Capture 截取轮盘下面那一小块屏幕，在 GPU 上模糊后作为背景（WebGL 或 CSS）。Windows 截屏不需要权限；边缘可以完美抗锯齿，模糊程度和色彩也能完全自己控制。代价是背景是静止的：轮盘只出现一瞬间，这一般感觉不到。
- 如果 B 的效果明显更好，Windows 就默认用 B，A 作为降级方案。macOS 上截屏需要「屏幕录制」权限，所以 macOS 只用原生方案。

### 7.3 边缘流动辉光

- **CSS 实现**（先做）：给选中的扇区外沿加一层描边，填充用 `conic-gradient(from var(--angle), …)`，配合 `@property --angle` 做旋转动画（2.4s 转一圈）。用 mask 只露出描边的部分，再叠一层带 `blur(10px)` 的副本作为外发光。渐变的起始角度跟随鼠标方向，看起来像光在追着鼠标走。
- **WebGL 实现**（如果 CSS 版本卡顿或不够细腻）：用一个全窗口的 canvas，fragment shader 根据到圆环边缘的距离场（SDF）画出流动的光带，可以加噪声扰动、HDR 式的光晕衰减；所有扇区共用这一个 canvas。WKWebView 里大面积动画的 `filter: blur` 开销比较高，到时候需要实测对比。
- 辉光颜色默认用系统强调色，也可以自定义成双色渐变，比如 `#7C5CFF → #00D1FF`。

### 7.4 动效

| 场景 | 参数 |
|---|---|
| 唤起 | scale 0.86→1，opacity 0→1，弹簧缓动约 140ms；每一格依次出现，间隔 12ms |
| 选中切换 | 高亮跟着角度平滑移动约 90ms，辉光跟随有轻微惯性 |
| 切换子轮盘 | 当前轮盘缩小淡出，子轮盘从中心展开，约 160ms |
| 关闭 | 执行时被选中的那一格亮一下（60ms），然后整体淡出（80ms） |
| 减少动态效果 | 跟随系统「减少动态效果」设置，关闭缩放和流动，只保留淡入淡出 |

### 7.5 主题与图标
- 跟随系统深色/浅色模式，可以手动覆盖；强调色读取系统设置。
- 图标来源：`app:<id>`（从系统读取应用图标）、`sf:<name>`（只在 macOS 上可用，Windows 上自动换成对应的 Lucide 图标）、`lucide:<name>`、自定义 PNG/SVG 路径。
- 设置页沿用同一套设计语言：毛玻璃侧边栏、轻描边、大圆角；macOS 上用 vibrancy 侧边栏，Windows 上用 Mica 窗口背景。

---

## 8. 脚本系统

- **引擎**：`rquickjs`。每次执行创建一个独立的 Context，默认超时 30s（通过中断回调实现），内存上限默认 64MB。
- **TypeScript**：执行前用 `oxc` 去掉类型标注，不支持 import 打包（v1 只支持单文件，或者相对路径引入 `.js` / `.ts` 文件）。同时提供 `anytools.d.ts`，在编辑器里有类型提示和补全。
- **API（暂定）**：

```ts
context            // 唤起时的 Context（app/selection/cwd…）
keys.send("mod+c"); keys.type("hello")
clipboard.readText() / writeText() / readFiles() / writeFiles(paths, {cut})
app.launch(id) / app.activate(id) / app.frontmost()
fs.read / write / list / exists / trash         // 受权限约束
shell.exec(cmd, {cwd, timeout}) → {code, stdout, stderr}
http.fetch(url, init)                           // 受权限约束
ui.toast(msg) / ui.wheel.open(id) / ui.pick(items)   // pick: 用轮盘让用户选择
sleep(ms); log.info(...)
```

- **权限**：脚本在文件开头用注释声明需要的权限，例如 `// @permissions fs:~/Documents shell http:api.example.com`。第一次运行时弹窗请用户授权，授权结果按脚本内容的哈希记录，脚本内容变了要重新授权。
- 外部脚本（Shell / AppleScript / PowerShell）以子进程方式运行，Context 通过环境变量传入（`ANYTOOLS_SELECTION`、`ANYTOOLS_CWD`、`ANYTOOLS_APP`……）。

---

## 9. 配置系统

- **存放位置**：macOS 在 `~/Library/Application Support/anyTools/`，Windows 在 `%APPDATA%\anyTools\`。

```
config.yaml          # 全局设置 + 各模块配置
wheels/*.yaml        # 也可以把轮盘拆成单独的文件
scripts/             # 用户脚本
icons/               # 自定义图标
state/               # 运行状态（权限授权记录、缓存），不建议手动编辑
```

- **热加载**：用 `notify` 监听文件变化，防抖 200ms 后重新加载。解析或校验失败时保留上一份有效配置，并提示具体的出错位置。
- **Schema**：用 `schemars` 生成 JSON Schema，在 VSCode 里手动编辑配置时也有补全和校验。
- **版本迁移**：配置里有 `version` 字段，启动时按版本号依次执行迁移函数，迁移前自动备份原文件。
- **设置页写回**：设置页修改后会重写配置文件，**YAML 里的注释会丢失**。所以设置页顶部要提示：「手动编辑过的配置被设置页保存后，注释会被移除」。

---

## 10. 权限、安全与限制

| 事项 | 处理 |
|---|---|
| macOS 辅助功能权限 | 第一次启动时进入引导页，一键跳转到系统设置，授权后自动检测生效（轮询 `AXIsProcessTrusted`） |
| 权限和签名绑定 | 签名一变，系统就认为是另一个应用，权限会失效。正式版必须固定使用 Developer ID 签名；**开发时用一张固定的自签名证书**，不然每次重新编译都要重新授权 |
| macOS 安全输入 | 光标在密码框里时系统开启 Secure Event Input，事件钩子收不到按键。检测到这种情况时，在托盘或菜单栏图标上提示 |
| Windows 管理员窗口 | 普通权限运行时，前台是管理员窗口就收不到快捷键，也发不了按键过去（系统的 UIPI 隔离）。提供「以管理员运行」选项（通过任务计划程序实现开机自启）；UIAccess 方案需要签名并安装在 Program Files，以后再评估 |
| 单独按 Alt 当触发键（Windows） | 吞掉 Alt 后系统可能激活窗口菜单栏。需要在抬起前注入一个未分配的虚拟键（`VK 0xE8`，AutoHotkey 用的就是这个办法）来屏蔽 |
| 脚本安全 | 按权限声明授权，超时可中断，只能访问授权过的路径；不会自动运行从外部导入的脚本 |
| 隐私 | 默认不联网，只有检查更新时会访问网络；日志里不记录选中的文本和文件内容 |

---

## 11. 工程结构

```
anyTools/
├── Cargo.toml                 # workspace
├── crates/
│   ├── core/                  # Module/Registry/EventBus/Config/Resolver/Action trait
│   ├── platform/              # traits + mock 实现
│   │   ├── src/macos/         # objc2 / CG / AppleEvents
│   │   └── src/windows/       # windows-rs / COM / DWM
│   ├── script/                # rquickjs host + API 绑定 + oxc strip
│   ├── actions/               # 内置 Action 实现
│   └── modules/
│       └── wheel/             # 轮盘模块：状态机、几何、匹配逻辑
├── app/                       # Tauri 主程序（src-tauri）：托盘、窗口、IPC、引导页
├── ui/
│   ├── overlay/               # 轮盘浮层（无组件库）
│   ├── settings/              # 设置页
│   └── shared/                # 设计 token、图标、IPC 类型（tauri-specta 生成）
├── docs/                      # M0 验证结果等文档
└── docs/
```

---

## 12. 构建、签名与分发

| 平台 | 产物 | 签名 / 公证 | 备注 |
|---|---|---|---|
| macOS | Universal `.dmg` | Developer ID + Hardened Runtime + `notarytool` 公证 | 不开沙盒；签名权限加 apple-events；只能自己分发，不上 App Store |
| Windows | NSIS 安装包（按当前用户安装，不需要管理员权限） | Azure Trusted Signing | Win11 自带 WebView2；ARM64 是否首发待定 |

- **CI**：GitHub Actions，在 `macos-latest` 和 `windows-latest` 上分别构建，使用 `tauri-action`。正式版本在打 tag 时自动签名并发布到 GitHub Releases。
- **自动更新**：`tauri-plugin-updater`，更新包有单独的签名；分 stable 和 beta 两个更新渠道。
- **体积目标**：安装包 < 15MB。空闲内存：macOS < 80MB，Windows < 120MB（主要是 WebView2 占用）。空闲时 CPU ≈ 0%。

---

## 13. 测试策略

- **单元测试**（覆盖重点）：几何与选择算法（含滞回和贴边挪动后的圆心）、Resolver 匹配、`extends` 合并、配置解析与迁移、状态机。状态机用假时钟和假输入事件测试。
- **集成测试**：用 `platform` 的 mock 实现跑完整流程：按键 → 匹配 → 执行。
- **平台冒烟测试**：维护一份手动检查清单，在两个平台上逐项验证：多显示器、混合缩放比例、全屏应用、多桌面空间（Spaces）、管理员窗口、密码框、资源管理器多标签页、桌面选中。
- **性能**：用 `tracing` 记录关键耗时（按下 → 显示、按下 → 拿到 Context、松开 → 执行完成），开发版里显示实时耗时。
- **视觉**：用 Ladle 或 Storybook 单独开发和预览轮盘组件，可以模拟输入数据。

---

## 14. 里程碑

macOS 上 M1–M5 已实现。Windows 的系统集成还没做。公证需要 Developer ID 证书，目前没有。

**M0 技术验证（最关键，任何一项不通过都要重新评估方案）**

| # | 验证项 | 通过标准 |
|---|---|---|
| P1 | 按住吞键 + 不抢焦点的浮层 | 两个平台上，按下到浮层出现 ≤ 50ms；前台应用不失去焦点；松开后发出的快捷键能被原应用正确接收 |
| P2 | 圆形毛玻璃 | macOS 用 maskImage 裁成圆形；Windows 方案 A 和 B 截图对比，选定一个 |
| P3 | 选中文件探测 | Finder、资源管理器（含多标签页）、桌面三种场景 p95 ≤ 80ms |
| P4 | 流动辉光性能 | 120Hz 屏幕上 CSS 版本不掉帧；否则改用 WebGL |

macOS 的结果见 [docs/M0-RESULTS.md](docs/M0-RESULTS.md)：吞键、不抢焦点、Finder 读取和 60Hz 下的辉光通过；关掉 WebView 的遮挡检测并提前画好默认轮盘后，首帧估算 p95 23ms。向其他应用发按键、WebGL、120Hz 没测，Windows 没做。

与方案的差异：发按键、剪贴板、启动应用、通知不需要权限声明；`http.fetch` 是同步的，返回普通对象。

**M1 核心框架**：Module / Registry / EventBus / ConfigStore（热加载）/ Resolver / 平台 trait 与 mock 实现、托盘或菜单栏图标、权限引导页。

**M2 轮盘第一版**：状态机、选择算法、三类轮盘、内置 Action、`file.*` 文件操作、视觉效果完整实现。

**M3 设置与脚本**：可视化轮盘编辑器、规则编辑、触发键录制、QuickJS 脚本运行时及 API、脚本权限授权。

**M4 分发**：签名、公证、自动更新、CI 发布流程、开机自启、崩溃日志。

**M5 扩展性验证**：第二个模块是剪贴板历史（`crates/modules/clipboard`）。只记文字，存在 `state/clipboard.json`（权限 600），跳过标记为敏感或临时的内容、忽略列表里的应用，以及 anyTools 自己借用剪贴板造成的变化。⌃⌘V 打开列表面板，`clipboard.recent` 可以放进任何轮盘。

为此加到核心的：动作可以是菜单（`is_menu` / `menu`），打开时才算出选项；`InputEvent` 带上按键字符和滚轮；模块有会话时独占按下类事件（`Module::exclusive`）；浮层形状支持圆角矩形。规则支持 `selection: text`，`open` 支持 `{text}`。

---

## 15. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| Windows 圆形毛玻璃边缘有锯齿或效果差 | 视觉效果 | 方案 B（截屏模糊）兜底 |
| 非公开 API（Accent）在以后的系统更新里失效 | 毛玻璃效果 | 优先用公开的 DWM API，非公开 API 只作为可选方案；失效时降级为纯色半透明 |
| Finder 的 AppleEvents 偶尔很慢（Finder 卡住时） | 文件轮盘出现得慢 | 设置时间预算，超时就降级为默认轮盘，后台查询结果直接丢弃 |
| Win11 资源管理器多标签页导致读错选中项 | 对错误的文件执行了操作 | 用当前可见的标签页窗口句柄严格比对；对不上就当作没有选中文件，**宁可不显示文件轮盘，也不能操作错误的文件** |
| 钩子回调超时被系统摘掉 | 快捷键失效 | 回调只查表；心跳检测，失效就重新安装 |
| 开发时签名变化导致权限反复失效 | 开发效率 | 使用固定的自签名开发证书 |
| WebView 冷启动慢 | 第一次唤起慢 | overlay 常驻内存；启动后先在屏幕外渲染一次预热 |

---

## 16. 待确认事项（已给默认值，可随时调整）

| # | 事项 | 默认值 / 建议 |
|---|---|---|
| 1 | 产品名与 Bundle ID | `anyTools` / `com.anytools.app`（需要确认归属的域名） |
| 2 | Windows ARM64 | 首发只做 x64，ARM64 作为后续 |
| 3 | 默认终端 | macOS：Terminal（检测到 iTerm/Warp 时提示可以切换）；Windows：Windows Terminal |
