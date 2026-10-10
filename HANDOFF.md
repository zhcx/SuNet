# SuNet 交接文档（HANDOFF）

> 给下一个接手的人 / agent。设计依据是仓库根目录的 `SuNet-设计方案.md`，
> 本文只写**设计方案没写、但改代码时必须知道**的事：现状、硬约束、踩过的坑、怎么验证。
>
> 阅读顺序建议：`SuNet-设计方案.md`（为什么这么做）→ `README.md`（用户视角与偏差表）→ **本文**（怎么改、怎么验）→ 代码。

---

## 0. 三十秒上手

```bash
npm install
npm run tauri -- dev            # 开发（先起 vite:5180，再起 Tauri）
npm run tauri -- build          # 生产包（真正内嵌前端资源的构建，见 §4.8）
npm run check                   # 前端类型检查
cd src-tauri && cargo test      # 本机平台跑自己那份单测（Windows 55 项，纯逻辑，不碰系统）

# macOS（需要 Xcode Command Line Tools）
npm run tauri -- build -- --target aarch64-apple-darwin --bundles dmg   # Apple Silicon
npm run tauri -- build -- --target x86_64-apple-darwin --bundles dmg    # Intel

# 发布：打 tag 即出包（Windows NSIS+MSI、macOS 两种架构各一份 dmg，同一个 Release）
git tag v0.0.1 && git push origin v0.0.1
```

当前状态：**功能完整、可运行、已实测通过**（范围见 §5.1），未做代码签名与自动更新。
> 2026-10-07 大修：快捷面板按钮全死的根因是 `bind()` 用 `getElementById("#…")`（永远 null），已改为 `querySelector`；DNS 网卡枚举从 PowerShell 换成 `GetAdaptersAddresses` FFI（3s → ~8ms）；全局热键语义改为「默认直连 ↔ 上一个方案」。细节见 §4.10–§4.12。
> 2026-10-07 热键专项：快捷键从「手打文本框」改为**按键捕获控件**（新增 `shortcut_capture` 命令，录制时临时让出全局热键），统一三套互不相认的键名表并新增 `E5004`，修掉「改绑后旧组合残留」「自我占用误判 E5001」「保存回写旧 binding」「连按 N 次 = N 次同向切换」四个不稳定根因；主界面不再显示「未启用任何方案」。细节见 §4.16。
> 2026-10-07 热键专项（续）：热键切换方案后**主窗口与快捷面板仍显示旧方案**（代理实际已清）——热键路径是唯一不推 `sunet://report` 的切换入口，前端因此收不到刷新信号；另加「快捷面板每次唤出即重拉状态」。版本号 `1.0.0` → **`0.0.1`**（package.json / tauri.conf.json / Cargo.toml，展示处由 `CARGO_PKG_VERSION` 自动跟随）。细节见 §4.17。
> 2026-10-07 无边框：主界面隐藏 Windows 标题栏，改为自绘顶栏（右上角 `−` 最小化 / `×` 隐藏到托盘，新增 `window_minimize` 命令）；踩到 tao「首帧 `WM_NCCALCSIZE` 走 DefWindowProc → 残留系统标题栏」的坑，用 `force_frame_recalc`（`SetWindowPos(SWP_FRAMECHANGED)`）修掉。细节见 §4.18；macOS 侧不用这套补丁（改用原生的 `titleBarStyle: "Overlay"`）。
> 2026-10-07 macOS：新增 `src-tauri/src/os/macos/*`（7 文件）与常驻提权助手 `src-tauri/src/helper/*`（unix socket + root LaunchDaemon）；
> 共用层按平台分叉（新增 `proxy_ops.rs`、`proxy_write_needs_root()`、`flush_needs_root()`、`critsec.rs` 的 flock 分支）；
> 前端新增 `src/platform.ts`（唯一平台真源，给 `<html>` 打 `data-os`）；**版本号保持 `0.0.1`**（macOS 支持并入同一版本，与 Release tag 对齐）；CI 加 macOS job。
> Windows 侧逻辑未变。细节见 §4.19 与 §6.4。
> 2026-10-08 首启与托盘口径：首启不再弹「保存原始设置」引导（默认基线 = 不写 hosts / 不开代理 / 不改 DNS）；托盘 tooltip、托盘菜单项与快捷面板徽标统一为 `effective_headline`（无活动方案时按真实生效状态显示「自定义设置」或「默认 · 直连」）。细节见 §4.16.2 与设计方案 §15.1。
> 2026-10-08 权限与性能专项：
> · **权限** —— `is_elevated()` 进程内 memoize；提权子进程 `SW_HIDE`（不再闪窗）；`apply()` 增加 **no-op 剪枝**（`prune_satisfied`）：已经是目标状态的 hosts / DNS 层不再下发，于是切到「默认 · 直连」或任何已达成方案时**不会再白弹 UAC**。
> · **性能** —— 托盘主题轮询 1s→5s（macOS 的 `taskbar_is_light()` 每秒会 fork 一个 `defaults` 进程）；`notify::send` 只有非 Info 才刷新托盘；`get_app_state` 的代理状态只读一次；Windows 网卡枚举加 2s 缓存（写后失效，与 macOS 对齐）；主窗口标签栏只构建一次、方案下拉仅在集合变化时重建、同页刷新不再闪「加载中…」并保住滚动位置；快捷面板高度未变不再发 resize IPC；`ui.openModal` 关闭时移除 Esc 监听（修掉监听累积泄漏）。
> · **UI** —— `.chip*` 的飞行面板样式收敛到 `.flyout` 作用域（原先未限定，会按源码顺序全局覆盖主窗口的方案 chip 样式）。
> 2026-10-08 三项推进（提权合并 / 静默提权 / 深刷新合并）：
> · **合并提权**：`apply()` 把 hosts + DNS 合并进**同一个提权子进程**（新任务 `batch`，见 `task_runner.rs`）—— Windows 上「切一次方案弹两次 UAC」变成一次。顺序调整为 **proxy → hosts+DNS**，于是回滚逆序 = dns → hosts → proxy，正好符合"先恢复解析层、再恢复传输层"的文档口径（旧顺序 hosts→proxy→dns 的逆序与口径相悖）；代理失败时也就不用回滚任何提权层了。新增 `elevation::run_elevated_raw`：批处理要靠子进程上报的**部分步骤**决定回滚哪些层，所以不能把 `!ok` 直接转成 `Err`。回滚路径仍逐层调用（失败路径，暂未合并）。
> · **静默提权通道（Windows，默认关）**：新增 `src-tauri/src/elevation_task.rs` —— 用户显式安装后注册一个 `RL HIGHEST` 的计划任务 `SuNet-SilentElevation`（动作是 `SuNet.exe --silent-worker`），此后的写入只需 `schtasks /Run` 触发，**不再弹 UAC**；工作进程读走 `SuNet/ipc` 里 60 秒内新写下的载荷（仍要过 `verify_payload_file` 的属主/形状/非符号链接校验与任务白名单），做完即退、不常驻、不建托盘。任何一步失败都自动回退 `runas`。**安全边界（已在模块头写清）**：它等价于"一次授权、长期有效"，任何以该用户身份运行的代码都能触发 → 因此**默认关闭**、必须在「设置 → 提权」里显式安装；卸载后立即恢复逐次 UAC。
> · **深刷新合并**：`main.ts` 的 `refreshState` 把同一时间窗内的多次刷新合并成一次（一次操作原本会连着触发 2–3 遍整页重渲），期间提出的 deep 请求补跑一次；DNS 预设搜索加 120ms 防抖。
> 2026-10-08 macOS 同步（静默提权做成两平台统一）：
> · 上面那条「静默提权通道」不再只有 Windows 有实现 —— `elevation_task.rs` 现在是三个实现：`imp`（Windows，计划任务）、**`macos_impl`（macOS，映射到常驻 root 助手）**、`unsupported`（兜底）。`noun()/detail()` 由后端按平台给文案，前端不做平台判断，于是「设置 → 提权」的按钮在两个平台都可用：Windows 显示「安装静默提权通道」，macOS 显示「安装免密提权助手」。
> · **补上的真实缺口**：macOS 的常驻助手此前**只有命令行入口**（`SuNet --helper-install` / `--helper-uninstall` / `--helper-status`，见 `main.rs` 的 `helper_entry`），界面上装不了。现在 `elevation::run_self_command_elevated(&["--helper-install"])` 用 `osascript ... with administrator privileges` 跑这条自身子命令（不改 `--task` 协议、不接受任意命令、不计 timeout），设置页可一键安装/卸载。
> · 其余两项（`batch` 合并提权、刷新合并、no-op 剪枝、memoize、主题轮询降频等）本来就写在共享代码里，macOS 同样生效；`batch` 在 macOS 上可用是因为 helper 守护进程也是调 `task_runner::execute`（见 `helper/daemon.rs`），无需另做适配。
> 2026-10-08 开机自启修复（用户实测截图报出）：
> · **症状**：开「开机自启动」必报「创建计划任务失败（退出码 1）」。
> · **根因**：`platform_autostart`（Windows）在**主进程**（asInvoker）里直接调 `schtasks /Create`，而任务库 `C:\Windows\System32\Tasks` 只有管理员可写。用未提权 shell 跑同样参数复现：`exit=1` + `ERROR: Access is denied.`。
> · **修法**：新增提权任务 `autostart_set`，实现抽到 `os/windows/autostart.rs::apply()`；`cmd/system.rs` 的 `platform_autostart` 改为 `elevation::run_elevated("autostart_set", …)`（装过静默通道则静默，否则一次 UAC）。macOS 侧本来就是 LaunchAgent，不需要提权，未动。
> · **顺带修掉乱码**：schtasks / netsh 的输出按 **OEM 代码页**（简中 = 936）编码，代码却用 `from_utf8_lossy`，中文系统上界面上只能看到一片方块、真正的报错全丢。新增 `winapi::decode_console_output` 并替换三处调用点（自启、静默通道、netsh DNS 写入）。
> · **发布流程加固**：`prepare-release` 在同 tag 重发时会先清掉旧资产再刷新说明（否则 tauri-action 上传同名资产会撞名字），参考 zeditor 的同类步骤。
> 2026-10-10 macOS 实测修复（用户报告，四项）：
> · **左上角 logo 与系统交通灯重合 = 首帧问题**：`data-os` 由 `src/platform.ts`（deferred 模块）打上，**晚于首次绘制** —— 而主窗口在 `setup()` 里就 `show()` 出来了，于是会先画一帧"没给交通灯留白"的顶栏再跳到正确位置。改为首帧脚本 `public/os-mark.js`（两个 HTML 的 `<head>` 以**经典脚本**引入；**不能内联** —— 生产 CSP 是 `script-src 'self'`，内联会被拦，`devCsp` 里的 `'unsafe-inline'` 会掩盖这个坑）在首帧前打标，`platform.ts` 优先读该属性、缺失才回退 UA，顶栏安全区 78 → 88px（覆盖 AppKit 把按钮弹回默认位置的那一帧，见 tao 的 `reapply_traffic_light_inset`）。
> · **一次方案切换要输两次密码 = 代理没并入提权批处理**：macOS 的系统代理写入（`networksetup`）要 root，此前 `step_proxy` 独占一次提权，与 hosts / DNS 的 `batch` 合计两次授权。现在代理层拆成 **准备（父进程：校验 / 前置探测 / 快照）→ 写入（`ProxyWrite::needs_root()` 决定本进程还是批处理）→ 收尾（父进程：回读校验 + 报告文案）**，macOS 上代理作为 `batch` 的第一个子任务（复用 `proxy_write` / `proxy_restore`）→ 一次切换只弹一次授权框。Windows 路径**完全不变**（代理不需要提权，仍在本进程写）。顺带把「组装子任务」的 `privileged_ops()` 抽出来并补了 4 条单测（顺序 = 代理 → hosts → DNS；`proxy` 传 `None` 时不含代理；名字都在 `task_runner::KNOWN_TASKS` 白名单里）。
> · **macOS 一开代理就断网 = 三个协议被当成一个**：macOS 的 HTTP / HTTPS / SOCKS 是三个独立开关，`os/macos/system_proxy.rs` 原先三项都写同一个 `host:port` —— 只支持 HTTP 的代理端口收到 SOCKS 请求，其它程序就走不动了。改为**只写 HTTP**（`-setwebproxy`；CFNetwork 没有 Secure Web Proxy 时 HTTPS 也走 HTTP 代理），启用时顺手关掉旧版本留下的 HTTPS / SOCKS 残留、关闭时三项全关；命令表收敛成 `Proto`（get/set/state 三个命令 + 标签）+ `MANUAL_PROTO` / `MANUAL_CLEAR` / `MANUAL_OFF` 三个常量，补了 3 条 macOS 专属单测（只写 HTTP、三个协议命令互不相同、还原点逐协议）。**还原点也改成逐协议**（`proxy_prev.json` 加 `fmt=2`，旧格式整份丢弃）：合并成一个值时"切回原状态"会把本来没开的协议一起打开，是同一个故障的另一副面孔；另外 `snapshot_services` 不再覆盖已存在的还原点（与 app 层 `ensure_proxy_snapshot` 同一约定）。
> · **输过密码后主动提示怎么免掉它**：`ApplyReport` 新增 `prompted`（提权层在走 `runas` / `osascript` 时打标记，事务开始先清零，只统计本次），`cmd/profile.rs` 的 `hint_no_password()` 在切换 / 还原确实弹过框、而免密通道还没装（或装了没跑起来）时提示一次并给「去设置」入口（`elevation_task::no_password_hint()`：macOS 给出安装/放行指引，Windows 返回 `None`——静默通道的安全边界要求显式开启，不主动引导）。每个会话只提示一次。

