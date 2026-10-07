# 速网 SuNet

[![最新版本](https://img.shields.io/github/v/release/zhcx/SuNet?sort=semver&label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/zhcx/SuNet/releases)
[![Release 工作流](https://github.com/zhcx/SuNet/actions/workflows/release.yml/badge.svg)](https://github.com/zhcx/SuNet/actions/workflows/release.yml)
[![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%2010%2F11%20x64-0078D4)](#平台支持)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB?logo=tauri&logoColor=white)](https://tauri.app)

网络配置切换器：**hosts 托管区块 + WinINET 系统代理 + 系统 DNS（IPv4/IPv6 分族）**，
三者打包为「方案」，一键原子切换，失败自动逆序回滚。

实现依据：`SuNet-设计方案.md` v1.5。技术栈 Rust + Tauri 2，目标平台 Windows 10/11 x64。

---

## 平台支持

<!-- BEGIN:version -->
当前版本 **v0.0.1**（2026-10-07） · [更新日志](CHANGELOG.md) · [下载安装包](https://github.com/zhcx/SuNet/releases/latest)
<!-- END:version -->

<!-- BEGIN:platforms -->
| 平台 | 安装包 | 状态 | 说明 |
| --- | --- | --- | --- |
| Windows 10 / 11 · x64 | `SuNet_<版本>_x64-setup.exe`（NSIS）、`SuNet_<版本>_x64_<语言>.msi`（WiX）、免安装的 `SuNet.exe` | ✅ 已支持 | 三件套（hosts / 系统代理 / DNS）全功能，托盘常驻与全局热键可用；安装包由 GitHub Actions 在打 tag 时自动构建，见 [Releases](https://github.com/zhcx/SuNet/releases) |
| macOS | — | ⏳ 已立项，未适配 | 平台层仍是 Win32 / 注册表 / `netsh` / `hosts`，当前编译不过；移植映射表见 `HANDOFF.md` §6.4 |
| Linux | — | ❌ 未支持 | 同上；托盘、全局热键与提权模型都需要重新设计 |
<!-- END:platforms -->

## 项目规模

下面这张表由 `scripts/update-readme.mjs` 扫描源码生成（每次打 tag 发布时自动刷新），不是手写的数字。

<!-- BEGIN:stats -->
| 项 | 数量 |
| --- | --- |
| 后端（`src-tauri/src`） | 33 个文件 / 11,147 行 Rust |
| 前端（`src`） | 15 个文件 / 6,322 行 TS + CSS |
| Tauri 命令 | 62 个 |
| Rust 单元测试 | 53 个（`cd src-tauri && cargo test`） |
| 错误码 | 26 个（集中定义在 `src-tauri/src/error.rs`） |
| 当前版本 | v0.0.1 |
<!-- END:stats -->

---

## 快速开始

```bash
npm install                     # 前端依赖
npm run tauri -- dev            # 开发（需要 WebView2，Win10/11 默认自带）
npm run tauri -- build          # 打包 NSIS 安装器（含卸载时询问还原）
npm run tauri -- build -- --bundles nsis,msi   # 再加上 WiX 的 .msi（首次会自动下载 WiX）
```

> dev server 用 **5180** 端口（不是 Tauri 默认的 5173）：
> 本机常有其他前端项目占用 5173，而 webview 连的是 `devUrl`。
> 一旦端口被别的项目占住，`strictPort` 会让 vite 直接退出，界面会加载成**别人的页面**，
> 表现为"进程正常、界面空白或诡异"——这类问题极难排查，因此换端口 + 保留启动日志。
> 日志里每次启动都会打印两个窗口的真实 URL，可直接确认加载的是谁。

单独验证：

```bash
npm run check                                   # 前端类型检查
npm run build                                   # 前端构建（输出 dist/）
cd src-tauri && cargo test                      # 53 项单元测试（纯逻辑，不碰系统）
cd src-tauri && cargo build                     # 后端编译
```

### 已实测通过的部分

| 验证项 | 结果 |
|---|---|
| `cargo test` | 53 passed / 0 failed（含编码兜底、区块幂等、旧标记清理、订阅校验、迁移、脱敏） |
| `npm run check` | 类型检查零错误 |
| `cargo build` / `tauri build --no-bundle` | 零警告，release 6.2 MB 且确认内嵌 `assets/*` 与 `quick.html` |
| 生产版加载路径 | 内嵌资源构建下两个窗口分别落在 `http://tauri.localhost/` 与 `…/quick.html`，生产 CSP 下 JS 与 IPC 均正常 |
| 启动（主窗口 / `--minimized` / `--panel`） | 进程存活、托盘构建成功、日志正常写入 |
| 两个窗口的页面归属 | 启动日志打印真实 URL；dev 与生产两条路径都验证过 |
| 前端与后端的 IPC | 两个窗口的 JS 都成功调用 `get_app_state`（日志可见） |
| 快捷面板 | DOM 实测 `tiles=4 / chips=2 / tabs=0 / cards=0`（确为独立瓦片界面，非主界面压缩）；高度自适应从 803→716 物理像素贴合内容；底边固定在工作区上方 |
| 主窗口 DOM | `tabs=5 / cards=3 / layers=3`（三层总览 + 方案列表 + 快照） |
| 字体 | 两个窗口 `getComputedStyle(body).fontFamily` 均为 `"Microsoft YaHei UI", …` |
| 全局热键 | 启动时 `Ctrl+Alt+S` 真实注册成功（非配置回显） |
| 单实例 | 第二个实例立即以退出码 0 退出，首实例继续运行 |
| 提权任务通道 | `--task probe --in … --out …` 端到端跑通：载荷属主校验通过、执行成功、退出码 0、结果 JSON 完整、载荷读完即删 |
| 配置落盘 | `config.json` 为合法 UTF-8 JSON；**故意带 BOM 保存也能正常加载**（记事本/PowerShell 常见行为） |

**尚未实机验证**（需要交互式点击或提权环境，请在目标机上过一遍）：
真实 UAC 授权后的 hosts/DNS 写入与 `ReplaceFileW` 被杀软占用时的回退路径、
代理开关在 Chromium 系应用中的即时生效、100 次反复切换后的 hosts 膨胀、
快捷面板在四类任务栏方向下的贴边效果、面板拖动与失焦收起的实际手感、
以及设计文档 §16.3 的实机验收矩阵（标准用户账户 / 域环境 / 多网卡 / 中文输入法热键吞键）。

### 实测中修掉的问题（都是"看着正常、用起来不对"的那类）

| 现象 | 根因 | 处理 |
|---|---|---|
| 界面空白但进程一切正常 | 5173 被本机另一个前端项目占用，webview 连到了别人的页面 | dev 端口改 5180 + 启动日志打印两个窗口的真实 URL |
| 面板从不回调后端 | 窗口 `url` 用了 `index.html?win=quick`，查询串在解析后丢失 | 改为独立 HTML 入口 `quick.html` |
| 点托盘面板一闪就没 | 窗口刚 show 出来时系统立刻投递一次失焦事件 | 只有"真正获得过焦点"之后才允许失焦收起 |
| 状态栏常挂"另一个进程正在写入" | 冲突探测用"试获取互斥体"实现，两个窗口并发查询时互相误判 | 改为进程内忙标记，跨进程冲突交给真实等待报错 |
| 空状态/提示元素设了 `hidden` 仍显示 | 作者样式的 `display: inline-flex` 盖掉了 `[hidden]` 的 `display:none` | 全局兜底 `[hidden]{display:none!important}` |
| 配置被记事本/PS 保存后进只读模式 | `serde_json` 不接受开头的 UTF-8 BOM | 读取时先按多编码解码（与 hosts 共用解码器） |
| 面板底部一大片空白 | `body.scrollHeight` 量到的是被 flex 拉伸后的高度 | 改为累加子元素高度，并做窗口高度自适应 |

首次运行不弹向导，只做三件事：检测当前系统状态、声明"不会动你没主动点的东西"、
把当前状态存成一个方案（命名「当前配置（日期）」）并静默写入基线快照。

---

## 任务栏快捷面板

托盘图标**左键**弹出无边框小窗。它是**独立设计的一套瓦片界面**，不是主窗口的压缩版：
没有标签页、没有卡片、没有字段表格 —— 主体是四块可点瓦片 + 一行方案 chip。

```
┌─ ● 速网                    ⊙ ↗ ✕ ─┐   ⊙ 主题   ↗ 主窗口   ✕ 收起（Esc）
│ [电影模式] [已提权] [已改系统配置] │   状态 chip：方案 / 权限 / 脏标记
│ ┌───────────────────────────────┐ │
│ │ ⇄ 系统代理              ( ⬤ ) │ │   大号开关：一步开关，不需提权
│ │ 127.0.0.1:7890                │ │
│ └───────────────────────────────┘ │
│ ┌──────────────┐ ┌──────────────┐ │
│ │ ▤ hosts      │ │ 🌐 DNS       │ │   两个等宽瓦片：状态 + 一键动作
│ │ 已写入 42 条 │ │ 手动(v4) …   │ │
│ │      [写入]  │ │ [还原自动][调整]│ │
│ └──────────────┘ └──────────────┘ │
│ ┌───────────────────────────────┐ │
│ │ ⏻ 清除代理设置          [清除] │ │   破坏性操作：内含风险说明
│ └───────────────────────────────┘ │
│ 刚刚清除了系统代理      [撤销]     │   仅撤销窗口内出现
│ ▤ 快速切换方案              新建   │
│ (默认·直连) (电影模式) (家里) ＋   │   点一下即切
│ [⚙ 设置]                 [主窗口] │
└───────────────────────────────────┘
```

行为细节（每条都有实现约束，不是"顺手加的"）：

| 行为 | 说明 |
|---|---|
| 摆放位置 | 按点击点离哪条屏幕边最近判断任务栏在上/下/左/右，再决定弹出方向，并夹在工作区内 |
| 贴合内容 | 高度由前端量出内容自然高度后回调主进程调整，**底边固定**地向上/向下生长，不留大片空白 |
| 失焦收起 | 点到别处自动收起；**两种例外不收**：正在执行操作（UAC 会抢焦点）、自弹出以来还没真正获得过焦点（启动瞬间的失焦事件会把面板立刻收掉） |
| 需要提权时 | 用面板内的**内联确认条**而不是居中弹层（小窗放不下），确认后 UAC 授权，面板保持可见以便看到结果 |
| 代理开关 | 一步到位不预先确认（不需要提权）；前置探测失败时才弹出"仍然开启"的明确出口 |
| 方案切换 | 点 chip 即切；hosts/DNS 变更会先确认（因为要弹 UAC） |
| 细节操作 | 「调整 DNS / 查看 hosts / 处理崩溃」等一律带着标签页跳主窗口，不丢上下文 |
| 第二条入口 | 主窗口「方案」页有一键唤出；也支持 `SuNet.exe --panel` 冷启动直接给面板（可做桌面快捷方式） |
| 独立 HTML 入口 | 面板是 `quick.html`（而非 `index.html?win=quick`）——Tauri 会把非 URL 形式的窗口 `url` 当路径处理，查询串在部分路径下会丢 |

字体：全软件（含面板）统一 `Microsoft YaHei UI → Microsoft YaHei → 微软雅黑`；
等宽内容（IP / 域名）用 Cascadia Mono，并带雅黑兜底，中文注释不会掉进衬线或方块。

---

## 权限模型（模型 C：主进程 asInvoker + 按需提权助手）

| 操作 | 是否需要管理员 | 说明 |
|---|---|---|
| 开关系统代理 | ❌ | HKCU 用户 hive 默认可写，**完全静默、不弹 UAC** |
| 全局热键 / 托盘 / 配置 | ❌ | 用户目录与用户态 API |
| 修改 hosts / DNS | ✅ | 仅在**用户主动点击**时弹一次 UAC |

- 提权子进程分流放在 `main()` 第一行：**不进 Tauri setup、不创建窗口、不加载 WebView2、干完即退**
- 提权走 `ShellExecuteExW` + `runas` verb（`CreateProcess` 不触发 UAC，会返回 740）
- 载荷走文件交换（命令行有长度上限，超长会被静默截断），文件名含随机 uuid，
  子进程校验属主与重解析点后才执行，只按 `task` 名分派到固定函数表
- 提权任务协议：退出码做粗粒度分类 + `--out` JSON 回传完整 `ApplyReport`

---

## 目录结构

> 改代码请同时读 `HANDOFF.md`：那里写了硬约束、踩过的坑、可复现的验证手段与待办清单。

```
src/                      前端（原生 TS + Vite，无 UI 框架）
├─ main.ts                外壳 / 路由 / 状态栏 / 事件桥
├─ tokens.css             设计令牌（深浅两套颜色，全部引用变量）
├─ theme.ts               主题三态 follow_system / light / dark
├─ api.ts                 与 Rust 命令一一对应的类型化封装
├─ ui.ts                  弹层 / 气泡 / 横幅 / 事务报告渲染
├─ settings.ts            设置弹层（自启 / 热键 / 退出行为 / 日志）
└─ tabs/                  hosts / proxy / dns / profiles / logs

src-tauri/src/
├─ main.rs                --task 分流 + Tauri 装配 + 托盘 + 退出策略
├─ task_runner.rs         提权子进程任务分派（固定函数表）
├─ elevation.rs           ShellExecuteExW + runas + 等待 + 收结果
├─ ipc.rs                 载荷/结果文件交换
├─ critsec.rs             跨进程命名互斥体
├─ apply.rs               事务式切换引擎（核心）
├─ backup.rs              快照 / 差异 / 回滚
├─ probe.rs               连通性自检
├─ subscribe.rs           远程订阅 + 内容校验 + 变更确认
├─ config.rs / migrate.rs 配置持久化与 schema 迁移
├─ logging.rs             按天切分日志 + 脱敏
├─ notify.rs / tray.rs    通知降级链路 + 托盘状态聚合
├─ state.rs               全局状态 + 会话标记 + 崩溃恢复检测
├─ cmd/                   Tauri 命令层
└─ os/                    hosts / wininet / dns_client / privilege / FFI
```

---

## 关键实现选择

**FFI 用自写声明而非 `windows` crate。** 设计文档在 `windows` crate 的版本上自身摇摆
（0.6 / 0.61），而该 crate 的 API 面貌随小版本漂移。这里只声明本项目真正调用的
十余个导出函数（`os/winapi.rs`），签名固定、无版本风险。

**DNS 的「是否 DHCP」不看 PowerShell 文案**（会被本地化），而是读注册表
`Tcpip\Parameters\Interfaces\{GUID}\NameServer`（v4）与 `Tcpip6\...`（v6）：
值存在且非空 = 手动配置，否则 = 自动获取。这与 Windows 自身的语义一致，
且不受界面语言影响。

**hosts 编辑走「草稿 + 一次写入」。** `hosts_upsert` / `hosts_delete` 只改配置草稿
（不弹 UAC），由用户点「写入系统 hosts」触发一次提权覆盖全部改动 ——
避免每改一行弹一次 UAC，同时不破坏"hosts 写入需要提权"这条边界。

**订阅内容按不可信输入对待。** 仅 https（禁止 302 跳到其他协议）、逐行语法校验、
拒绝通配符/正则元字符/控制字符/特殊地址、单源 ≤5000 条（超出整源拒绝不静默截断）、
首次导入或变更 >20% 必须先过 diff 确认、命中敏感域名（银行 / 支付 / 政务 / 邮箱 / CA）
的变更默认不勾选、两源都失败保留本地内容。

---

## 发布日志

<!-- BEGIN:latest -->
**v0.0.1**（2026-10-07）的变更（摘要）：

### 新增

- **方案三件套原子切换**：hosts 托管区块 + WinINET 系统代理 + 系统 DNS（IPv4/IPv6 分族）
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
- **权限模型（模型 C）**：主进程按 `asInvoker` 常驻，只在真正需要写 hosts / DNS 时按需提权。

完整历史见 [CHANGELOG.md](CHANGELOG.md)，全部版本与安装包见 [Releases](https://github.com/zhcx/SuNet/releases)。
<!-- END:latest -->

## 已知偏差（相对设计方案，实现阶段的有意取舍）

| 项 | 设计 | 实现 | 原因 |
|---|---|---|---|
| hosts 编辑提权次数 | `hosts_upsert` 标注 [需提权] | 草稿不弹 UAC，`hosts_apply` 提权一次 | 逐行编辑都要 UAC 会不可用；写入仍是提权的 |
| 气泡通知 | 托盘气泡 + 「撤销」按钮 | 事件驱动的前端横幅/气泡 + 托盘 tooltip `⚠` 前缀 + 日志 | 未引入 `tauri-plugin-notification` 以缩小依赖面；降级链路的三个落点均已实现 |
| 主题变更监听 | 监听 `WM_SETTINGCHANGE` | 后台线程每秒比对 `AppsUseLightTheme`，变化时重建托盘图标 | 效果等价，避免在非窗口线程挂消息钩子 |
| 载荷属主校验 | 校验"必须属于发起用户" | 拒绝重解析点 + 拒绝低权限属主（Everyone / Users / Authenticated Users / Guests / Anonymous）+ 属主 SID 入日志 | 标准用户 + 管理员凭据场景下子进程无法得知发起用户身份；组合防线见 `os/fs_security.rs` |
| `hosts` 域名校验 | 每段 `a-zA-Z0-9-` | 额外允许下划线 | 现实 hosts 里存在 `_ldap._tcp` 这类记录，拒绝会误伤 |
| 托盘代理开关 | 未细化 | 用当前方案的代理配置；未配置时打开窗口并提示 | 避免在配置里再造一个"隐藏的代理地址" |
| 快捷面板入口 | 未设计 | 独立的 `quick.html` 入口（而非 `index.html?win=quick`） | Tauri 把非 URL 形式的窗口 `url` 当路径处理，查询串在部分路径下会丢；独立入口是确定性的 |
| 面板内确认 | 居中弹层 | 内联确认条 | 372px 宽的窗口里居中弹层会挤成一团 |
| dev 端口 | 5173（Tauri 默认） | 5180 | 5173 常被其他前端项目占用，会导致界面加载成别人的页面 |
| 全局热键语义 | 按 §5.5.5 = 清除系统代理 | `Ctrl+Alt+S` = 「默认直连 ↔ 上一个方案」来回切（2026-10-07 用户调整） | 直连方案是回滚固定点；记忆存在配置 `previous_profile_id`，重启有效；托盘菜单的「清除代理设置」保持原语义 |
| 网卡/DNS 枚举与写入 | PowerShell（方案 B） | 枚举=`GetAdaptersAddresses` FFI；写入/还原=netsh 分族命令（`Set-DnsClientServerAddress` 无分族参数，见 HANDOFF §4.14） | 实测每次 PowerShell 冷启动约 3 秒，枚举遍布状态聚合/DNS 页/写入前校验，是"点了没反应"的主因；FFI 毫秒级 |
| 面板常驻内存 | 未约定 | 面板窗口在创建后不销毁，`hide` 时保留 | 冷启动到面板可用 <200ms；代价是常驻一个约 30MB 的 WebView2 进程 |

---

## 验收对照

| 设计条目 | 落地位置 |
|---|---|
| hosts 托管区块 + 编码兜底 + 原子替换 + 回读校验 | `os/hosts_file.rs`（含幂等单测） |
| 写注册表后必须通知系统（39 / 37） | `os/wininet.rs::notify_changed`，失败降级为 warning 不中断事务 |
| 代理快照—还原而非简单置 0；清零必删 `AutoConfigURL` | `apply.rs::step_proxy` / `hard_clear` + 5 秒内存撤销 |
| 前置连通性自检（800ms）+ 开启后外网复检 | `probe.rs` + `apply.rs` Verify 阶段（方案切换严格回滚，手动开关给"仍然开启"出口） |
| DNS 分族独立控制，取消勾选 = 该族还原 DHCP | `os/dns_client.rs::set/reset` + `apply.rs::build_targets` |
| 约 30 项内置 DNS 清单（逐条 v4/v6 成对） | `os/dns_presets.rs`（含地址合法性单测） |
| 事务：快照 → 逐层执行 → 回读校验 → 逆序回滚 → Recovery | `apply.rs` |
| 跨进程命名互斥体 + `WAIT_ABANDONED` 接管 | `critsec.rs` |
| 单实例 + 开机自启（`/RL LIMITED`，不弹 UAC） | `main.rs` + `cmd/system.rs::set_autostart` |
| 全局热键：注册前探测、失败报 `E5001`、按下/释放只取 Pressed、双入口 | `cmd/system.rs` + `main.rs` handler |
| 崩溃恢复：检测 marker + 不自动还原 + 给差异与选择 | `state.rs::detect_crash_recovery` + 前端横幅 |
| 动态清零撤销（内存暂存、只留最近一次、写回后同样通知） | `apply.rs::clear_proxy/undo_clear` + `state.rs::ClearState` |
| 远程订阅：镜像、失败不清空、逐条校验、diff 确认 | `subscribe.rs` |
| 日志按天切分 + 脱敏 + 诊断包 | `logging.rs` + `cmd/system.rs::logs_export` |
| 深浅两版托盘图标 + `include_bytes!` 内嵌 | `tray.rs` |
| 设计令牌与主题三态 | `tokens.css` + `theme.ts` |

**未实现**（设计文档本身列为 v2 或「不做」）：WinHTTP 系统代理、PAC 代理模式、
TUN/虚拟网卡、hosts 通配符规则、系统 DoH 写入（v1 只提供 DoH/DoT 端点参考表）、
自动更新下载器与「检查更新」跳转（仓库已公开在 GitHub，Releases 地址是
`https://github.com/zhcx/SuNet/releases`；但代码里还没有接 tauri 更新器，
只保留了 `channels` 占位字段，加更新通道时不需要再做配置迁移）。
