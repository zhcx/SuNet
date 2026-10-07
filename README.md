# 速网 SuNet

[![最新版本](https://img.shields.io/github/v/release/zhcx/SuNet?sort=semver&label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/zhcx/SuNet/releases)
[![Release 工作流](https://github.com/zhcx/SuNet/actions/workflows/release.yml/badge.svg)](https://github.com/zhcx/SuNet/actions/workflows/release.yml)
[![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%2010%2F11%20x64-0078D4)](#下载)

网络配置切换器：把 **hosts 托管区块 + WinINET 系统代理 + 系统 DNS（IPv4/IPv6 分族）**
打包成一个「方案」，一键整体切换；任何一步失败就按逆序回滚，不会停在"半套配置"上。

技术栈 Rust + Tauri 2，目标平台 Windows 10 / 11 x64；设计依据是 `SuNet-设计方案.md` v1.5。

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
- **权限模型（模型 C）**：主进程按 `asInvoker` 常驻，只在真正需要写 hosts / DNS 时按需提权。

## 下载

<!-- BEGIN:version -->
当前版本 **v0.0.1**（2026-10-07） · [更新日志](CHANGELOG.md) · [下载安装包](https://github.com/zhcx/SuNet/releases/latest)
<!-- END:version -->

<!-- BEGIN:platforms -->
| 平台 | 安装包 | 状态 | 说明 |
| --- | --- | --- | --- |
| Windows 10 / 11 · x64 | `SuNet_<版本>_x64-setup.exe`（NSIS）、`SuNet_<版本>_x64_en-US.msi`（WiX），以及免安装的 `SuNet_windows_x64.exe` | ✅ 已支持 | 三件套（hosts / 系统代理 / DNS）全功能，托盘常驻与全局热键可用；安装包由 GitHub Actions 在打 tag 时自动构建，见 [Releases](https://github.com/zhcx/SuNet/releases) |
| macOS | — | ⏳ 已立项，未适配 | 平台层仍是 Win32 / 注册表 / `netsh` / `hosts`，当前编译不过；移植映射表见 `HANDOFF.md` §6.4 |
| Linux | — | ❌ 未支持 | 同上；托盘、全局热键与提权模型都需要重新设计 |
<!-- END:platforms -->

> 未做代码签名，首次安装时 Windows 可能弹出 SmartScreen 提示，选「仍要运行」即可。

## 快速开始（从源码构建）

```bash
npm install
npm run tauri -- dev                            # 开发（需要 WebView2，Win10/11 默认自带）
npm run tauri -- build                          # 打包 NSIS 安装器
npm run tauri -- build -- --bundles nsis,msi     # 再加上 WiX 的 .msi（首次会自动下载 WiX）
cd src-tauri && cargo test                      # 53 项单元测试（纯逻辑，不碰系统）
```

需要 Node 22+ 与 stable Rust 工具链；dev server 用 **5180** 端口（不是 Tauri 默认的 5173）。

打 `v*` 形式的标签会触发 [Release 工作流](.github/workflows/release.yml)：在 Windows 上跑测试、
构建两种安装包并创建 Release，同时刷新本页顶部的版本号与平台对照表。

## 文档

| 文档 | 内容 |
| --- | --- |
| [`CHANGELOG.md`](CHANGELOG.md) | 每个版本的变更；发布页说明也从这里提取 |
| [`HANDOFF.md`](HANDOFF.md) | 工程交接：架构、关键实现选择、已实测清单、回归项、macOS 移植立项 |
| [`SuNet-设计方案.md`](SuNet-设计方案.md) | 设计方案 v1.5，记录决策与取舍的依据 |

## 已知限制

- 未接入自动更新（没有签名公钥），「检查更新」只跳转 Releases。
- macOS / Linux 未适配：平台层仍是 Win32 / 注册表 / `netsh` / `hosts`。
- 未实现：WinHTTP 系统代理、PAC 模式、TUN / 虚拟网卡、hosts 通配符规则、系统 DoH 写入（v1 只提供端点参考表）。