| 事实 | 值 |
|---|---|
| 后端 | Rust 47 文件 / 13 827 行（`os/windows` 8 + `os/macos` 7 + `helper` 4）；Tauri 2.12.1 + tauri-build 2.7.1 |
| 前端 | 原生 TS + Vite 7（**无 UI 框架**）16 文件 / 6 439 行（TS + CSS） |
| Tauri 命令 | **62 个**，全部在 `src-tauri/src/main.rs` 的 `invoke_handler` 注册 |
| 错误码 | 26 个（`src-tauri/src/error.rs`，含中文文案表与退出码映射） |
| 配置 schema | `schema_version = 1`（迁移表在 `migrate.rs`） |
| 单测 | 源码共 **66** 个 `#[test]`（覆盖 hosts 编解码/幂等、预设地址、订阅校验、迁移、脱敏、hex、IPC 映射、热键直连切换目标选择、快捷键键名收敛与非法组合拒绝、DNS/代理解析与 `pick_in_use`）；其中平台专属 macOS 11 / Windows 7，各自只编译自己那份 → **Windows 实测 55**，macOS 预期 59 |
| 前端产物 | index.html + quick.html 双入口，约 98 KB JS / 25 KB CSS（压缩后 gzip 约 31 KB） |
| 生产包体积 | Windows：`SuNet.exe` 6.2 MB（不含 WebView2，用系统自带）；NSIS 安装包 `SuNet_<版本>_x64-setup.exe` 约 2.2 MB（`target/release/bundle/nsis/`），MSI 在 `bundle/msi/`。macOS：`SuNet_<版本>_aarch64.dmg` / `SuNet_<版本>_x86_64.dmg`（ad-hoc 签名，未公证；此前 universal 包约 5.8 MB） |
| 发布流水线 | `.github/workflows/release.yml`（结构参考同作者的 zeditor）：tag `v*` → `prepare-release`（先建 Release，说明取自 CHANGELOG）→ `build` matrix 并行（Windows nsis+msi；macOS aarch64 / x86_64 各一份 dmg，`fail-fast: false`）→ `readme`（把平台表与版本号写回 README 并提交）；各构建 job 只依赖 `prepare-release`，平台之间互不连坐 |

---

## 1. 本机环境相关事实（换机器前先确认）

