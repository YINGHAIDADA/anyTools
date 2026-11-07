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

- 配置文件：`~/Library/Application Support/com.anytools.app/config.yaml`。可以在设置窗口里可视化编辑，也可以手动编辑（保存后自动生效；写错时继续使用上一份正确的配置）。从设置窗口保存会重写整个文件，手写的注释不保留，上一版在 `config.yaml.bak`
- 脚本：同目录下的 `scripts/`，见下文
- 日志与崩溃记录：`~/Library/Logs/com.anytools.app/`（只保存在本机）
- 自测：`ANYTOOLS_SELFTEST=6 anyTools.app/Contents/MacOS/anytools`。它走和键盘钩子相同的代码路径唤起轮盘、展开子轮盘、再取消，不会执行任何动作，结果写在日志里

## 脚本

在轮盘的一格里选「JS / TS 脚本」，文件放在 `scripts/` 下。脚本在内嵌的 QuickJS 里运行：

- 以 async 函数执行，可以直接 `await` / `return`；`.ts` 文件会去掉类型后运行（不做类型检查）
- 每次运行是独立的环境：默认超时 30 秒、内存上限 64MB
- API：`context`、`keys`、`clipboard`、`app`、`ui`、`fs`、`shell`、`http`、`sleep`、`console`。`scripts/anytools.d.ts` 提供编辑器补全，`scripts/example.ts` 是示例
- 文件、命令、网络需要在脚本开头声明，如 `// @permissions fs:~/Documents shell http:api.example.com`。第一次运行和每次修改脚本后会请你确认；未声明的访问会报错

## 发布

```bash
APPLE_SIGNING_IDENTITY="<签名身份>" ./scripts/release-macos.sh
```

脚本先跑全部测试，再构建、签名、校验，按环境变量决定做哪些步骤（详见脚本开头的说明）：

| 步骤 | 需要 | 缺少时 |
|---|---|---|
| 签名 | `APPLE_SIGNING_IDENTITY` | 不能运行 |
| 公证 | Developer ID Application 证书 + `APPLE_API_KEY`/`APPLE_API_ISSUER`/`APPLE_API_KEY_PATH`（或 `APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID`） | 跳过；包只能在本机直接运行 |
| 更新包签名 | `TAURI_SIGNING_PRIVATE_KEY_PATH`（本机在 `~/.tauri/anytools-updater.key`） | 不生成更新包 |
| 应用内更新 | `ANYTOOLS_UPDATE_URL`（latest.json 的 https 地址） | 该版本的应用内更新关闭 |

GitHub 上推送 `v*` 标签会触发 [.github/workflows/release.yml](.github/workflows/release.yml)：构建 Universal 包、签名（有证书时公证），并把 DMG、更新包和 `latest.json` 发布到草稿 Release。需要的 Secrets 写在工作流文件开头。更新包的公钥在 `app/tauri.conf.json`，私钥**不在仓库里**，丢失后已发布的版本将无法再自动更新。

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
| `crates/core` | Module / Registry / EventBus / ConfigStore（热加载、保存）/ 规则匹配 / 平台 trait |
| `crates/platform` | macOS 实现 + 用于测试的 mock |
| `crates/actions` | 内置动作：快捷键、文本、启动、打开、外部脚本、序列、`file.*` |
| `crates/script` | 内嵌 JS/TS 脚本：QuickJS 运行时、API、权限声明与授权 |
| `crates/modules/wheel` | 轮盘模块：配置编译、状态机、几何 |
| `app` | Tauri 主程序：菜单栏图标、浮层、设置窗口、开机启动、崩溃记录、更新、默认配置 |
| `ui` | 浮层页面（纯 TS）和设置窗口（React，含轮盘与规则编辑器） |
| `scripts` | 发布脚本 |
| `poc` | M0 技术验证（临时代码） |

GPL-3.0
