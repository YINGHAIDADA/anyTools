# anyTools

常驻菜单栏的日常工具。按住 ⌥Space，以鼠标为中心弹出轮盘，移到一格上松开就执行。选中了文件、选中了文字，或者前台是访达、浏览器、编辑器时，弹出的轮盘不同。设计见 [TECH_DESIGN.md](TECH_DESIGN.md)。

macOS 13 及以上可用。Windows 的安装包会随版本一起构建，系统集成还没做，现在不能当日常工具用。

## 下载

当前版本是 [v0.1.0](https://github.com/YINGHAIDADA/anyTools/releases/tag/v0.1.0)。

- macOS：[anyTools_0.1.0_universal.dmg](https://github.com/YINGHAIDADA/anyTools/releases/download/v0.1.0/anyTools_0.1.0_universal.dmg)（Apple Silicon 和 Intel）
- Windows：[anyTools_0.1.0_x64-setup.exe](https://github.com/YINGHAIDADA/anyTools/releases/download/v0.1.0/anyTools_0.1.0_x64-setup.exe)（x64，功能未完成）

这两个包都没有用对外分发的证书签名。macOS 上第一次打开，到「系统设置 › 隐私与安全性」里点「仍要打开」。Windows 上 SmartScreen 会拦截。装好后授予辅助功能权限；签名一变，要重新授权。

## 用法

从菜单栏图标打开设置。默认：

- 按住 ⌥Space 唤起轮盘，移到一格上松开执行。短按则保持打开，再点击选择
- ⌃⌘V 打开剪贴板历史，只记文字，只存在本机。轮盘上的「剪贴板」展开最近几条
- 选中文件时是文件轮盘，选中文字时是文字轮盘

配置在 `~/Library/Application Support/com.anytools.app/config.yaml`。保存后自动生效；写错时继续用上一份正确的配置。从设置里保存会重写整个文件，手写的注释不保留，上一版在 `config.yaml.bak`。日志在 `~/Library/Logs/com.anytools.app/`。

## 脚本

轮盘的一格可以跑配置目录下 `scripts/` 里的 JS 或 TS。脚本在内嵌的 QuickJS 里执行，当成 async 函数，可以直接 `await` 和 `return`。`.ts` 只去掉类型，不做类型检查。每次运行是独立环境，默认超时 30 秒、内存上限 64MB。

API 有 `context`、`keys`、`clipboard`、`app`、`ui`、`fs`、`shell`、`http`、`sleep`、`console`。编辑器补全在同目录的 `anytools.d.ts`，`example.ts` 是示例。

读写文件、运行命令、访问网络要在脚本开头声明，例如 `// @permissions fs:~/Documents shell http:api.example.com`。第一次运行，以及脚本改过之后，会再问一次。没声明的访问直接报错。

## 从源码构建

只支持在 macOS 上构建可运行的应用。用固定的签名身份，否则辅助功能权限每次都要重新授。

```bash
cd ui && npm install
APPLE_SIGNING_IDENTITY="<签名身份>" npm run tauri -- build --bundles app
open ../target/release/bundle/macos/anyTools.app
```

本机发布（先跑测试，再构建并签名；有凭据才公证）：

```bash
APPLE_SIGNING_IDENTITY="<签名身份>" ./scripts/release-macos.sh
```

推送和 `app/tauri.conf.json` 版本号一致的 `v*` 标签，会走 [.github/workflows/release.yml](.github/workflows/release.yml)：打出 macOS Universal 包和 Windows x64 安装包，上传到草稿 Release。需要的 Secrets 写在工作流开头。更新包用 `~/.tauri/anytools-updater.key` 签名，公钥在 `app/tauri.conf.json`。私钥不在仓库里，丢了之后，已经发出的版本就不能再自动更新。

## 测试

```bash
cargo test --workspace
```

```bash
cd ui && npm run typecheck
```

## 目录

| 目录 | 内容 |
|---|---|
| `crates/core` | 模块、动作注册、事件、配置、规则匹配 |
| `crates/platform` | macOS 实现，以及测试用的 mock |
| `crates/actions` | 内置动作 |
| `crates/script` | JS/TS 脚本运行时和权限 |
| `crates/modules/wheel` | 轮盘 |
| `crates/modules/clipboard` | 剪贴板历史 |
| `app` | 主程序：菜单栏、浮层、设置、开机启动、崩溃记录、更新 |
| `ui` | 浮层和设置窗口 |
| `docs` | M0 验证结果 |

GPL-3.0