| 项 | 值 / 说明 |
|---|---|
| **dev 端口** | **5180**（`vite.config.ts` 与 `tauri.conf.json` 的 `devUrl` 必须一致）。**不要改回 5173**：本机上 5173 常被另一个前端项目占用，`strictPort` 会让 vite 直接退出，webview 会连到**别人的页面**，表现为"进程正常但界面空白" |
| 配置目录 | `%APPDATA%\SuNet\`（`config.json` / `logs/` / `snapshots/` / `staging/` / `session.marker` / `proxy_snapshot.json`） |
| IPC 目录 | `%LOCALAPPDATA%\SuNet\ipc\`（提权任务载荷，`task-<uuid>.in.json` / `.out.json`，用完即删） |
| hosts | `C:\Windows\System32\drivers\etc\hosts`，备份写在**同目录** `hosts.bak.<时间戳>` |
| 显示环境 | 3840×2160 @150%；面板几何分析依赖 `Monitor::work_area()`（物理像素） |
| WebView2 | 已装 154.x；缺 WebView2 时 Tauri 会在建窗阶段报错退出 |
| 配置目录（macOS） | `~/Library/Application Support/SuNet/`（同一批文件；日志落在 `~/Library/Logs/SuNet/`，IPC 载荷在 `~/Library/Caches/SuNet/ipc/`） |
| hosts（macOS） | `/etc/hosts`（root 写：同目录 `.tmp` → `sync_all` → `rename` → `chown 0:0` + `chmod 0644`） |
| 提权助手（macOS） | `--helper-install`（需 root）装到 `/Library/PrivilegedHelperTools/com.sunet.helper` + `/Library/LaunchDaemons/com.sunet.helper.plist`；socket `/var/run/sunet-helper.sock`（0666，按 peer uid 校验调用方）；日志 `/var/log/sunet-helper.log` |
| 本机做不了的事 | **macOS 侧本地无法编译**：没有 `cc`，`cargo check --target aarch64-apple-darwin` 会死在 `objc2-exception-helper` 的 build script（`failed to find tool "cc"`）；macOS 能不能编译只能由 CI 回答 |

---

## 2. 代码地图：要改 X 就去 Y

| 要改什么 | 去哪个文件 |
|---|---|
| hosts 解析/渲染/编码/回读校验（平台无关） | `src-tauri/src/os/hosts_file.rs`（**最核心**，改动必跑单测） |
| hosts 落盘（平台相关） | `src-tauri/src/os/{windows,macos}/hosts_io.rs`（Windows=临时文件 + `ReplaceFileW` 回退；macOS=同目录 `.tmp` + `rename` + `chown/chmod`） |
| 系统代理（读 + 平台实现） | `src-tauri/src/os/{windows,macos}/system_proxy.rs`（Windows=注册表 `Internet Settings` + `InternetSetOptionW`；macOS=`networksetup` × 活动服务 + `scutil --proxy`） |
| 系统代理（**写**，唯一出口） | `src-tauri/src/proxy_ops.rs`：按 `system_proxy::proxy_write_needs_root()` 决定进程内直写（Windows）还是提权任务（macOS 的 `proxy_write` / `proxy_restore` / `proxy_clear`） |
| DNS 网卡枚举 / 分族设置 / 回读 | `src-tauri/src/os/{windows,macos}/dns_client.rs`（Windows=GetAdaptersAddresses FFI + netsh 分族，FFI 声明在 `os/windows/winapi.rs`；macOS=`networksetup` 服务顺序 + `ifconfig`，2s 缓存 + `invalidate_cache()`） |
| 提权助手（macOS） | `src-tauri/src/helper/{mod,client,daemon,install}.rs`（unix socket 协议、peer uid 校验、LaunchDaemon 安装/卸载/状态） |
| 内置 DNS 清单 / 敏感域名表 | `src-tauri/src/os/dns_presets.rs`（编译进二进制，**不进配置文件**） |
| 事务：快照→执行→回滚 | `src-tauri/src/apply.rs`（切换引擎，改动前先读 §3 的不变量） |
| 快照存储 / 差异对比 | `src-tauri/src/backup.rs` |
| 提权调用 / IPC 载荷 / 子进程任务表 | `elevation.rs`（Windows=`ShellExecuteExW` runas；macOS=常驻助手 → `osascript` 回退）/ `ipc.rs` / `task_runner.rs` |
| 跨进程锁 | `critsec.rs`（Windows=命名互斥体；macOS=`flock(/tmp/sunet-critsec.lock)`，都是 10s 超时） |
| 配置结构 / 默认值 / 方案模型 | `config.rs`（加字段务必带 `#[serde(default)]`） |
| 配置迁移 | `migrate.rs`（纯函数 + 单测，只增不减） |
| 订阅拉取与校验 | `subscribe.rs` |
| 日志 / 脱敏规则 | `logging.rs` |
| 托盘 / 快捷面板行为 / 状态聚合 | `tray.rs` |
| Tauri 命令层 | `src-tauri/src/cmd/{hosts,proxy,dns,profile,system}.rs` |
| 命令注册 / 窗口事件 / 启动分流 | `src-tauri/src/main.rs` |
| 主窗口界面 | `src/main.ts` + `src/tabs/*.ts` |
| **快捷面板界面** | `src/quick.ts` + `quick.html`（独立入口，不是主界面压缩版） |
| 设计令牌（颜色/尺寸/字体） | `src/tokens.css`（**所有颜色只准引用变量**） |
| 组件样式 | `src/app.css` |
| 图标 | `src/icons.ts`（内联 SVG，`svg(name,size)`） |
| 弹层/提示/忙碌态 | `src/ui.ts` |
| **平台判定与文案/键名差异（前端）** | 首帧脚本 `public/os-mark.js`（在 `<head>` 里给 `<html>` 打 `data-os`）＋ `src/platform.ts`（`IS_MACOS` / `AUTH_*` / `MOD_GLYPH` / `markPlatform()` 兜底）＋ `src/app.css` 末尾的 `[data-os="macos"]` 段 |

---

## 3. 不能破坏的不变量（硬约束）

这些是设计文档的拓扑骨架，改动前先想清楚，别顺手"优化"掉：

1. **`main()` 第一行必须是 `--task` 分流**。提权子进程不进 Tauri setup、不建托盘、**不加载 WebView2**、干完就退。这是模型 C 的核心安全边界。
2. **hosts/DNS 必须提权；代理是否提权按平台分叉**（Windows 代理写 HKCU，不需要提权；macOS 写网络服务代理需要 root，所以代理写统一走 `proxy_ops.rs` → 提权任务）。别为了"省一次 UAC"把 hosts 写进主进程——主进程是普通权限。
3. **提权只走平台提权通道**：Windows = `ShellExecuteExW` + `runas`（`CreateProcess` 不触发 UAC，会返回 740）；macOS = 常驻助手（root LaunchDaemon + unix socket，按 peer uid 校验）或 `osascript … with administrator privileges`。
4. **任何写入都必须回读校验**，不一致就报失败（`E2004` / `E4003`）。"调用没抛异常就当成功"是禁止的。
5. **hosts 只动托管区块**（`# >>> SuNet BEGIN … # <<< SuNet END`）：区块外内容逐字节保留；条目为空时**整块删除**而不是写空区块。
6. **回滚必须逆序**：dns → proxy → hosts。回滚失败必须 `recovery_required=true` 并落到界面上，不许吞。
7. **跨进程锁 + 进程内锁双层**：跨进程冲突靠 `with_critical_section`（Windows 命名互斥体 + 10s 超时 + `WAIT_ABANDONED` 接管；macOS `flock` 同语义），进程内靠 `AppState.apply_lock`。
8. **通知只对"用户没预期到的结果"**：在界面上刚点的成功操作不弹气泡；失败/后台发生的事/带撤销窗口的操作才提示。降级链：前端横幅 → 托盘 tooltip `⚠` 前缀 → 日志。
9. **窗口 / 托盘 / 热键 / 提权全部由 Rust 驱动**，前端只调自研命令。capabilities 只开放 `core:default` + `core:window:allow-start-dragging`（面板拖动）。**不要给前端开放更多窗口权限**。
10. **日志默认脱敏**（代理主机名打码、内网地址、用户目录、订阅 URL query）。协议内容不落盘。
11. **危险操作二次确认，且默认焦点不在危险按钮上**（`ui.confirmModal` 已实现：焦点给"取消"，可要求输入指定词）。
12. **加 Tauri 命令必须同时改两处**：命令函数（`cmd/*.rs` 加 `#[tauri::command]`）+ `main.rs` 的 `generate_handler![]` 列表。漏了会在运行时报 "command not found"。
13. **快捷面板是独立 UI**：不要往面板里塞主窗口的标签页/表格/字段网格（用户明确要求过）。它只有瓦片 + chip + 内联确认条。
14. **字体统一微软雅黑**：`--font` 全软件共用；等宽内容走 `--mono`（带雅黑兜底）。新增样式不要写死 `font-family`。

---

## 4. 已踩过的坑（**别再犯**，每条都是实测踩出来的）

### 4.1 dev 端口被占 → 加载到别人的页面
`strictPort` 下 vite 直接退出，webview 连到占用 5173 的其他项目，界面空白但进程一切正常。
现状：dev 端口 5180；且启动 4 秒后会打印两个窗口的**真实 URL / 可见性 / 标题**（`boot` target，info 级）。

### 4.2 窗口 `url` 带查询串会丢
`"url": "index.html?win=quick"` 实测窗口停在 `about:blank`。Tauri 把非 URL 形式的 `url` 当**路径**处理，查询串在部分路径下丢失。
现状：面板用**独立 HTML 入口** `quick.html`（`vite.config.ts` 里配了双入口）。

### 4.3 面板"一闪就没"
`show()` 之后系统会立刻投递一次失焦事件，无条件收起 = 点托盘看不到任何东西。
现状：必须**真正获得过焦点**（`RuntimeStatus.quick_focus_seen`）之后才允许失焦收起。

### 4.4 用"试获取互斥体"探测冲突 → 假警告
`WaitForSingleObject(handle, 0)` 本身会**获取所有权**，两个窗口并发查状态时互相把对方看成"正在写入"。
现状：`critsec::is_contended()` 改为进程内 `AtomicBool` 忙标记；跨进程冲突交给真实等待报错。

### 4.5 「设了 `hidden` 还是显示」
作者样式里的 `display: inline-flex` 会盖掉 `[hidden]` 的 UA 规则 `display:none`。
现状：`app.css` 顶部有全局兜底 `[hidden] { display: none !important; }`。**不要删**，它同时管着空状态、冲突提示、结果卡片。

### 4.6 配置带 BOM → 进只读模式
用户用记事本 / PowerShell 保存 `config.json` 会写入 UTF-8 BOM，`serde_json` 直接报错 → 程序只读启动。
现状：`config::load` 先按字节读，用 `hosts_file::decode`（UTF-8/BOM/UTF-16LE/GBK）解码，再喂给 serde。

### 4.7 `body.scrollHeight` 量不出"自然高度"
`.flyout-body` 是 `flex:1`，内容不足时 `scrollHeight` 返回**被拉伸后**的高度，面板缩不下去。
现状：改为**累加子元素 offsetHeight** + gap + padding，再回调 `quick_fit` 调窗口尺寸。

