# 更新日志

本项目的所有重要变更都记录在这里。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

发布流程：打一个 `v*` 形式的标签（例如 `v0.0.2`）→ GitHub Actions 先用 `prepare-release` 建好 Release，
再由 matrix 并行构建并上传（Windows：NSIS `.exe` + MSI `.msi`；macOS：Apple Silicon 与 Intel 各一份 `.dmg`），
**本文件里对应版本的段落会被自动提取为 Release 说明**，README 顶部的版本号与平台对照表也会同步刷新。

## [0.0.2] - 2026-10-08

本次集中在**提权体验、启动与刷新速度、以及首次运行/托盘的口径统一**上，
并重构了发布流水线（各平台互不连坐）。

### 新增

- **静默提权通道**：一次授权安装后，写 hosts / DNS（macOS 还包括系统代理）不再逐次弹授权框。
  Windows 走最高权限计划任务（装一次，之后由 `schtasks /Run` 静默触发，工作进程做完即退、不常驻）；
  macOS 走常驻 root 助手（本次补上了界面里的安装入口，此前只有命令行 `--helper-install`）。
  入口在「设置 → 提权」；**Windows 默认关闭**，需显式安装（它等价于一次授权长期有效，
  安全边界写在 `src-tauri/src/elevation_task.rs` 模块头）。
- **设置 → 提权**：新增安装/卸载按钮，两个平台同一套 UI（后端按平台给出名称与说明）。

### 变更

- **首次运行不再提示保存「原始设置」**：默认基线就是干净的直连状态（不写 hosts、不开系统代理、
  不改 DNS），要什么由用户主动开启；首启也不再枚举网卡（旧的 `first_run_report` 命令已删除）。
- **托盘提示按真实状态显示**：悬浮提示、托盘菜单项与快捷面板徽标统一口径 —— 有活动方案显示方案名；
  没有活动方案但某一层实际生效中显示「自定义设置」；否则回落「默认 · 直连」。
  不再出现与实际状态不符的「未启用方案」。
- **一次切换的 UAC 次数：Windows 2 次 → 1 次**：hosts 与 DNS 合并进同一个提权进程执行
  （新任务 `batch`，失败即停、已完成的步骤照实上报以决定回滚范围）。
- **应用顺序改为 proxy → hosts+DNS**：回滚逆序由此变成 `dns → hosts → proxy`，
  正好符合设计方案「先恢复解析层、再恢复传输层」的口径；代理不通时不再需要回滚任何提权层。
- **发布流水线重构**：`prepare-release` 先建 Release，再由 matrix 并行构建 Windows 与
  macOS（aarch64 / x86_64 各一份 `.dmg`），平台之间互不连坐（`fail-fast: false`）。
  macOS 由「通用二进制单包」改为「按架构分别产出」。

### 优化

- **权限**：`is_elevated()` 进程内记忆化；提权子进程隐藏窗口（不再闪一下）；
  **no-op 剪枝** —— 已经是目标状态的层不再下发，切到「默认 · 直连」等已达成状态时可 0 次 UAC。
- **速度**：托盘主题轮询由 1 秒放宽到 5 秒（macOS 上每次查询都会 fork 一个 `defaults` 进程）；
  通知只在非 Info 级时刷新托盘；`get_app_state` 的代理状态只读一次；
  Windows 网卡枚举加 2 秒缓存（写后失效，与 macOS 对齐）；
  主窗口刷新合并（一次操作原本会触发 2–3 遍整页重渲）；同页刷新不再闪「加载中…」并保持滚动位置；
  快捷面板高度未变时不再发窗口 resize IPC；DNS 预设搜索加 120 毫秒防抖。

### 修复

- `ui.openModal` 关闭时移除 Esc 监听：此前每开一次弹层都会在 `document` 上残留一个监听，长会话下持续累积。
- 飞行面板的 `.chip` 样式收敛到 `.flyout` 作用域：此前未加限定，会按源码顺序**全局覆盖**主窗口的方案 chip 样式。

## [0.0.1] - 2026-10-07

首个公开版本。Windows 10 / 11 x64 产出 NSIS `.exe` 与 MSI `.msi`；
macOS 10.15+ 产出通用二进制 `.dmg`（Apple Silicon 与 Intel 通用）。
同日晚些时候完成的 macOS 支持也并入本版本。

### 新增

- **方案三件套原子切换**：hosts 托管区块 + 系统代理 + 系统 DNS（IPv4/IPv6 分族）
  作为一个整体一键切换；失败按逆序回滚，并留下可回滚快照，不会停在"半套配置"上。
- **任务栏快捷面板**：常驻托盘，热键或托盘左键唤出；贴边定位、高度自适应、`Esc` 或失焦自动收起。
- **全局热键**：默认在「直连 ↔ 上一个方案」之间切换；改绑时直接按组合键捕获，不用手写字符串。
- **无边框主界面**：自绘顶栏（拖动、双击最大化、最小化、隐藏到托盘），
  并且修掉了 Windows 上首帧残留系统标题栏的问题。
