# anyTools M0 技术验证（macOS 部分）

临时验证代码，验证完就丢弃或重写，不进入正式工程。对应 [TECH_DESIGN.md](../TECH_DESIGN.md) §14 的 P1–P4。

## 运行

```bash
npm install
APPLE_SIGNING_IDENTITY="E5CE64D80FD46DDD19D0A3ED188165E5A815FE9F" npx tauri build --bundles app
open "src-tauri/target/release/bundle/macos/anyTools PoC.app"
```

必须用**固定的签名身份**打包后运行：辅助功能权限绑定在签名上，`tauri dev` 每次编译签名都会变，权限会反复失效。

启动后会打开「M0 技术验证」面板：
1. 点「请求授权」，在系统设置里打开 anyTools PoC 的辅助功能开关。面板会自动检测并启动钩子。
2. 按面板里「手动检查」一节操作，结果表会实时统计。
3. 原始数据写在 `~/Library/Logs/com.anytools.poc/m0-metrics.jsonl`。

关闭面板只是隐藏，程序继续在后台运行；要退出请点面板右上角「退出」，或在轮盘里选「PoC 面板」重新打开。

## 验证项与测量方式

| # | 验证项 | 怎么测 | 通过标准 |
|---|---|---|---|
| P1 | 按住吞键 + 不抢焦点 | 钩子记录内核事件时间戳 → 回调 → 原生窗口显示 → WebView 连续两帧 rAF 后上报；唤起 150ms 后检查本程序是否变成前台 | 按键到画面 p95 ≤ 50ms；前台应用不变；发出的按键被原应用收到 |
| P2 | 圆形毛玻璃 | `NSVisualEffectView` + `maskImage`，形状与 SVG 用同一公式计算 | 肉眼检查：边缘平滑、和描边对齐、在各种背景上都好看 |
| P3 | Finder 选中文件 | 预编译的 `NSAppleScript`，放在专用线程执行 | 唤起时读取 p95 ≤ 80ms |
| P4 | 流动辉光 | overlay 里用 rAF 统计帧间隔，CSS 和 WebGL 两种渲染分别统计 | 不掉帧（丢帧率 ≤ 1%） |

## 代码结构

- `src-tauri/src/wheel.rs`：与平台无关的部分，包括几何计算、选择算法（死区、滞回）、贴边挪动、按住/点击状态机。有单元测试：`cargo test`。
- `src-tauri/src/mac.rs`：CGEventTap、按键注入、不抢焦点的浮层窗口和毛玻璃、Finder 读取。
- `src-tauri/src/main.rs`：把各部分连起来，负责计时上报和 IPC。
- `src/overlay.*`：轮盘界面（SVG、CSS 辉光、WebGL 辉光）。
- `src/panel.*`：验证面板。

## 未覆盖

- Windows 部分的 P1–P4 需要在 Windows 11 机器上另外实现和验证。
- 多标签页、桌面选中等边界情况只在手动检查清单里覆盖。