### 4.8 `cargo build --release` ≠ 生产构建
Tauri 靠 CLI 设的环境变量决定用 `devUrl` 还是内嵌 `frontendDist`；直接 `cargo build --release` 出来的二进制仍然连 dev server（可用 `Select-String` 检查二进制是否含 `localhost:5180` / `assets/main-` 来辨别）。
现状：**出包一律 `npm run tauri -- build`**。

### 4.9 其它已修的问题
- 脚本安全校验曾把**多行 PowerShell 脚本**当控制字符拒掉（会让网卡枚举整体失效）→ 现在只拒 NUL，注入防线是 argv 形式 + 值校验 + `quote_ps`。
- 托盘/菜单写操作在 worker 线程直接调用 → 统一 `run_on_main_thread`。
- `parse_render_idempotent`：托管区块内**原样保留行文本**，不重新排版，否则 100 次切换会漂移。

### 4.10 快捷面板"所有按钮无反应"——真凶是 `getElementById("#…")`
`quick.ts` 的 `bind()` 曾把选择器（`"#q-close"`）直接传给 `document.getElementById`，
后者只接受**裸 id**，永远返回 null，`?.` 又把错误吞掉 —— 结果 12 个按钮**一个监听器都没挂上**，
界面照常渲染、点击却全部无效（2026-10-07 实测定位，已改为 `querySelector`）。
**教训**：整页 `innerHTML` 重渲 + 逐个 `addEventListener` 的组合对这类静默错误零免疫；
排查手法见 §5.7。

### 4.11 PowerShell 每次冷启动约 3 秒，枚举类调用必须走 FFI
`powershell.exe`（加载 NetAdapter/DnsClient 模块）在本机实测 **约 3.0–3.1s/次**（不是交接初期估计的 0.5–1.5s）。
网卡枚举曾遍布状态聚合、DNS 页、写入前白名单校验、主窗口每次打开的 first_run_report，
是"整个软件反应极慢"的主因。现状：枚举走 `GetAdaptersAddresses`（纯内存读，毫秒级）；
`first_run_report` 已于 2026-10-08 随首启引导一并删除，主窗口打开不再做任何枚举；
`Set-DnsClientServerAddress` **已被弃用**（见 §4.14），写入/还原走 netsh 分族命令。

### 4.14 DNS 写入路径：Set-DnsClientServerAddress 没有 -AddressFamily（2026-10-07 实测踩出）
切换方案时 DNS 步骤报 `[E0000] 找不到与参数名称"AddressFamily"匹配的参数`：
`Set-DnsClientServerAddress` 的参数表里**从来没有** `-AddressFamily`，且
`-ResetServerAddresses` 只能两族一起还原，表达不了 §4.3 的"仅还原 IPv4/IPv6"。
修复：写入/还原改走 **netsh 分族命令**（`interface {ipv4|ipv6} set dnsservers / add dnsservers index=N / source=dhcp`），
argv 直传（别名来自枚举白名单），写后仍按注册表回读校验（netsh 写的就是同一对 NameServer 值）。
验证手法（不需要提权）：非提权跑同一条 netsh，**"请求的操作需要提升" = 参数解析通过**；
真实失败（坏网卡名/坏地址）返回 exit=1 且带错误文本。
注意 netsh 的子命令名是 `dnsservers`（`show dns` 是另一组名字），写命令前先 `netsh interface ipv4 set dnsservers /?` 核对。

### 4.15 空网卡方案与"当前正在使用"判定（2026-10-07）
内置直连的 `interface_alias` 是空的，apply 的 DNS 步骤曾直接报 E4001"未选择网卡"。
现在的语义（用户确认的需求）：**方案未指定网卡 = 自动选择当前正在使用的网卡**；
仅当"要写手动 DNS 地址却没指定网卡"时才报错（提示去编辑方案）。
"当前正在使用"的判定：`GetBestInterface(223.5.5.5)` 查路由表（纯本地查询、不发包、毫秒级），
拿承载默认流量的 ifIndex 对回 GAA 枚举结果 —— **不要用启发式**：
Hyper-V 的 vEthernet 报 IfType=6（会被 if_type∈{6,71} 的物理近似误判）且有内部网关，
按"物理+Up+网关"选会错落到虚拟网卡上（实测踩过）。离线兜底链：物理+Up+网关 > 物理+Up > Up > 第一个。
前端 `DnsInterfaceView.is_in_use` 供 DNS 页与方案编辑器做默认选中与"（当前使用）"标注。

### 4.12 诊断"点击无反应"时的三个假象
- **CDP `el.click()` 不等于真实输入**：合成点击绕过 OS 命中测试，输入管线坏了它照样"通过"。
  真实验证要用 `SendInput`/`mouse_event` 发真实鼠标事件（§5.7）。
- **Git Bash 的 `cat` 看 UTF-8 中文是乱码**：那只是 GBK 控制台显示假象（config.json 实际是干净 UTF-8）。
  判断文件编码要用 node / `Get-Content -Encoding UTF8`，别信终端回显。
- `document.visibilityState` 对隐藏的 Tauri 窗口**仍报 "visible"**；判断窗口可见性要用 Win32
  `IsWindowVisible`（按 PID 枚举）。另外 PowerShell 里 `WindowFromPoint` 的 POINT 传参不可靠，别用它做命中诊断。

### 4.13 订阅"解析 0 条 + 待确认清单找不到"（2026-10-07，双重坑）
- **`char::is_control()` 包含 `\t`**：传统 hosts 用 TAB 分隔 IP 与域名，
  `parse_content` 的控制字符检查曾把 TAB 分隔的整份订阅 100% 拒成"含控制字符的行"
  （实测：SteamHostSync 33 行全拒 → 解析 0 条 → 空清单被暂存还要求用户确认）。
  现已排除 `\t`。**教训**：hosts 解析的单测必须覆盖 TAB 分隔样本，不能只测空格。
- **首次导入解析出 0 条时按失败处理**（`sync()` 里的防线）：错误页 / 全非法内容不应
  暂存一个"空变更"让用户确认 —— 那个确认界面里什么都没有。
- **确认清单的可达性**：diff 卡曾只在"刚点完同步"的那一次渲染里出现，刷新/重启后
  只剩一行"有待确认的变更"文字。现在 `renderSources` 每次渲染都拉 `sources_staged`
  自动列出所有待确认清单，通知里的「查看变更」也正确跳到 Hosts 页（曾落到没有订阅 UI 的方案页）。

### 4.16 全局热键：从「手打文本框」到「按键捕获」+ 四个不稳定根因（2026-10-07）

需求原话是「快捷键设置完全靠输入，而且切换的稳定性极差」。查下来是**四个独立缺陷叠加**，
每一个都会单独表现为「热键不好使」：

1. **三套互不相认的键名表**。界面只有自由文本框；`cmd/system.rs` 自研的 `key_to_vk` 只认
   字母/数字/F1–F24 和少数命名键；插件 `global-hotkey` 另有一套（`MINUS`/`NUMPAD5`/`BRACKETLEFT`/`ARROWUP`…）。
   后果：`Ctrl+Minus` 这类组合「检测可用性」报无法解析、「应用」却成功。
   现在 `system.rs` 的 `mods_of` / `key_of` 是**唯一真源**，同时收 `KeyboardEvent.code`（`KeyS`/`Digit1`/`ArrowUp`）、
   插件名与字符写法，输出统一规范名（`HotkeySpec::text()`，修饰键固定 Ctrl→Alt→Shift→Super 序）。
   新增 `E5004`（无法识别的组合）；`cargo test` 里的 `capture_output_round_trips` 就是「录制控件输出 → 后端必须认得」的契约测试。
2. **改绑后旧组合还在**。旧代码 `register_shortcut` 里 `unregister(shortcut)` 释放的是**新**组合
   （插件按组合存注册表），旧组合一直留在系统热键表里 → 新旧同时触发。
   现在一律 `release_all_shortcuts()`（本程序只有这一个全局热键，整表释放才可靠）。
3. **自我占用误判 E5001**。`is_hotkey_available` 用 `RegisterHotKey(hWnd=NULL, PROBE_ID, …)` 试探，
   分不出「别人占用」与「自己占用」，于是同一组合再应用一次就报「已被其他程序占用」。
   实测证据（`%APPDATA%\SuNet\logs\sunet-2026-10-07.log` 13:34:47）：
   `[WARN] hotkey: 全局热键注册失败：[E5001] …（Ctrl+Alt+S 已被其他程序占用）` 紧接着
   `[INFO] hotkey: 全局热键注册成功：Ctrl+Alt+S`。
   现在 `shortcut_check` 返回 `ShortcutProbe { binding, is_current, available }`，
   `shortcut_set` 先判「目标 == 当前正在生效的组合」直接短路确认（`probe_available` 的注释里写明了这个前提）。