- **Hosts 编辑**：托管区块隔离（只改 `# su.net` 起止标记之间）、语法校验、远程订阅导入
  （镜像地址 + 逐条校验 + diff 确认后落地）。
- **代理设置**：HTTP / HTTPS / SOCKS 手动配置、绕过列表、动态清零与撤销（只保留最近一次）。
- **DNS 管理**：常用地址预设表、按网卡写入 IPv4 / IPv6、DoH / DoT 端点参考表
  （v1 不写系统 DoH，只做参考与提示）。
- **可观测与可恢复**：日志按天切分 + 敏感信息脱敏 + 一键导出诊断包；配置快照与损坏文件隔离；
  检测到异常退出（marker）后给出差异与选择，不静默还原。
- **权限模型（模型 C）**：主进程以普通权限常驻，只在真正需要写 hosts / DNS（macOS 还包括系统代理）时按需提权。
- **macOS 平台层**：系统代理走 `networksetup` 写全部活动网络服务（读回 `scutil --proxy`）；
  DNS 走 `networksetup -getdnsservers` / `-setdnsservers`（服务顺序即优先级，写后回读校验，
  刷缓存用 `dscacheutil -flushcache` + `killall -HUP mDNSResponder`）；hosts 写 `/etc/hosts`，
  同目录临时文件 → `rename` → 归位 `root:wheel` `0644`。
- **macOS 提权**：新增常驻提权助手（root LaunchDaemon + unix socket，按调用方 uid 校验），
  装一次之后写 hosts / DNS / 系统代理不再弹授权框；未装助手时回退到
  `osascript … with administrator privileges`，弹一次系统授权框（输登录密码）。
  助手可用 `SuNet --helper-install` / `--helper-uninstall` / `--helper-status` 管理。
- **macOS 开机自启**：写 `~/Library/LaunchAgents` 的 LaunchAgent（Windows 侧仍是计划任务）。
- **macOS 界面适配**：改用原生交通灯（`titleBarStyle: Overlay` + `hiddenTitle`），隐藏自绘的
  最小化 / 隐藏按钮；授权提示、自启文案、代理说明与热键符号（⌃⌥⇧⌘）随平台切换。
- **发布流水线**：打 `v*` 标签后 CI 依次构建 Windows 与 macOS 产物，再按本文件对应段落创建 Release，
  并把版本号与平台对照表写回 README。

### 变更

- 代理写入收口到 `proxy_ops.rs`，由平台决定是否需要提权（Windows 写 HKCU 不需要，macOS 需要 root）；
  顺带修掉"macOS 上回滚代理会因权限不足静默失败"的问题。
- 跨进程互斥分平台：Windows 仍是命名互斥体，macOS 改用 `/tmp/sunet-critsec.lock` 的 `flock`（同为 10 秒超时）。
- `winreg` 移入 Windows 专用依赖，新增 `libc`（macOS）。
- 错误码 E3003 文案改为「代理设置写入被拒绝（可能需要管理员权限）」。

### 修复

- 快捷面板按钮全部无响应（`getElementById("#id")` 选择器写法错误）。
- 网卡枚举从调用 PowerShell 改为 `GetAdaptersAddresses` FFI：约 3 s → 约 8 ms。
- 全局热键切换不稳定：改绑残留、自我占用误判、保存时回写旧绑定、连按 N 次触发 N 次切换。
- 热键切换方案后主界面不刷新。
- 无边框窗口首帧残留系统标题栏（`WM_NCCALCSIZE` 时序问题，改为显式触发一次 frame recalc）。
- macOS 上回滚代理、刷 DNS 缓存会因缺少管理员权限而静默失败（前者改走提权任务，后者改走 `dns_flush` 任务）。
- 无边框顶栏在 macOS 上会与原生交通灯重叠（改为按平台隐藏自绘按钮并留出左边距）。

### 已知限制

- **macOS 包未签名未公证**（没有开发者证书），首次打开需要在「系统设置 → 隐私与安全性」放行一次，
  或执行 `xattr -dr com.apple.quarantine /Applications/SuNet.app`。
- **macOS 平台层尚未在真机上验证**：开发机是 Windows，且没有 macOS 交叉编译环境
  （`cargo check --target aarch64-apple-darwin` 会因缺少 C 编译器失败），目前只能由 CI 验证"能编译"。
  待实测清单见 `HANDOFF.md` §6.4。
- Linux 未支持：平台层、托盘、全局热键与提权模型都需要重新设计。
- 未接入自动更新（没有签名公钥），「检查更新」只跳转 Releases 页面。
- 未做代码签名，Windows 首次安装时可能触发 SmartScreen 提示。
- 未实现：WinHTTP 系统代理、PAC 模式、TUN / 虚拟网卡、hosts 通配符规则、系统 DoH 写入。

<!-- 模板：

## [x.y.z] - YYYY-MM-DD

### 新增
### 变更
### 修复
### 移除
### 已知限制

-->
