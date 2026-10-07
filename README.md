# 速网 SuNet

[![最新版本](https://img.shields.io/github/v/release/zhcx/SuNet?sort=semver&label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/zhcx/SuNet/releases)
[![Release 工作流](https://github.com/zhcx/SuNet/actions/workflows/release.yml/badge.svg)](https://github.com/zhcx/SuNet/actions/workflows/release.yml)
[![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%2010%2F11%20x64%20%7C%20macOS%2010.15%2B-0078D4)](#下载)

网络配置切换器：把 **hosts 托管区块 + 系统代理 + 系统 DNS（IPv4/IPv6 分族）**
打包成一个「方案」，一键整体切换；任何一步失败就按逆序回滚，不会停在"半套配置"上。

技术栈 Rust + Tauri 2，目标平台 Windows 10 / 11 x64 与 macOS 10.15+（Apple Silicon + Intel 通用二进制）；
设计依据是 `SuNet-设计方案.md` v1.5。

---

## 功能

- **方案三件套原子切换**：hosts、系统代理、DNS 作为一个整体生效，失败逆序回滚并留下快照。
- **任务栏快捷面板**：常驻托盘，热键或托盘左键唤出，贴边定位、高度自适应、`Esc` 或失焦自动收起。
- **全局热键**：默认在「直连 ↔ 上一个方案」之间切换，改绑时直接按组合键捕获，不用手写字符串。
- **Hosts 编辑**：托管区块隔离（只改 `# su.net` 起止标记之间）、语法校验、远程订阅导入（镜像 + 逐条校验 + diff 确认）。
- **代理设置**：HTTP / HTTPS / SOCKS、绕过列表、动态清零与撤销。
- **DNS 管理**：常用地址预设表、按网卡写入 IPv4 / IPv6、DoH / DoT 端点参考表。
- **无边框主界面**：自绘顶栏，拖动、双击最大化、最小化、隐藏到托盘。
- **可观测与可恢复**：日志按天切分 + 敏感信息脱敏 + 一键导出诊断包；配置快照；异常退出检测后给出差异与选择。
- **权限模型（模型 C）**：主进程以普通权限常驻，只在真正需要写 hosts / DNS（macOS 还包括系统代理）时按需提权；
  macOS 上优先走常驻的后台助手（装一次免密码），否则弹一次系统授权框。
- **分平台平台层**：Windows 走 Win32 / 注册表 / `netsh`；macOS 走 `networksetup` / `scutil` / `/etc/hosts`，
  界面文案随平台切换（UAC ↔ 系统授权框、计划任务 ↔ 登录项、WinINET ↔ 网络服务代理）。

## 下载

<!-- BEGIN:version -->
当前版本 **v0.1.0**（2026-10-07） · [更新日志](CHANGELOG.md) · [下载安装包](https://github.com/zhcx/SuNet/releases/latest)
<!-- END:version -->

<!-- BEGIN:platforms -->
| 平台 | 安装包 | 状态 | 说明 |
| --- | --- | --- | --- |
| Windows 10 / 11 · x64 | `SuNet_<版本>_x64-setup.exe`（NSIS）、`SuNet_<版本>_x64_en-US.msi`（WiX），以及免安装的 `SuNet_windows_x64.exe` | ✅ 已支持 | 三件套（hosts / 系统代理 / DNS）全功能，托盘常驻与全局热键可用；安装包由 GitHub Actions 在打 tag 时自动构建，见 [Releases](https://github.com/zhcx/SuNet/releases) |
| macOS 10.15+ · Apple Silicon / Intel | `SuNet_<版本>_universal.dmg`（通用二进制，内含 `SuNet.app`） | ✅ 已支持（未签名） | 三件套（hosts / 系统代理 / DNS）与托盘、全局热键可用；写 hosts / 系统代理 / DNS 会弹一次系统授权框（输入登录密码，安装后台助手后免密码）；未做代码签名与公证，首次打开需在「系统设置 → 隐私与安全性」里放行，见 [Releases](https://github.com/zhcx/SuNet/releases) |
| Linux | — | ❌ 未支持 | 平台层、托盘、全局热键与提权模型都需要重新设计 |
<!-- END:platforms -->

> 未做代码签名：Windows 首次安装可能弹 SmartScreen，选「仍要运行」即可；
> macOS 首次打开会提示「无法验证开发者」，在「系统设置 → 隐私与安全性」里点「仍要打开」，
> 或者 `xattr -dr com.apple.quarantine /Applications/SuNet.app`。

## 快速开始（从源码构建）

```bash
npm install
npm run tauri -- dev                            # 开发（Windows 需要 WebView2，Win10/11 默认自带）
npm run tauri -- build                          # 打包 NSIS 安装器
npm run tauri -- build -- --bundles nsis,msi     # 再加上 WiX 的 .msi（首次会自动下载 WiX）
cd src-tauri && cargo test                      # 单元测试（Windows 55 项 / macOS 59 项，纯逻辑，不碰系统）

# macOS（需要 Xcode Command Line Tools）
npm run tauri -- dev
npm run tauri -- build -- --target universal-apple-darwin --bundles dmg   # 通用二进制 dmg
```

需要 Node 22+ 与 stable Rust 工具链；dev server 用 **5180** 端口（不是 Tauri 默认的 5173）。
两个平台的平台层代码是分开的（`src-tauri/src/os/windows` 与 `os/macos`），
所以**在哪个平台构建就只能跑那个平台的功能**；打 tag 时两条流水线各出一套安装包。

打 `v*` 形式的标签会触发 [Release 工作流](.github/workflows/release.yml)：在 Windows 上跑测试、
构建两种安装包并创建 Release，同时刷新本页顶部的版本号与平台对照表。

## 文档

| 文档 | 内容 |
| --- | --- |
| [`CHANGELOG.md`](CHANGELOG.md) | 每个版本的变更；发布页说明也从这里提取 |
| [`HANDOFF.md`](HANDOFF.md) | 工程交接：架构、关键实现选择、已实测清单、回归项、macOS 平台层与待实测清单 |
| [`SuNet-设计方案.md`](SuNet-设计方案.md) | 设计方案 v1.5，记录决策与取舍的依据 |

## 已知限制

- 未接入自动更新（没有签名公钥），「检查更新」只跳转 Releases。
- macOS 包未签名未公证（没有开发者证书）：首次打开要在系统设置里放行一次；
  并且 macOS 侧的平台层还没有在真机上跑过，待实测清单见 `HANDOFF.md` §6.4。
- Linux 未支持：平台层、托盘、全局热键与提权模型都需要重新设计。
- 未实现：WinHTTP 系统代理、PAC 模式、TUN / 虚拟网卡、hosts 通配符规则、系统 DoH 写入（v1 只提供端点参考表）。