4. **连按 N 次 = N 次同向切换**。`hotkey_toggle_direct` 每次按键都 spawn 一个新线程，`apply_profile` 内部
   用**阻塞** Mutex 串行化 → 连按不报错只排队，且每个线程在**排队前**就读了 `cfg_clone()`，
   于是一串按键全部算出同一个目标、逐个重复 apply（含 hosts/DNS 时连弹 N 次 UAC、写 N 份快照）。
   实测证据（同一日志 13:35:05–13:35:27）：`已切换到默认直连` 之后连续 **6 次**
   `已切换回「Github」`，并产生 2 份同因快照。
   现在 `cmd/profile.rs` 加了 `ToggleGate`（`AtomicBool` + `compare_exchange`），**在热键回调线程上抢闸门**，
   抢不到就直接丢弃本次按键（`log::debug!(target: "hotkey", …)`）。切换是幂等的反向操作，排队毫无意义。

### 4.16.1 前端：按键捕获控件（`src/settings.ts`）
- 点方框 → 捕获阶段监听 `document` 的 `keydown`，**必须 `preventDefault() + stopImmediatePropagation()`**：
  `ui.openModal` 在 `document` 上挂了 Escape 关闭监听，不吃掉事件就会「按 Esc 想取消录制，结果弹层关了」。
- 录制期间**临时让出全局热键**（`shortcut_capture(on)`）：全局注册是系统级的，否则按到当前生效的组合
  会真的切一次方案（含 hosts/DNS 还会弹 UAC）。让出/收回必须**串行**（`queueCapture` 里的 promise 链）：
  并发 `await` 会让「收回」先落地、「让出」后落地，热键就永久失效了。
- **监听泄漏会让整个界面键盘全死**：监听挂在 `document` 上，若弹层被关掉时没摘，之后每次按键都被
  `preventDefault` 吃掉。所以除了点遮罩（`.backdrop` 上的 `pointerdown`）停止录制，
  事件里还有 `abandonIfClosed()` 自查 `boxHotkey.isConnected`，不依赖调用方「记得清理」。
- **`applied` vs `pending`**：`applied` = 真实生效值，**只有它**写进「保存」的载荷；`pending` = 刚录制的候选，
  只有它发给 `shortcut_set`。旧代码保存时展开 `...s` 但没覆盖 `shortcuts`，而 `settings_save` 是
  **整体替换** `c.settings` —— 「先点应用、再点保存」会把刚改好的热键写回旧值（改绑静默回滚）。
- 界面显示的键帽只是展示（`GLYPH` 把 `Minus` 显示成 `-`、`ArrowUp` 显示成 `↑`），**配置里存的永远是规范名**。

### 4.16.2 不再显示「未启用任何方案」（2026-10-08 全量统一）
`active_profile_id` 为 `None` 时，顶部下拉与方案页显示内置「默认 · 直连」
（`main.ts::renderHeader` 删掉了空 value 选项 —— 它还会让 `select.value=""` 把「切换」按钮永久置灰）。
托盘侧原先还残留「未启用任何方案」：tooltip 与托盘菜单项（`tray.rs`）、快捷面板状态徽标
（`quick.ts`）。2026-10-08 起三处统一为同一口径 `effective_headline`：
1. 有活动方案 → 方案名；
2. 没有活动方案，但三层里有实际生效的（代理开着 / 有托管 hosts 条目 / DNS 手动）→ 「自定义设置」；
3. 都没有 → 「默认 · 直连」。

用户手动开启设置（托盘开关代理、直接应用 hosts/DNS）后，不再被错误提示成"未启用方案"。

### 4.17 热键切换后界面不刷新（2026-10-07 用户实测报出）
现象：按热键切到内置「默认 · 直连」，**代理确实被清掉了，但主窗口顶部下拉与快捷面板仍显示切换前的方案**。

根因是**前端没有第二个状态来源**，所有刷新都挂在事件上：
`src/main.ts:325 listen("sunet://report") → refreshState(true)`、`src/quick.ts:35 listen("sunet://report") → refresh()`。
发出该事件的只有 `cmd::emit_report`，而它被 `profile_apply`（`cmd/profile.rs:99`）、`dns_*`、`hosts_*`、`proxy_*` 调用 ——
**`hotkey_toggle_direct` 是唯一一个改了状态却不推报告的入口**（它只 `notify::send`，前台只弹一条 toast）。
于是配置里的 `active_profile_id` 已经变了，两个窗口却谁都不知道。

修两处：
1. `cmd/profile.rs` 的 `Ok(ToggleDirect::Switched { .. })` 分支开头加 `crate::cmd::emit_report(&a, &message);`
   → 主窗口 `refreshState(true)`、快捷面板 `refresh()` 都会跟上（含失败/回滚的报告也一并可见）。
2. `cmd/mod.rs` 新增轻量事件 `EVENT_REFRESH = "sunet://refresh"` + `emit_refresh()`；
   `tray::show_quick_anchored` 在 `win.show()` 之后调一次，`quick.ts` 里 `listen("sunet://refresh", refresh)`。
   原因是**快捷面板是常驻窗口**（失焦只 `hide()` 不重建，见 `main.rs` 的 `WindowEvent::Focused(false)`），
   WebView 不会自己重拉状态 —— 凡是不推报告的状态变更（`profile_delete`、`settings_save`、自启开关…）
   都会在面板上留下陈旧显示。

### 4.18 无边框窗口在 Windows 上「首帧残留系统标题栏」（2026-10-07 实测踩出）

目标：主界面不要系统标题栏，只留自绘顶栏。`tauri.conf.json` 里 main 窗口改成 `"decorations": false, "shadow": true` 之后，**窗口顶端仍然有一条系统标题栏**（速网 SuNet + − □ ✕），其下才是自绘顶栏。

根因在 tao 0.37.1：tao **不**删除窗口样式里的 `WS_CAPTION`（`platform_impl/windows/window_state.rs:238` 的 `to_window_styles()` 无条件 `|= WS_CAPTION | WS_CLIPSIBLINGS | WS_SYSMENU`，`resizable` 再加 `WS_SIZEBOX`），去标题栏完全依赖 `WM_NCCALCSIZE` 的返回值：
`event_loop.rs:2074` `if wparam == WPARAM(0) || window_flags.contains(MARKER_DECORATIONS) { DefWindowProc }` —— 只有 `wParam = TRUE` 时才走自绘分支（返回 0 = 整窗都算客户区）。
而 `CreateWindowExW` 之后系统第一次计算用的是 `wParam = FALSE`，于是落到 DefWindowProc 默认分支，按样式算出非客户区 → 系统**真把标题栏画出来**（实测非客户区顶边 45px，正好一条标题栏）。
之后任何一次移动/缩放/最大化都会重新触发带 `wParam = TRUE` 的计算，标题栏随即消失 —— 快捷面板每次唤出都要重新定位，所以从没出现过这个现象，只有主窗口暴露。

修法（已落盘）：`src-tauri/src/main.rs` 的 `.setup` 里、窗口显示之前，对 `main` / `quick` 各调一次 `os::winapi::force_frame_recalc(hwnd)`
（新增于 `src-tauri/src/os/winapi.rs`：声明 `SetWindowPos` + 常量 `SWP_NOSIZE|SWP_NOMOVE|SWP_NOZORDER|SWP_NOACTIVATE|SWP_FRAMECHANGED`，语义就是"再触发一次框架重算"）。
另新增 `window_minimize` 命令（`cmd/system.rs` + `api.ts` 的 `windowMinimize()`），`×` 沿用原 `window_hide`（隐藏到托盘，进程继续跑）。

**判据（重要）**：不要用 `PrintWindow`！它会凭样式里的 `WS_CAPTION` **凭空虚画**出一条标题栏，在已修好的窗口上照样画出白条。可靠判据是非客户区几何：
`GetWindowRect` 与 `ClientToScreen(0,0)` 之差 —— `T=45` 有系统标题栏，`T=0` 才是真无标题栏（修复后本机实测 `NC(L=11 T=0 R=11 B=11)`，修复前同一流程是 `T=45`）。
探针脚本：`%TEMP%\sunet-launch-measure.ps1 -Exe <exe> -Label <标签>`（杀进程→启动→量 insets→ShowWindow→再量→截图）。两个坑：PowerShell 必须先 `SetProcessDpiAwareness(2)`；C# 动态类里的静态方法**不能叫 `Main`**（被当成入口点，编译直接失败）。

顺带事实：无边框窗口在 tao 里缩放与拖动是两条路 —— 顶边由 tao 的 `WM_NCHITTEST` 分支自己处理，左右/底边/角落交给 DefWindowProc；客户区一律 `HTCLIENT`，所以自绘顶栏的拖动只能靠前端 `data-tauri-drag-region="deep"`（tauri `drag.js`：deep 让整棵子树都算拖动区，但 `a/button/input/[tabindex≠-1]` 等可交互元素除外；双击该区域触发 `plugin:window|internal_toggle_maximize`，在 `core:default` 默认权限集内，不必加 capability）。
修复后实测：`top+3 → HTTOP`、`top+20 → HTCLIENT`、`left/right/bottom → HTLEFT/HTRIGHT/HTBOTTOM`、右下角 → `HTBOTTOMRIGHT`。

### 4.19 macOS 平台层：三处"看着能共用、其实不能"（2026-10-07）

