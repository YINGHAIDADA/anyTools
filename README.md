# anyTools

常驻菜单栏的日常效率工具。第一个功能是**快捷轮盘**：按住 ⌥Space，以鼠标为中心弹出轮盘，移到一格上松开就执行。设计见 [TECH_DESIGN.md](TECH_DESIGN.md)。

当前只实现了 macOS；Windows 的 trait 已经定义好，等完成 Windows 部分的 M0 验证后再实现。

## 构建与运行（macOS）

```bash
cd ui && npm install
APPLE_SIGNING_IDENTITY="<你的签名身份>" npm run tauri -- build --bundles app
open ../target/release/bundle/macos/anyTools.app
```

必须用**固定的签名身份**打包：辅助功能权限是按签名记录的，签名一变就要重新授权。第一次启动会打开设置窗口，引导你授予权限。

- 配置文件：`~/Library/Application Support/com.anytools.app/config.yaml`（保存后自动生效；写错时继续使用上一份正确的配置）
- 日志：`~/Library/Logs/com.anytools.app/`
- 自测：`ANYTOOLS_SELFTEST=6 anyTools.app/Contents/MacOS/anytools`。它走和键盘钩子相同的代码路径唤起轮盘、展开子轮盘、再取消，不会执行任何动作，结果写在日志里

## 测试

```bash
cargo test --workspace
```

```bash
cd ui && npm run typecheck
```

## 工程结构

| 目录 | 内容 |
|---|---|
| `crates/core` | Module / Registry / EventBus / ConfigStore（热加载）/ 规则匹配 / 平台 trait |
| `crates/platform` | macOS 实现 + 用于测试的 mock |
| `crates/actions` | 内置动作：快捷键、文本、启动、打开、脚本、序列、`file.*` |
| `crates/modules/wheel` | 轮盘模块：配置编译、状态机、几何 |
| `app` | Tauri 主程序：菜单栏图标、浮层、设置窗口、默认配置 |
| `ui` | 浮层页面（纯 TS）和设置窗口（React） |
| `poc` | M0 技术验证（临时代码） |

GPL-3.0