1. **代理写入在 macOS 需要 root，Windows 不需要**。原来 `apply.rs` 直接调 `system_proxy::restore` / `hard_clear`，搬到 macOS 就是"回滚时静默失败"——最危险的那种 bug。现在代理写统一走 `proxy_ops.rs`，由 `proxy_write_needs_root()` 决定进程内直写还是提权任务（`proxy_write` / `proxy_restore` / `proxy_clear`）。
2. **跨进程锁不能用数据目录里的文件**：助手以 root 运行、`HOME=/var/root`，锁文件放 `~/Library/…` 等于两个进程锁的不是同一个文件 → 改用 `/tmp/sunet-critsec.lock`（`flock` + 10s 超时，语义与命名互斥体对齐）。
3. **提权子进程的 argv 不要经 shell**：`networksetup` 的服务名含空格（`iPhone USB`），macOS 侧的外部命令全部走 `Command::new(路径).args([...])` 直传；只有 `osascript` 那条必须拼字符串，用 POSIX 单引号转义（`sh_quote`）后再嵌进 AppleScript。

---

## 5. 怎么证明改动是对的

### 5.1 已实测通过（可作为回归基线）

| 项 | 判据 |
|---|---|
| 单测 | `cargo test` → **55 passed**（Windows；源码共 66 个 `#[test]`，macOS 专属 11 个不参与编译） |
| 前端 | `npm run check` 零错误；`npm run build` 双入口产物 |
| 后端 | `cargo build` 零警告 |
| IPC 延迟 | CDP 实测 `get_app_state` ≈ 2–6ms；`dns_interfaces`（FFI）≈ 8ms；主窗口五个标签页切换全部 ≈ 110ms（DNS 页曾 3080ms） |
| 面板按钮 | CDP `DOMDebugger.getEventListeners` 确认每个按钮都有 click 监听；`SendInput` 真实点击关闭按钮后窗口 `IsWindowVisible=false`；点主题按钮 `data-theme` 变化；点方案 chip 出现内联确认条 |
| 启动三态 | 裸启（主窗显示）/ `--minimized`（只托盘）/ `--panel`（只面板）：进程存活、托盘建成、日志写入 |
| 页面归属 | 日志 `窗口 main：url=… 可见=… 标题=…` 与 `窗口 quick：…` 应为本项目页面（dev 是 `localhost:5180/…`，生产是 `tauri.localhost/…`） |
| 前端 ↔ 后端 | 日志出现 `ipc: get_app_state`（debug 级）即证明 JS 与 IPC 通 |
| 快捷面板 | DOM 自检 `tiles=4 / chips=2 / tabs=0 / cards=0`；高度自适应日志 `面板高度自适应 → N`；几何日志底边固定在工作区上方 |
| 主窗口 | DOM 自检 `tabs=5 / cards=3 / layers=3` |
| 字体 | `getComputedStyle(document.body).fontFamily` 以 `"Microsoft YaHei UI"` 开头 |
| 热键 | 启动日志 `全局热键注册成功：Ctrl+Alt+S`（真实注册，不是配置回显）。**语义（2026-10-07 起）**：按一下切到内置「默认 · 直连」，再按一下切回 `previous_profile_id` 记住的方案；托盘菜单的「清除代理设置」仍是原语义 |
| 无边框窗口（Windows） | 首帧即 `decorations:false` 生效：`NC(L=11 T=0 R=11 B=11)`（脚本 `%TEMP%\sunet-launch-measure.ps1`）；`WM_NCHITTEST`：`top+3→HTTOP`、`left/right/bottom→HTLEFT/HTRIGHT/HTBOTTOM`、右下角→`HTBOTTOMRIGHT`、客户区→`HTCLIENT`（2026-10-07 在 release 包上复核） |
| 单实例 | 第二个实例立即退出码 0 退出，首实例存活 |
| 提权通道 | 手工造载荷 + `--task probe` 端到端（见 §5.3） |
| 配置容错 | 故意带 BOM 保存后仍能加载（`只读=false`） |
| macOS 平台层语法 | `rustfmt --edition 2021 --check src/os/macos/*.rs src/helper/*.rs` 无语法错误（**只是语法**，不是编译，更不是实测） |
| macOS 平台层编译 + 单测 | CI 的 `macos-latest` job（Actions run **37639760440**，tag `v0.0.1` → commit `91bd35d`）：`cargo test` → **59 passed / 0 failed**、`npm run check` 零错误、`--target universal-apple-darwin --bundles dmg` 构建成功（`SuNet_0.0.1_universal.dmg` 5 821 694 B）。**这只证明能编译 + 单测通过，不等于真机跑通**（见 §6.4） |
| CI 抓到的两个 macOS 专属 bug（已修） | ① 编译错误 4 处：`src-tauri/src/helper/install.rs:57/60/70` 少了 `?`（`paths::exe_path()` 返回 `io::Result<PathBuf>`）；`src-tauri/src/os/macos/dns_client.rs:384` 的 `write_dns` 尾表达式返回 `Result<String>` 而签名是 `Result<()>`（补 `?` + `Ok(())`）；另加测试闭包 `_up` 未用参数。<br>② 单测 `os::macos::dns_client::tests::parse_order_extracts_service_and_device` 失败：`parse_service_order` 原本用 `strip_prefix('(')` + `strip_suffix(')')`，要求**整行**被括号包住，而 `networksetup -listnetworkserviceorder` 的服务行是 `(1) Wi-Fi`（括号只占行首一段，后面还有名字）→ 解析出 0 个服务。已改为「取第一个 `)` 之前的为序号/硬件端口、之后的为名字」，并在本机用独立 harness 调真实函数复验（含 `*停用` 与 `USB 10/100/1000 LAN` 这类带斜杠的名字） |

### 5.2 启动冒烟 + 日志判读（最常用）

```powershell
# 起 dev server（或跳过，用生产包）
Start-Process npm.cmd -ArgumentList "run","dev" -WorkingDirectory "d:/Documents/Code/SuNet" -WindowStyle Hidden

$log = Join-Path $env:APPDATA "SuNet\logs"
Get-Process SuNet -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
$p = Start-Process -FilePath ".\src-tauri\target\debug\SuNet.exe" -ArgumentList "--panel" -PassThru
Start-Sleep -Seconds 12
$f = Get-ChildItem $log -Filter *.log | Sort-Object LastWriteTime -Descending | Select-Object -First 1
Get-Content $f.FullName -Encoding UTF8 -Tail 10      # ← 必须 -Encoding UTF8，否则中文乱码
Stop-Process -Id $p.Id -Force
```

想看 debug 级日志（`ipc:` / `tray:` / `panel:`）就临时把 `config.json` 的 `settings.log_level` 改成 `debug`，
**验完改回 `info`**。

### 5.3 提权任务通道手工验证（不需要真点 UAC）

```powershell
$ipc = Join-Path $env:LOCALAPPDATA "SuNet\ipc"; New-Item -ItemType Directory -Force -Path $ipc | Out-Null
$id = [guid]::NewGuid().ToString()
$in  = Join-Path $ipc "task-$id.in.json"; $out = Join-Path $ipc "task-$id.out.json"
$json = '{"version":1,"task":"probe","created_at":"2026-10-06T00:00:00+08:00","data":{}}'
[System.IO.File]::WriteAllText($in, $json, (New-Object System.Text.UTF8Encoding $false))   # 不要 BOM
& ".\src-tauri\target\debug\SuNet.exe" --task probe --in $in --out $out
"exit=$LASTEXITCODE"; Get-Content $out -Raw; "载荷已删=$( -not (Test-Path $in) )"
```
期望：`exit=0`、结果 JSON `ok:true`、载荷文件被删除。可换 `task` 为 `hosts_apply` / `dns_flush` 等验证其它分支
（`hosts_apply` 的 `data.entries` 传 `HostsEntry` 数组）。

### 5.4 DOM 探针法（**验证前端逻辑最有效的手段**）

无法开 DevTools 时，用 `eval` 把结果编码进 URL，再用 `window.url()`（已验证可靠的观察通道）读回来。
临时加在 `main.rs` 启动后的后台线程里：

```rust
let _ = w.eval(
    "location.replace('http://localhost:5180/?w=' + \
     (location.pathname.indexOf('quick')>=0?'quick':'main') + \
     '&tiles=' + document.querySelectorAll('.tile').length + \
     '&font=' + encodeURIComponent(getComputedStyle(document.body).fontFamily.slice(0,26)) + \
     '&lock=' + encodeURIComponent(getComputedStyle(document.getElementById('status-lock')||document.body).display))",
);
// 3 秒后再 log 一次 w.url() 即可读到结果
```
注意：`__TAURI_INTERNALS__` 在任何页面都存在，**所以"eval 能执行"不等于"加载的是我们的页面"**——
要靠 `w.url()` 与页面里的 DOM 一起判断。**验证完记得删掉探针代码。**

### 5.5 截图核对（UI 改动必做）

```powershell
# 1) 从日志取几何（debug 级）：窗口 quick 几何：x=… y=… w=… h=…
# 2) 抓屏（必须先 SetProcessDPIAware，否则坐标会按 DPI 缩放错位）
Add-Type -TypeDefinition 'using System.Runtime.InteropServices; public class D{ [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }'
[D]::SetProcessDPIAware() | Out-Null
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap($w,$h); $g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($x,$y,0,0,(New-Object System.Drawing.Size($w,$h)))
$bmp.Save("d:\Documents\Code\SuNet\tmp-shot.png", [System.Drawing.Imaging.ImageFormat]::Png)
```
坑：`CopyFromScreen` 抓的是**合成后屏幕**，被遮挡时抓到的是别的窗口。
面板是 `alwaysOnTop` 所以直接可抓；主窗口需要先置顶（`SetWindowPos(hwnd, -1, …)`）或干脆用 DOM 探针代替。
**抓完记得删掉 png。**

### 5.6 生产版验证（必走 CLI）

```bash
npm run tauri -- build --no-bundle
# 判据（二进制里应含内嵌资源、且运行后 URL 是 tauri.localhost）
$b=[IO.File]::ReadAllBytes("d:\...\target\release\SuNet.exe"); $t=[Text.Encoding]::ASCII.GetString($b)
$t.Contains("assets/main-") -and $t.Contains("quick.html")     # True
# 运行 release --panel 后日志应显示 url=http://tauri.localhost/quick.html
```

### 5.7 真实输入与监听器验收（"点击无反应"类 bug 的标准流程，2026-10-07 实战沉淀）

```text
① 页面收到事件了吗？  CDP 注入 document 捕获阶段监听器 → SendInput 真实点击 → 读记录。
     收到了 → 输入管线没问题，查按钮监听器/处理逻辑；
     没收到 → 查窗口命中（不要用 PowerShell 的 WindowFromPoint，POINT 传参不可靠）。
② 按钮有监听器吗？    CDP：Runtime.evaluate 拿 objectId → DOMDebugger.getEventListeners。
③ 处理器发出 IPC 吗？ monkeypatch window.__TAURI_INTERNALS__.invoke 记录调用（api/core 每次调用
     都现读该属性，补丁有效），再真实点击，读记录；同时挂 window error / unhandledrejection 收异常。
④ 窗口真的变了吗？    用 Win32 IsWindowVisible（按 PID 枚举）判断，别信 visibilityState。
真实鼠标点击：SetProcessDPIAware 后 SetCursorPos(物理坐标) + mouse_event(LEFTDOWN/LEFTUP)。
物理坐标 = (window.screenX + rect.左) × devicePixelRatio（CSS px → 物理 px）。
```

---

## 6. 待办与已知缺口

### 6.1 尚未实机验证（需要交互式点击 / 真提权环境，**优先级最高**）
1. 真实 UAC 授权后的 hosts / DNS 写入全链路。
2. `ReplaceFileW` 被杀软占用时的回退路径（`E2001` → 截断写入 → 回读校验）。
3. 代理开关是否对已运行的 Chromium 系应用即时生效（依赖 `InternetSetOptionW`）。
4. 反复切换 100 次后 hosts 是否膨胀（逻辑上已保证：内容相同直接跳过写入，但值得实测）。
5. 快捷面板在**上/下/左/右**四类任务栏下的贴边效果（当前只在"底部任务栏 + 150% 缩放"下验证过）。
6. 面板拖动（`data-tauri-drag-region` + `allow-start-dragging`）与失焦收起的实际手感。
7. 设计文档 §16.3 的实机验收矩阵：标准用户账户 / 域环境 / 多网卡 / 中文输入法吞键。
8. **按键捕获控件的真实按键录制**（§4.16.1）：点方框 → 按 `Ctrl+Alt+Q` → 点「应用」→ 再按热键应只切一次；
   以及「录制中按到当前生效组合不会触发切换」「Esc 能取消录制且弹层不关」。
   注意：`ToggleGate`（§4.16 第 4 条）需要**快速连按热键**才能验证 —— 日志中应只出现一次切换。
9. **热键切换后两个窗口的显示是否跟上**（§4.17）：按热键切直连后，主窗口顶部下拉应变成
   「默认 · 直连（内置）」，快捷面板的方案 chip 同步变化；再唤出面板不应看到旧方案。
   （此条用户已在 0.0.1 实测报出过一次，修复后需复验。）

10. **无边框自绘顶栏的实际手感**（§4.18）：拖顶栏移动窗口、双击顶栏最大化/还原、点 `−` 最小化、点 `×` 隐藏到托盘。
    （几何与命中测试已实测通过，缺的是真实鼠标拖拽/双击这一步。）
11. **macOS 全套**（§6.4）：整个平台层从没在真机上跑过，按 §6.4 的实测清单逐条来。
12. **macOS 代理关闭后的还原**：`scutil --proxy` 是否回到原值；有侧车文件（`proxy_prev.json`）应优先吃侧车，没有时把传入快照写到所有活动服务——两条路径都要看。
13. **macOS 提权助手的边界**：`--helper-install` 是否被 BTM 拦、卸载是否干净（plist / 二进制 / socket 三样）、SSH 登录（非 console 用户）时 peer uid 校验是否按预期拒绝。

### 6.2 未实现（设计方案里标为 v2 或"不做"）
WinHTTP 代理、PAC 代理模式、TUN/虚拟网卡、hosts 通配符规则、系统 DoH 写入（只提供端点参考表与测速）、
自动更新下载器与"检查更新"跳转（`config.channels` 已留占位，加通道不需要再做迁移）。

### 6.3 可优化点（按性价比排序）
1. **面板圆角**：当前是无边框不透明窗口，四角是直角（Win11 飞行面板通常圆角）。想做就开 `transparent: true` + 内层圆角卡片，但 WebView2 透明窗口有观感风险（黑角/闪烁），**改前先截图对比**。
2. ~~安装包（NSIS）尚未产出~~ → **已产出**（2026-10-07）：`npm run tauri -- build`，产物 `src-tauri/target/release/bundle/nsis/SuNet_<版本>_x64-setup.exe`（约 2.2 MiB，LZMA 压缩后）。NSIS 工具链缓存在 `%LOCALAPPDATA%\tauri\NSIS`，离线重打包不用再下载。安装包未签名，首次运行会有 SmartScreen 提示（"仍要运行"）。
3. `cmd/system.rs` 864 行偏大，`first_run_*` 与 `logs_export` 可拆出去。
4. ~~DNS 走 PowerShell~~ → **读路径已完成**：枚举换 `GetAdaptersAddresses` FFI（2026-10-07）。剩余的写入路径也已离开 PowerShell：netsh 每次约 0.1–0.3s，仅在真实写入时发生。
5. 面板目前每次操作后整页重绘（`render()` 重写 innerHTML）。对 372px 小窗足够快，但要加动画就得改成局部更新。

### 6.4 macOS（**已实现，未真机验证**）

平台层按 §2 的映射写完了：`src-tauri/src/os/macos/*`（`net.rs` / `dns_client.rs` / `system_proxy.rs` / `hosts_io.rs` / `fs_security.rs` / `privilege.rs` / `mod.rs`）+ `src-tauri/src/helper/*`（常驻提权助手）。共用层只动三处：代理写抽成 `proxy_ops.rs`、`critsec.rs` 分平台、`Cargo.toml` 把 `winreg` 挪进 `[target.'cfg(windows)'.dependencies]` 并加 `libc`。Windows 侧行为不变（`cargo test` 55 项仍全绿）。

| 能力 | Windows（已实测） | macOS（已实现，**未真机验证**） |
|---|---|---|
| 系统代理 | 注册表 `Internet Settings` + `InternetSetOptionW` 通知 | **只写 HTTP**（`-setwebproxy`——2026-10-10：三项同端口会让只支持 HTTP 的代理端口收到 SOCKS 而断网）× 全部活动服务，并清掉 HTTPS / SOCKS 残留；读回 `scutil --proxy`；还原点 `~/Library/Application Support/SuNet/proxy_prev.json` 逐协议记录（`fmt=2`，旧格式整份丢弃） |
| DNS | `GetAdaptersAddresses` 枚举（~8ms）+ `netsh` 分族写 | `networksetup -listnetworkserviceorder` + `-listallhardwareports` + `ifconfig`（2s 缓存）；`-setdnsservers` 分族写、写后回读校验；刷缓存 `dscacheutil -flushcache` + `killall -HUP mDNSResponder`（走提权任务 `dns_flush`） |
| hosts | `…\drivers\etc\hosts` + `ReplaceFileW` | `/etc/hosts` + 同目录 `.tmp` → `rename` → `chown 0:0` / `chmod 0644` |
| 提权 | `ShellExecuteExW` runas + `--task` 子进程 IPC | ① 常驻助手：root LaunchDaemon（`launchctl bootstrap system`）+ `/var/run/sunet-helper.sock`，按 peer uid（root 或 console 用户）校验，`--helper-install` 装一次免密码；② 回退 `osascript … with administrator privileges`，弹一次系统授权框（这条路径**没有超时**——等用户输密码不算卡死；取消按 `-128` 或 "User canceled" 判 `E1001`）。一次方案切换最多弹**一次**（代理与 hosts / DNS 同批，见顶上 2026-10-10 那条） |
| 自启动 | 计划任务（`/RL LIMITED` `/SC ONLOGON`） | `~/Library/LaunchAgents/com.sunet.desktop.autostart.plist` + `launchctl bootstrap gui/<uid>` |
| 全局热键 | `global-shortcut`，展示 `Ctrl+Alt+S` | 同一个插件；⌘ 在录制里规范名为 `Super`（后端 `mods_of` → `MOD_WIN`，插件认识 `Super`），展示层由 `platform.ts` 的 `MOD_GLYPH` 渲染成 ⌃⌥⇧⌘ |
| 窗口外观 | 无边框自绘顶栏（§4.18） | 原生交通灯：`tauri.macos.conf.json` 的 `titleBarStyle: "Overlay"` + `hiddenTitle` + `trafficLightPosition {x:14,y:18}`；CSS 隐藏自绘 `−` / `×` 并给 `.topbar` 留 88px 左内边距。`data-os` 由首帧脚本 `public/os-mark.js`（两个 HTML 的 `<head>` 经典脚本引入）在**首帧前**打上（`src/platform.ts` 只做兜底重写） |
| 签名 | 未签名（SmartScreen） | `signingIdentity: "-"`（ad-hoc；没有开发者证书，因此不签名不公证 → 首次打开要在系统设置里放行） |
| 界面文案 | UAC / 计划任务 / WinINET | 系统授权框 / 登录项 / 网络服务代理（`src/platform.ts` 按 `navigator.userAgent` 切） |

**已知未验证点（按风险排序）**
1. `osascript` 提权路径的取消/失败判定，以及"取消后不留半套配置"（靠 `apply.rs` 的回滚）。
2. 常驻助手的安装/鉴权/卸载：BTM 是否拦 `bootstrap system`、socket peer uid 校验、卸载残留。
3. `pick_in_use()` 的服务选择：只有 Wi-Fi、或 Wi-Fi + 有线同时在用时，写到的服务是否是用户实际在用的那个。
4. 按架构构建的 dmg 产物（`--target aarch64-apple-darwin` / `x86_64-apple-darwin`）、ad-hoc 签名后的 Gatekeeper 提示。
5. 前端 `data-os="macos"` 的样式/文案分支是否完整（自绘按钮隐藏、顶栏留白、授权文案）。用户实测反馈过"logo 与交通灯重合"，2026-10-10 按「首帧打标 + 安全区 88px」修，**仍需真机复核**（本机没有 macOS）。

**本机验证边界**：本机没有 macOS，交叉检查也做不了 —— 没有 `cc`，`cargo check --target aarch64-apple-darwin` 会死在 `objc2-exception-helper` 的 build script（`failed to find tool "cc"`）。所以"macOS 能不能编译"只能由 CI 回答（**CI 已给出答案：能编译、59 项单测通过、dmg 能出——当时为 universal 构建，见 §5.1**）；本地能保证的只有 `rustfmt --edition 2021 --check` 无语法错误 + Windows 侧 `cargo test` 不回归。

**Mac 实测清单（拿到 mac 后按顺序跑）**
1. `npm ci` → `npm run tauri -- build -- --target aarch64-apple-darwin --bundles dmg`（Intel 机器换 `x86_64-apple-darwin`）→ 装 dmg（首次要在「系统设置 → 隐私与安全性」放行）。
2. `cd src-tauri && cargo test` → 预期 **59 项**（66 − 7 项 Windows 专属）。
3. 启动三态冒烟：裸启 / `--minimized` / `--panel`；日志无 error；托盘菜单与 ⌘ 组合热键可用。
4. hosts：写入 → `cat /etc/hosts` 看托管区块 → 还原 → 区块消失；`ls -l /etc/hosts` 应为 `root wheel` `0644`。
5. 代理：开关各一次，对比 `scutil --proxy` 与「系统设置 → 网络 → 代理」；关闭后是否回到原值。
6. DNS：`networksetup -getdnsservers Wi-Fi` 前后对比；切"自动(DHCP)"后应落回 `aren't any DNS Servers`；刷缓存无报错。
7. 提权助手：`sudo /Applications/SuNet.app/Contents/MacOS/SuNet --helper-install` → `--helper-status`；之后写 hosts **不该**再弹授权框。
8. 无助手路径：`--helper-uninstall` 后再写 hosts，应弹一次系统授权框；**取消**时应报 E1001 且配置回到操作前。
9. 外观：无残留 Windows 标题栏、无重复的最小化/关闭按钮、交通灯不被顶栏内容压住、快捷面板贴边正常。
10. 自启：开关各一次，检查 `~/Library/LaunchAgents/com.sunet.desktop.autostart.plist` 与 `launchctl list | grep sunet`。

---

## 7. 约定（写代码时请遵守）

- **注释写"为什么"**，用中文。读代码的人需要知道"这里为什么不那么做"，不是"这行在赋值"。
- **错误必须有码**：新增错误先加到 `error.rs`（常量 + `text()` 中文文案 + 需要时 `exit_code()` 映射），再在业务里 `AppError::coded(EXXXX).with_detail(...)`。前端靠 `code` 做分支，别只靠 message。
- **日志 target** 沿用：`hosts` / `proxy` / `dns` / `ipc` / `elevation` / `task` / `tray` / `panel` / `subscribe` / `recovery` / `exit` / `boot`。
- **配置新增字段一律 `#[serde(default)]`**，并在 `migrate.rs` 里为老版本补默认；**禁止删除用户数据**（迁移只增不减）。
- **前端颜色只准用 `tokens.css` 的变量**；间距用 `--sp-*`，圆角用 `--r-*`。写死十六进制 = 深色模式下必然出问题。
- **面板与主窗口共用** `api.ts / ui.ts / theme.ts / icons.ts / tokens.css`，但**外壳各自独立**。
- 改动 hosts / 事务 / 提权三者任一，**必须**跑 `cargo test` 并补一条单测。
- **平台分叉原则**：能共用就共用（`ipc.rs` / `hosts_file.rs` / `apply.rs` / `cmd/*`），平台差异收敛到 `os/<plat>/*` 与 `proxy_ops.rs` 这类小函数（`proxy_write_needs_root()` / `flush_needs_root()` / `privilege::detect()`）。**前端不许自己判断平台**——一律走 `src/platform.ts`，样式则用 `<html data-os>` + CSS。
- 提交前自检：`npm run check` + `cargo test` + `cargo build`（零警告）+ 启动冒烟（日志无 error）+ 涉及 UI 则截图看一眼。

---

## 8. 常用改动 recipe

**加一个 Tauri 命令**
1. `cmd/<域>.rs` 写函数，标 `#[tauri::command]`，参数用 snake_case（前端传 camelCase，Tauri 自动转换）。
2. `main.rs` 的 `generate_handler![]` 里加一行。
3. `src/api.ts` 加类型化封装（`call<T>("命令名", { 参数 })`）——**前端只通过 api.ts 调命令**。
4. 需要提权就内部走 `elevation::run_elevated("task_name", payload, timeout)`，并在 `task_runner.rs` 的 `KNOWN_TASKS` 与 `execute()` 函数表里登记。

**加一个内置 DNS 预设**
在 `os/dns_presets.rs` 的 `P` 数组加一项（`id` 唯一、`region`、`v4`/`v6` 成对、可选 `doh`/`dot`）。
单测 `all_addresses_parse` / `preset_ids_unique` 会自动校验地址合法性与 id 唯一性。

**加一个设置项**
`config.rs::Settings` 加字段（带默认）→ `src/settings.ts` 加控件 → 需要立即生效就在 `settings_save` 里处理（如日志级别）→ 记到 README 的偏差表（如果与设计文档不同）。

**改快捷面板布局**
只动 `src/quick.ts`（渲染 + 绑定）与 `app.css` 的 `.flyout-* / .tile*` 段。保持"瓦片 + 内联确认条"的语言，
不要引入主窗口的 `.card` / `.tabs` / `.field-grid`。改完用 §5.4 探针确认 `tiles/chips` 数量，并截图。

**改 macOS 平台层**
1. 先确定改动属于哪一层：读/写系统状态 → `os/macos/*`；"要不要提权" → `proxy_ops.rs` / `flush_needs_root()`；任务形状 → `task_runner.rs` 的 `KNOWN_TASKS` + `execute()` 分支。
2. 外部命令一律 `Command::new(绝对路径).args([...])` 直传（服务名含空格，别拼 shell）；要 root 的写操作走提权任务。
3. 写完必须**回读校验**（`set` 里做完 `networksetup` 再 `read_dns` 对比，不一致报 `E4001` / `E4003`）。
4. 本地只能验语法（`rustfmt --check`）与 Windows 不回归；**真正的编译与验证在 CI 的 `macos-latest`**。
5. 真机验证按 §6.4 的实测清单走，改完把结论补回 §6.4 与 §5.1。

**接入真机提权测试**
把 `SuNet.exe` 复制到 `C:\Program Files\SuNet\`（避免从 `\\?\` 或用户目录启动），
用**标准用户**账户登录，点主窗口「三层总览 → hosts → 写入」，观察：UAC 出现 → 授权 → 日志 `提权任务 hosts_apply：退出码 0` → hosts 文件回读一致。
