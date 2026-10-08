# 速网 SuNet · 网络配置切换器 · 设计方案

> 版本 v1.5 · 2026-10-06
> 技术栈：Rust + Tauri 2 · 目标平台 Windows 10/11 x64
> 参考原型：SteamHostSync（Clov614/SteamHostSync）—— hosts 多方案 + 远程订阅 + 一键切换
> 能力范围：hosts 托管区块 + WinINET 系统代理开关 + 系统 DNS（IPv4/IPv6 全量清单）

---

## 0. 需求还原与范围界定

### 0.0 命名：速网 / SuNet

| 项 | 内容 |
|---|---|
| 中文名 | **速网** |
| 英文名 / 程序名 | **SuNet**（`SuNet.exe`） |
| 一句话释义 | 快速切换网络配置 |

**选名理由**：

- **「速」直指核心价值**——三层网络配置的一键原子切换，速度就是这个工具存在的理由
- **「网」覆盖完整语义边界**——hosts、代理、DNS 全部在「网」这个字里，不需要三个字才能说清
- 两字好记、口语化，叫得出来（「速网打开一下」「用速网切」），符合工具软件的实际使用场景
- 英文取 `Su + Net`，与 `subnet` 谐音联想，暗示「网络」，且无知名项目占用

**避开的名称（记录理由，避免后续返工）**：

| 曾考虑 | 放弃原因 |
|---|---|
| `NetBox` | 与 **Cisco NetBox**（开源网络自动化平台，运维圈广泛使用）正面撞名，GitHub 仓库名与域名不可用，搜索结果会被完全淹没 |
| `NetFlip` | 已被 Netflip（电影配对 App）占用，Google Play 有同名应用 |
| `净网 / NetKit` | 「净」隐含过滤/拦截语义，与本工具「切换」的中性定位不符；NetKit 亦有同名项目 |

**品牌资产清单**：

```
产品中文名      速网
程序名          SuNet.exe
安装目录        C:\Program Files\SuNet\
配置目录        %APPDATA%\SuNet\
计划任务名      SuNet
hosts 托管标记  # >>> SuNet BEGIN  …  # <<< SuNet END
```

> `hosts` 托管标记使用 `SuNet`。产品开发过程中曾用 `NetBox` 作为暂定名（因与 Cisco NetBox 撞名已废弃），若本地留有旧版区块，升级时需替换标记并清理旧区块——见 §2.4。

### 0.1 你描述的四件事

| # | 需求 | 实质是什么 |
|---|---|---|
| 1 | 快速修改 hosts | 静态 `域名 → IP` 映射层的增删改查 + 立即生效 |
| 2 | 快捷开关系统代理 | 传输路径改道（填地址+端口） |
| 3 | 快捷修改系统 DNS | 解析器改道（选 DNS 服务器） |
| 4 | 开机自启 + 托盘常驻 | 后台进程生命周期管理 |

### 0.2 关键澄清：这三件事不在同一层

这是整个设计的立足点，也是多数同类工具做不好的地方。Windows 的域名解析与连接路径是**三层独立机制**：

```
┌─ 应用层 ───────────────────────────────────────┐
│ 浏览器 / 微信 / Steam 客户端 / curl / git      │
└───────────────────┬───────────────────────────┘
                    │
        ┌───────────┴────────────┐
        │  ① Proxy（传输路径）    │  ← 需求 2
        │  应用是否支持系统代理？ │
        │  是 → 流量交给代理进程  │
        │  否 → 忽略，直接建连    │
        └───────────┬────────────┘
                    │ （直连时）
        ┌───────────┴────────────┐
        │  ② Hosts（静态映射）    │  ← 需求 1
        │  命中则直接返回 IP     │
        │  永不询问 DNS 服务器   │
        └───────────┬────────────┘
                    │ （未命中）
        ┌───────────┴────────────┐
        │  ③ DNS（解析器）        │  ← 需求 3
        │  向哪台 DNS 服务器问   │
        └────────────────────────┘
```

**三条结论，直接决定产品形态：**

1. **三者互不覆盖。** 只改 hosts 不影响代理；开代理不改 hosts 依然走代理；改 DNS 对已命中 hosts 的域名无效。把三者堆在一个界面里而没有"方案（Profile）"概念，用户每次都要点三次开关——这是伪需求满足。
2. **优先级固定且不可协商。** Hosts 永远赢过 DNS，DNS 永远赢过系统缓存。所以"改了 DNS 没反应"绝大多数情况不是 bug，是 hosts 里已有该域名。
3. **代理层是唯一会"全局断网"的层。** DNS 配错还能改回来，代理开着但代理软件没启动 = 所有应用瞬间失联。因此代理开关必须带**前置连通性自检**和**失败自动回滚**，这是硬性需求，不是加分项。

### 0.3 核心产品决策：引入"方案"（Profile）

把 hosts + 代理 + DNS 打包为一个具名方案，一键原子切换。这是本工具相对 SteamHostSync 的真正增量——SteamHostSync 只解决第①层。

| 层级 | 能力 |
|---|---|
| 基础 | hosts 编辑器（表格形式，不用手写文本） |
| 基础 | 代理开关（地址/端口/绕过列表） |
| 基础 | DNS 切换（网卡选择 + **IPv4 / IPv6 分族独立控制** + 约 40 项内置清单） |
| **方案** | **三者的原子组合，一键切换 / 一键还原** |
| 增强 | hosts 远程订阅（沿用 SteamHostSync 的 Source.yaml 思路） |
| 增强 | 开机按上次方案自动恢复 |

> DNS 是本版本扩展幅度最大的一块：从最初讨论的 5 个预设，扩到覆盖国内 12 家 + 国外 18 家的完整清单，每项都成对提供 IPv4 与 IPv6。详见 §4.3。

### 0.4 明确不做（本版本）

- ❌ **TUN / 虚拟网卡 / SOCKS5 强制接管**：需驱动与 WFP 过滤器，复杂度与风险高出两个数量级。你选的是 WinINET 全局代理，本版本严格限定在此层。
- ❌ **WinHTTP 系统代理**（`netsh winhttp`）：机器级、影响 Windows Update 与部分系统服务，副作用面大。列入 v2 可选开关，默认关闭。
- ❌ **PAC 代理模式**：写入 `AutoConfigURL` 会覆盖手动代理开关，属于互斥模式，本版本不共存。
- ❌ **防火墙规则管理**：独立话题，另开。
- ❌ **自动更新**：v1 无签名分发渠道，只提供「检查更新」跳转 Releases 页（§15.3）。自带下载器会引入"更新包被替换"的风险，而这个风险需要代码签名才压得住。
- ❌ **hosts 通配符 / 正则规则**（SwitchHosts 支持）：通配符会让"最终哪条生效"变得不可预测，与 §0.2「优先级必须可解释」的前提直接冲突。本工具只做**精确映射**。

---

## 1. 技术选型与依赖清单

### 1.1 已确认的选型

| 项 | 选择 | 理由 |
|---|---|---|
| 框架 | Tauri 2 | 安装包 3–8 MB，内存 40–90 MB，托盘/自启/权限都是官方一等公民 |
| 语言 | Rust | 写文件、注册表、Win32 FFI 都是强类型无 GC，且避免 Electron 的杀软误报 |
| 前端 | 原生 TS + Vite（不用 UI 框架） | 界面只有 3 个标签页 + 托盘菜单，引入 React/Vue 反而增体积 |
| 权限模型 | 主进程 `asInvoker` + 按需提权助手 | 仅 hosts/DNS 需提权，代理开关完全静默。详见 §1.4 |
| 代理层 | WinINET（注册表 + `InternetSetOptionW`） | 覆盖浏览器/Electron/国产办公软件，无需网卡驱动；且 HKCU 本就无需提权 |
| 提权方式 | `ShellExecuteW` + `runas` 命令行分流 | 避免 `CreateProcess` 的 740 错误；提权子进程不加载 WebView2 |

### 1.2 Cargo 依赖

```toml
[dependencies]
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-single-instance = "2"  # 多实例防护，见 §5.3
tauri-plugin-global-shortcut = "2"  # 全局热键，见 §5.5
tauri-plugin-dialog  = "2"
serde  = { version = "1", features = ["derive"] }
serde_json = "1"
uuid   = { version = "1", features = ["v4"] }
anyhow = "1"
thiserror = "2"
chrono = "0.4"
regex  = "1"
encoding_rs = "0.8"                  # hosts 文件编码兜底，见 §2.3
dirs   = "6"
log   = "0.4"

[target.'cfg(windows)'.dependencies]
windows = { version = "0.6", features = [
  "Win32_Foundation",
  "Win32_System_Registry",
  "Win32_Networking_WinInet",
  "Win32_NetworkManagement_IpHelper",
  "Win32_System_Console",
  "Win32_System_Process",
  "Win32_System_Threading",
] }

[build-dependencies]
tauri-build = { version = "2", features = [] }
```

**版本说明**：`tauri-plugin-global-shortcut` 当前为 2.3.2，要求 Rust ≥ 1.77.2；它是 Tauri 1.x `global_shortcut_manager` 的正式替代品。`windows` crate 用 0.6 系；如果你要碰 `netioapi.h`，需 0.61/0.62 分支（见 §3.4）。

### 1.3 目录结构

```
sunet/
├─ package.json
├─ vite.config.ts
├─ index.html
├─ src/                          # 前端
│  ├─ main.ts
│  ├─ tabs/
│  │  ├─ hosts.ts                # hosts 表格编辑器
│  │  ├─ proxy.ts                # 代理开关表单
│  │  ├─ dns.ts                  # DNS 切换
│  │  └─ profiles.ts             # 方案管理
│  ├─ tray.ts                    # 前端侧托盘（备用，Rust 侧为主）
│  ├─ theme.ts                   # 深浅色三态 + prefers-color-scheme 监听（§9.1）
│  └─ tokens.css                 # 设计令牌：颜色 / 间距 / 字体（§9.1）
└─ src-tauri/
   ├─ build.rs                   # 不注入 requireAdministrator —— 主进程用 asInvoker
   ├─ tauri.conf.json
   ├─ capabilities/default.json
   ├─ icons/
   │  └─ tray.png                # 32×32 透明底
   └─ src/
      ├─ main.rs                 # 入口：先拦 --task 参数分流，再进 Tauri
      ├─ task_runner.rs          # 提权子进程的任务分流（不进 Tauri setup）
      ├─ elevation.rs            # ShellExecuteW + runas 提权调用
      ├─ tray.rs                 # 托盘构建与事件
      ├─ state.rs                # 全局状态 + 互斥锁
      ├─ config.rs               # 配置/方案持久化
      ├─ logging.rs              # 日志初始化与脱敏（§14.1）
      ├─ apply.rs                # 事务式切换引擎（核心）
      ├─ backup.rs               # 快照与回滚
      ├─ probe.rs                # 连通性自检
      ├─ ipc.rs                  # 载荷/结果文件交换 + 命名互斥体（§1.4.10 §7.2）
      ├─ subscribe.rs            # 远程订阅拉取 + 内容校验（§2.6 §10.1）
      ├─ notify.rs               # 托盘气泡与降级链路（§5.4.4 §5.4.5）
      ├─ migrate.rs              # 配置 schema 迁移（§15.2）
      ├─ error.rs                # 错误码枚举与用户文案（§14.3）
      ├─ cmd/
      │  ├─ mod.rs
      │  ├─ hosts.rs             # tauri commands
      │  ├─ proxy.rs
      │  ├─ dns.rs
      │  ├─ profile.rs
      │  └─ system.rs
      └─ os/
         ├─ mod.rs
         ├─ hosts_file.rs        # 读写 + 原子替换 + 编码处理
         ├─ wininet.rs           # 注册表 + InternetSetOptionW（无需提权）
         ├─ dns_client.rs        # 网卡枚举 + 设 DNS + 清缓存
         └─ privilege.rs         # 仅探测当前是否提权（无 SID 比对逻辑）
```

### 1.4 权限模型：逐项拆解与最终选择

#### 1.4.1 先把权限需求逐项拆开

前一版方案直接接受了"整程序管理员运行"，这是**偷懒的答案**——把三个功能打包成同一个权限需求，结果为一个功能付了全部代价。正确的做法是逐项核实：

| 操作 | 实际写入位置 | 是否需要管理员 | 依据 |
|---|---|---|---|
| 读网卡列表 / 读当前 DNS | `Get-NetAdapter`、`Get-DnsClientServerAddress` | ❌ 不需要 | 只读 cmdlet |
| 改 hosts 文件 | `C:\Windows\System32\drivers\etc\hosts` | ✅ **需要** | 文件 ACL：Administrators `(F)`，Users `(RX)`；KB 923947 |
| 改系统 DNS（v4/v6） | 网卡配置（`Tcpip\Parameters`） | ✅ **需要** | 非提权执行返回 `0x80070005`（Access Denied） |
| 清 DNS 缓存 | `Clear-DnsClientCache` | ❌ 不需要 | 无需提权 |
| **读写系统代理** | `HKCU\...\Internet Settings` | ❌ **不需要** | **HKCU 是用户自己的 hive，用户进程默认可写** |
| 通知代理变更 | `InternetSetOptionW`（用户态 API） | ❌ 不需要 | 纯 WinINet 调用 |
| 托盘 / 窗口 / 配置读写 | `%APPDATA%\SuNet` | ❌ 不需要 | 用户目录 |
| 开机自启注册 | `HKCU\...\Run` | ❌ 不需要 | 用户 hive |

**关键事实：WinINET 代理设置在 `HKEY_CURRENT_USER`，用户进程默认可写，不需要管理员权限。** 这是前一版方案没有讲清的一点——代理功能被"连坐"到了管理员权限上，而它本身完全不需要。

所以真实的权限需求是：**hosts + DNS 需要提权，代理 + 界面 + 自启都不需要。** 三项里有两项（且包括使用频率最高的代理开关）根本不需要提权。

#### 1.4.2 四种模型对比

| 模型 | 代理开关 | hosts / DNS | 开机自启 | 标准用户可用 | 复杂度 |
|---|---|---|---|---|---|
| **A. 整程序提权**（前一版） | 弹 1 次 UAC | 弹 1 次 UAC | **每次开机弹 UAC** | ❌ HKCU 归属错乱 | 低 |
| **B. 常驻提权服务 + 本地 IPC** | 静默 | 静默 | 静默 | ✅ 可用 | 高（需装服务） |
| **C. 主进程普通 + 按需提权助手** | **静默** | 应用时弹 1 次 UAC | 静默 | ✅ 可用 | 中 |
| **D. 改造系统 ACL 永久放权** | 静默 | **静默** | 静默 | ❌ 有安全代价 | 中，但见下 |

#### 1.4.3 为什么选 C（按需提权助手）

**先排除 D，因为它看起来最诱人但实际上最危险。** `icacls hosts /grant Users:F` 确实能让 hosts 永久可写（SwitchHosts 等工具就这么干），DNS 也可以类似放开。但代价是：

- **把一个系统级配置文件永久开放给所有本地用户**，任何以你身份运行的程序都能改 hosts 做 DNS 劫持
- 杀软与部分企业策略会把 hosts 设回受保护状态，届时功能静默失效
- Windows 大版本升级或 SFC 修复后权限可能被重置
- **这是用安全性换一个不该省的 UAC 弹窗。** 本工具是日常高频使用的网络切换器，不应该为了少一次点击就永久打开系统文件的写权限。

**再看 B。** 常驻提权服务能实现全部静默，但要额外承担：服务安装/卸载逻辑、服务与桌面会话之间的通信（Session 0 隔离问题，见 1.4.6）、服务崩溃后自恢复、卸载时忘记清理服务。对一个"个人用网络工具"来说，这个复杂度不成比例——**服务常驻还意味着提权进程全天在线，攻击面反而更大。**

**选 C 的理由**：它精确匹配了真实权限需求。使用频率最高的代理开关完全静默（不需要权限），只有 hosts/DNS 这类低频重操作才在**用户主动触发时**弹一次 UAC。既不为不需要的权限付代价，也不引入常驻提权进程。

#### 1.4.4 C 模型的具体形态

```
┌────────────────────────────────────────────────────┐
│  SuNet.exe（主进程 · asInvoker · 普通权限）          │
│  托盘 · 窗口 · 配置 · 代理开关 · 状态读取            │
│         │                                          │
│         │ 需要改 hosts / DNS 时                     │
│         ▼                                          │
│    ShellExecuteW("runas", "SuNet.exe --task xxx")   │
│         │  ← 此时弹一次 UAC                         │
│         ▼                                          │
│  SuNet.exe --task（短命提权进程 · 干完就退）        │
│  · 按 argv 分派到具体任务                           │
│  · 改完 hosts / DNS / 还原后立即退出                 │
│  · 不加载 WebView2、不创建托盘、不常驻              │
└────────────────────────────────────────────────────┘
```

**三个必须做对的细节：**

**① 提权子进程必须用 `argv` 分派，绝不能"再启动一个自己的实例"。**
子进程不再走 Tauri 的 `setup`，不创建 WebView2 窗口，不注册托盘。理由很实在：**提权进程加载 WebView2 意味着把浏览器渲染引擎的攻击面提升到管理员权限**。这也顺带解决了启动速度——提权子进程是纯 Rust 二进制，毫秒级完成。

**② 用 `ShellExecuteW` + `runas`，不要用 `CreateProcess`。**
`CreateProcess` 不会自动触发 UAC，会直接返回 `ERROR_ELEVATION_REQUIRED`（740）。这是社区反复踩过的坑。`runas` verb 才是正确的提权入口。

**③ UAC 提示前先自绘说明对话框。**
Windows 的 UAC 框只显示程序名 `SuNet.exe`，用户可能不知道这弹窗是自己刚才点"应用 DNS"触发的。做法：主进程在触发提权前，先弹一个自绘对话框（"即将修改系统 DNS，需要管理员权限确认"），再发起 `runas`。这样 UAC 出现在用户有心理预期的时刻。

#### 1.4.5 C 模型下 UAC 只在低频操作出现

用户实际感知到的提权次数：

| 场景 | UAC 次数 |
|---|---|
| 开机自启、启动托盘 | **0** |
| 开关系统代理（每次） | **0** ← 最高频操作，完全不涉及 |
| 切换方案（若方案不含 hosts/DNS 变更） | **0** |
| 修改 hosts / DNS | 1（用户主动点击时） |
| 启用「开机自动恢复上次方案」（且方案含 hosts/DNS） | 1（开机后一次） |

对比前一版"整程序提权"：开机 1 次 + 每次启动程序 1 次 + 每次代理开关 1 次。**C 模型把提权次数从"每次操作"降到"只有真正需要的那一次"。**

#### 1.4.6 标准用户场景（本模型的关键优势）

前一版最大的缺陷在这里暴露：**整程序提权 + 标准用户账户 = HKCU 归属错乱**。用户输入另一个管理员的密码后，代理设置写进了那个管理员的 hive，自己的浏览器毫无反应，且很难自查。

C 模型下这个问题**根本不存在**：

- 代理功能在主进程执行，主进程始终以交互用户身份运行 → `HKCU` 天然正确
- 提权子进程只碰 hosts 和 DNS，**不碰 HKCU**（`InternetSetOptionW` 也在主进程调用）

所以前一版中原本需要的那套 SID 自检逻辑（`GetTokenInformation` 比对 explorer.exe token）**可以整段删除**。这不是优化，是消灭一整类 bug。

#### 1.4.7 卸载与残留

C 模型的提权是**按需、短命**的，不存在"计划任务残留一个提权进程"的问题。但仍需处理：

- 卸载时若用户选择了还原 hosts/DNS，需要一次提权——正常弹 UAC
- 不需要像 B 模型那样清理服务
- `schtasks` 计划任务仍由安装器创建（用于开机静默自启），卸载时 `schtasks /Delete /TN "SuNet" /F`

#### 1.4.8 保留的服务端升级路径

若日后确实需要"hosts/DNS 也不弹 UAC"，正确的升级路线是**先验证本文档的模式 C 是否已足够**，而不是直接跳到服务。模式 C 的实现中，`os/` 层所有函数都不持有 UI 状态、不依赖 WebView2 上下文，因此把它们整体搬进一个提权服务只需替换调用方，事务/快照/回滚逻辑完全复用。

#### 1.4.9 实现要点汇总

```rust
// 权限探测：主进程启动时判断自身状态，用于界面显示与降级
pub struct PrivilegeState {
    pub is_elevated: bool,
    pub can_write_proxy: bool,   // 恒为 true —— HKCU 用户可写
    pub can_write_hosts: bool,   // 取决于 is_elevated
    pub can_write_dns: bool,     // 取决于 is_elevated
}

// 提权调用：仅在 hosts / DNS 写入前触发
fn run_elevated_task(task: &str, payload: &Payload) -> Result<TaskOutput> {
    // 完整协议见 §1.4.10：载荷走文件（不经命令行，避免超长静默截断），
    // 用 ShellExecuteExW + SEE_MASK_NOCLOSEPROCESS 拿进程句柄以等待并取退出码。
    // 必须用 runas verb；CreateProcess 不触发 UAC，会直接返回 740 错误。
    let (in_path, out_path) = ipc::prepare(task, payload)?;
    let hproc = shell_execute_ex_runas(&format!(
        "--task {task} --in \"{}\" --out \"{}\"",
        in_path.display(), out_path.display()
    ))?;                                   // FALSE + ERROR_CANCELLED(1223) → E1001
    ipc::wait_and_collect(hproc, &out_path, Duration::from_secs(30))
}

// 子进程入口：main.rs 最前面拦截
fn main() {
    if let Some(task) = parse_task_arg() {
        std::process::exit(run_task_and_exit(task));  // 不进 Tauri setup，不加载 WebView2
    }
    run_gui();  // 正常 Tauri 应用路径
}
```

**未安装为服务、无常驻提权进程、提权子进程不加载 WebView2** —— 这三条是本模型的核心安全边界，任何实现改动不得破坏。

#### 1.4.10 提权子进程的通信协议（必须明确定义）

`ShellExecuteW` 只能告诉父进程"提权请求已发起"，**它不返回子进程的 stdout，也不把输出管道化**。如果不专门设计通信通道，提权子进程干完活后父进程拿不到任何结果，只能盲猜——这是模型 C 最容易留坑的地方。本节把它定为硬性规范。

**两条通道，各司其职：**

| 通道 | 内容 | 用途 |
|---|---|---|
| 进程退出码 | `0` 成功；非 0 为粗粒度失败分类 | 父进程判断是否继续、走哪条错误提示 |
| 结果文件（JSON） | 完整 `TaskOutput`：每步的 applied / verified / message | 界面如实展示（对应 §8 的 `ApplyReport`） |

退出码约定（与 §14.3 错误码表对应）：

```
0     成功
2     载荷无效（E1004）
3     提权后仍被拒（E1002，如组策略 / ACL）
4     目标文件被占用（E2001，多为杀软）
5     回读校验失败（E2004 / E4003，写入被策略刷回）
6     内部错误
UAC 被用户取消时不返回退出码 —— ShellExecuteEx 返回 FALSE，
     GetLastError() == ERROR_CANCELLED(1223)，映射为 E1001
```

**为什么不用命令行内联传参：** `ShellExecuteEx` 的 `lpParameters` 有长度上限，hosts 条目一多序列化出的 JSON 很容易超限；**超长会被静默截断**，子进程拿到半个 JSON 后行为不可预期。改为文件传参：

```
父进程：
  1. 在 %LOCALAPPDATA%\SuNet\ipc\ 下建 task-<uuid>.in.json（写入载荷）
  2. ShellExecuteExW(runas, "SuNet.exe --task <task> --in <绝对路径> --out <绝对路径>")
     带 SEE_MASK_NOCLOSEPROCESS → 拿到 hProcess 句柄
     └─ 返回 FALSE 且 GetLastError()==1223 → 用户取消了 UAC，报 E1001，清理载荷文件后返回
  3. WaitForSingleObject(hProcess, 30_000)
     ├─ WAIT_TIMEOUT → TerminateProcess + 报「提权任务超时」（E1003）
     └─ WAIT_OBJECT_0 → GetExitCodeProcess
  4. 读 --out 文件（存在则反序列化为 TaskOutput；不存在则按退出码生成兜底报告）
  5. CloseHandle(hProcess) + 删除两个临时文件
```

**路径必须由父进程显式构造并传入，不能让子进程自己拼 `%TEMP%`。** 原因：标准用户场景下（§1.4.6），提权子进程的身份是另一个管理员账户，环境变量与用户目录可能不一致；显式绝对路径消除这个不确定性。`%LOCALAPPDATA%` 下目录的 ACL 天然包含 `Administrators`（继承自用户配置文件夹），两侧都能读写。

**安全约束（不可省略）：**

- 文件名含随机 `uuid`，防止可预测路径被抢占做符号链接攻击
- 子进程写入前**先校验载荷文件的属主与 ACL**（必须属于发起用户、且无 `Everyone` 写权限），不通过则拒绝执行
- 子进程读完输入立即删除；父进程读完输出立即删除
- **同一时刻只允许一个提权任务在途**——由跨进程命名互斥体保证（§7.2），否则两次 hosts 写入会互相踩踏
- 载荷是**纯数据**（JSON）。子进程只按 `task` 名分派到一张固定的函数表，不做任何动态派发、不执行表达式

> 通信**不用命名管道**：提权与非提权进程之间的管道需要在创建时显式设置 DACL（父进程建服务端要放 `Administrators`，子进程建服务端要放发起用户），任一侧写错都表现为一个"连接超时"式的模糊故障，排查成本远高于文件交换。文件交换多两次磁盘 I/O —— 对低频的 hosts/DNS 操作完全可接受。

---

## 2. 模块一：Hosts 管理

### 2.1 系统事实（已核实）

| 项 | 值 | 来源/说明 |
|---|---|---|
| 路径 | `C:\Windows\System32\drivers\etc\hosts` | 无扩展名 |
| 写权限 | 需管理员。即便账户有管理员凭据，非提权进程保存会被拒 | Microsoft KB 923947 |
| 编码 | 必须是 ANSI 或 **UTF-8 无 BOM** | 带 BOM 时首行前 3 字节污染，解析失败 |
| 换行 | CRLF | LF 在部分版本上解析异常 |
| 生效 | 需刷新 DNS 客户端缓存 | `ipconfig /flushdns` 或 `DnsFlushResolverCache()` |
| 首命中规则 | 同一域名出现多次时，**第一条**生效 | 追加式写入是常见错误来源 |

### 2.2 文件操作策略：绝不整文件覆盖

**错误做法**：读取 → 拼接 → `fs::write()` 整体覆盖。会丢权限、丢属性、丢用户自己写的注释、中途崩溃留下半截文件。

**本设计做法**：

```
写入流程：
  1. 备份      → hosts.bak.<timestamp>  （与 hosts 同目录，防杀软跨卷扫描漏检）
  2. 解码原文件 → 按 §2.3 规则
  3. 结构化编辑 → 只操作本工具托管的区块，见 §2.4
  4. 写临时文件 → 同目录 hosts.tmp（必须同卷，保证 ReplaceFile 原子性）
  5. 原子替换   → ReplaceFileW(tmp, hosts, backup, REPLACEFILE_IGNORE_MERGE_ERRORS, ..)
                  ↑ ReplaceFileW 保留目标文件原有的 ACL 与属性，这是相对
                    "删除再新建"的关键优势
  6. 失败回退   → 若 5 因杀软占用失败：
                  SetFileAttributes(FILE_ATTRIBUTE_NORMAL) → WriteFile 截断写入
  7. 刷新缓存   → DnsFlushResolverCache()（dnsapi.dll，不弹黑窗，比 spawn
                  ipconfig 快一个数量级）
```

### 2.3 编码处理规则（保守优先）

```
读入 bytes:
  ├─ 有 UTF-8 BOM (EF BB BF)        → 剥离 BOM，按 UTF-8 解码
  ├─ 有 UTF-16LE BOM (FF FE)        → 按 UTF-16LE 解码 → 转 UTF-8
  ├─ UTF-8 解码成功                  → 直接用
  ├─ 按 GBK/ANSI 解码成功            → 用（中文注释场景常见）
  └─ 全部失败                        → 拒绝写入，界面报错
                                       「无法识别文件编码，为避免破坏已拒绝修改」
```

写出一律 **UTF-8 无 BOM + CRLF**。若原文件是 GBK 且含中文注释，转为 UTF-8 后注释内容不变（Windows 10/11 的 hosts 解析器接受 UTF-8 无 BOM）。

最后那条"解码失败就拒绝写入"是刻意的：宁可让功能不可用，也不能把用户 hosts 搅乱。

### 2.4 托管区块标记（借鉴 SwitchHosts 的成熟做法）

不要往 hosts 里无脑追加。每次写入只替换自己的标记区块，其余部分原样保留：

```
# >>> SuNet BEGIN (managed by SuNet, do not edit this block)
140.82.114.26    alive.github.com
140.82.113.26    live.github.com
185.199.108.153  github.githubassets.com
# <<< SuNet END
```

**好处**：用户在区块外手写的本地域名映射永不丢失；反复切换不会让 hosts 膨胀成几千行；关闭方案 = 删除整个区块，而不是注释掉。

### 2.5 Hosts 条目数据模型

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HostsEntry {
    pub id: String,          // uuid
    pub ip: String,          // 校验为合法 IPv4/IPv6
    pub hostname: String,    // 校验为合法域名，支持多域名（空格分隔）
    pub enabled: bool,       // 关闭 → 写入为注释行，保留记忆
    pub comment: String,     // 可选，写为行尾注释
}
```

**校验规则**（拒绝即在输入框标红，不静默丢弃）：

- IPv4：四段 0–255；IPv6：`::`/压缩形式
- 域名：每段 `a-zA-Z0-9-`，不以 `-` 开头/结尾，总长 ≤ 253
- **批量屏蔽**（把域名指向 `0.0.0.0`）拆成显式开关，默认关闭；开启前逐条列出将被屏蔽的域名，避免顺手写出几百条屏蔽后自己也想不起来
- 重复 hostname：在编辑器内标黄提示"存在重复条目，首条生效"

### 2.6 远程订阅（沿用 SteamHostSync 思路）

```yaml
# sources.yaml
sources:
  - name: "GitHub 加速"
    url: "https://cdn.jsdelivr.net/gh/Clov614/SteamHostSync@main/Hosts_github"
    mirror: "https://cdn.statically.io/gh/Clov614/SteamHostSync@main/Hosts_github"
    interval_hours: 24
    enabled: true
```

**约束（这些是踩过坑的点）**：

- **必须支持镜像源**：主源挂了就切镜像，这是远程订阅唯一真正的价值
- **拉取失败绝不能清空本地**：拉取失败时保留上一次订阅内容，界面标"同步失败（3 小时前）"
- **订阅内容与手工条目分离存储**：订阅拉下来的条目进 `source_entries`，手工条目进 `entries`，渲染时合并。手工条目永远优先（写在托管区块顶部）
- 拉取走 Rust `reqwest`（阻塞版，放 `spawn_blocking`），不走前端 fetch——避免 CORS 与权限问题

---

## 3. 模块二：系统代理（WinINET）

### 3.1 系统事实（已核实）

代理开关全部落在**一个注册表键**下：

```
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Internet Settings
```

| 值名 | 类型 | 含义 |
|---|---|---|
| `ProxyEnable` | `REG_DWORD` | `1` 启用 / `0` 关闭 |
| `ProxyServer` | `REG_SZ` | 形如 `127.0.0.1:7890` 或 `http=127.0.0.1:7890;https=127.0.0.1:7890` |
| `ProxyOverride` | `REG_SZ` | 绕过列表，默认 `localhost;127.*;<local>` |
| `AutoConfigURL` | `REG_SZ` | PAC 地址。**存在时优先级高于手动代理** |

值名与语义出自 Microsoft WinINet 文档与注册表规范；`AutoConfigURL` 覆盖手动代理这一行为在多份实测记录中一致。

### 3.2 关键：写完注册表必须通知系统

**只写注册表是最常见的错误。** Chrome / Edge 等基于 Chromium 的应用会继续沿用启动时缓存的代理配置，直到重启进程。必须显式通知：

```rust
unsafe fn notify_proxy_changed() -> Result<()> {
    use windows::Win32::Networking::WinInet::*;
    // 39 = INTERNET_OPTION_SETTINGS_CHANGED
    // 通知系统注册表已变，下次 InternetConnect 时重读
    InternetSetOptionW(
        std::ptr::null_mut(),
        INTERNET_OPTION_SETTINGS_CHANGED,
        std::ptr::null_mut(),
        0,
    ).ok()?;
    // 37 = INTERNET_OPTION_REFRESH
    // 对已有 handle 强制重新读取代理数据
    InternetSetOptionW(
        std::ptr::null_mut(),
        INTERNET_OPTION_REFRESH,
        std::ptr::null_mut(),
        0,
    ).ok()?;
    Ok(())
}
```

常量值 39 / 37 来自 `wininet.h` 的 `SETTINGS_CHANGED` 与 `REFRESH`，MSDN《Option Flags (Wininet.h)》与社区实测一致。

**推荐更彻底的写法**：`INTERNET_OPTION_PER_CONNECTION_OPTION`（值 75）+ `INTERNET_PER_CONN_OPTION_LIST` 结构，一次设置 `PROXY_TYPE_DIRECT|PROXY` + server + bypass。这是 IE 时代"per-connection 设置"的正统 API，能处理多网卡分场景代理。v1 先用注册表 + 通知两条，v2 换 API，两者对外接口不变。

**失败处理**：`InternetSetOptionW` 返回 `BOOL`，**必须检查**。若通知失败而注册表已写成功，代理功能上已生效、只是即时性打折——此时**不要中断事务**（写回原值反而更糟），只记一条 warning 到日志。

### 3.3 "快照—还原"而非"开关"

**不要把关闭实现成 `ProxyEnable = 0`。** 这样会静默摧毁用户原有的 PAC 配置、企业代理配置、局域网绕过规则。

```
启用代理前：
  snapshot = 读(ProxyEnable, ProxyServer, ProxyOverride, AutoConfigURL)
  → 存入 %APPDATA%\SuNet\proxy_snapshot.json

关闭代理：
  若 snapshot 存在 → 完整写回（含 AutoConfigURL），删除 snapshot
  若不存在       → 视为"接管前系统本无代理"，写回默认值
```

快照文件是崩溃恢复的依据，见 §6.3。

### 3.4 前置连通性自检（硬性需求）

**没有这一条，这个功能会造成全网中断。** 代理开着但代理软件没运行 = 所有遵循系统代理的应用瞬间失联，且用户往往反应不过来是你干的。

```
用户点"开启代理"：
  1. 校验地址格式（IP 或域名；端口 1–65535）
  2. TCP 探测 proxy_host:proxy_port，超时 800ms
     ├─ 连通 → 继续
     └─ 不通 → 【拒绝开启】，弹窗：
             「无法连接到 127.0.0.1:7890，开启后所有应用将无法联网。
               请先启动代理软件，或检查端口。」
             提供两个出口：取消 / 仍然开启（高级用户，明确知情）
  3. 写 ProxyServer + ProxyEnable=1 + 通知
  4. （可选，建议做）TCP 探测 1.1.1.1:443 / www.baidu.com:443，
     5 秒内不通 → 提示"代理已开启但外网不通"，询问是否回滚
```

第 4 步是"开了但不通"和"根本不该开"的区分，能省掉大量用户求助。

### 3.5 生效范围与盲区（必须明示）

| 应用 | 是否跟随 WinINET |
|---|---|
| Chrome / Edge / Firefox（Windows 版） | ✅ |
| IE / Edge 传统模式 | ✅ |
| Electron 应用（VS Code / Discord / 新版 QQ 等） | ✅（读系统代理） |
| 大部分国产办公软件、微信 PC 版 | ✅ |
| Windows Update / 部分系统服务 | ❌ 走 WinHTTP，需 `netsh winhttp` |
| UWP / 微软商店应用 | ⚠️ 部分版本走自己的 WinHTTP 配置 |
| 部分游戏客户端 | ❌ 多数自带加速模块 |
| `curl` / `git` / `npm` | ❌ 默认不读系统代理（Git 可配 `http.proxy`） |

**这张表必须做进界面**，否则用户开了代理发现 `git clone` 没走，会认为工具坏了。工具只承诺它承诺的。

---

## 4. 模块三：系统 DNS

### 4.1 三条实现路径的取舍

| 方案 | 调用 | 优点 | 缺点 |
|---|---|---|---|
| A. `netsh interface ip set dns` | spawn 子进程 | 最直观、资料多 | 编码依赖 locale、约 150–400ms 进程开销、**v4/v6 是两套独立命令**（`netsh interface ipv4` 与 `ipv6`） |
| B. `Set-DnsClientServerAddress` | spawn `powershell -NoProfile -NonInteractive` | 语义清晰、显式 `-AddressFamily` 分族、还原用 `-ResetServerAddresses` | 每次起一个 PowerShell，约 300–600ms |
| C. `SetInterfaceDnsSettings` | `netioapi.dll` FFI | 原生、毫秒级、无子进程 | `windows` crate 需 0.61+ 分支；结构体字段需照 SDK 头文件核对 |

**本设计：v1 用 B，v2 切 C。**

理由：DNS 修改是用户显式触发的低频操作（不像代理开关会随手点），300ms 完全可接受。B 的 `-AddressFamily` 参数天然对应 §4.3 要求的 v4/v6 独立控制，`-ResetServerAddresses` 还原 DHCP 的行为一目了然。**不推荐 v1 就上 C**——`DNS_INTERFACE_SETTINGS` 结构体布局和 `DevolutionLevel` 语义需要对照本机 SDK 头文件逐一验证，`windows` crate 的低版本分支未必暴露该 API，贸然上手会把 v1 拖进无谓的调试深渊。

C 的对接点已经在代码里预留：`os/dns_client.rs` 的对外接口（`list_interfaces` / `set_dns` / `reset_dns`）在 A/B/C 三种实现下完全一致，替换只动内部。

### 4.2 多网卡处理（这是 DNS 最容易出错的地方）

```powershell
# 枚举：只列物理网卡且链路已连接
Get-NetAdapter -Physical | Where-Object Status -eq 'Up'

# 读取当前 DNS（分族读，便于界面分别展示）
Get-DnsClientServerAddress -InterfaceAlias "WLAN" -AddressFamily IPv4
Get-DnsClientServerAddress -InterfaceAlias "WLAN" -AddressFamily IPv6

# 设置 IPv4（一次可传主备两个）
Set-DnsClientServerAddress -InterfaceAlias "WLAN" -AddressFamily IPv4 `
  -ServerAddresses ("223.5.5.5","223.6.6.6")

# 设置 IPv6 —— 注意：必须显式带 -AddressFamily IPv6
# 不带时 PowerShell 会把 IPv6 地址也塞进 IPv4 那次调用，导致行为不符预期
Set-DnsClientServerAddress -InterfaceAlias "WLAN" -AddressFamily IPv6 `
  -ServerAddresses ("2400:3200::1","2400:3200:baba::1")

# 单独还原某一族为 DHCP 自动（v4 保留、v6 还原的场景必须用这个）
Set-DnsClientServerAddress -InterfaceAlias "WLAN" -AddressFamily IPv6 `
  -ResetServerAddresses

# 全部还原
Set-DnsClientServerAddress -InterfaceAlias "WLAN" -ResetServerAddresses

# 清缓存（等价于 ipconfig /flushdns）
Clear-DnsClientCache
```

**分族写入的必要性**：`Set-DnsClientServerAddress` 的 `-ServerAddresses` 同时接受 v4 与 v6 地址并按族分配，看着可以一条命令搞定。但要实现 §4.3 要求的「IPv6 单独还原、IPv4 保留」时，一条命令做不到——必须显式按 `-AddressFamily` 分两次调用。这是 v1 选方案 B 时必须接受的额外复杂度，也是为什么 `os/dns_client.rs` 的接口要把 `address_family` 作为一等参数：

```rust
pub enum AddressFamily { V4, V6 }

pub fn set_dns(alias: &str, family: AddressFamily, servers: &[IpAddr]) -> Result<()>;
pub fn reset_dns(alias: &str, family: AddressFamily) -> Result<()>;
```

**必须处理的三个坑：**

1. **虚拟网卡要排除。** VMware / VirtualBox / Hyper-V / WSL / Clash TUN / 各类 VPN 虚拟网卡都在列表里。默认只对 `Get-NetAdapter -Physical` 中 `Status = Up` 的网卡操作，且允许用户手动改。
2. **家庭路由器场景下改本机 DNS 意义有限。** 路由器分配的运营商 DNS 往往有内网 CDN 调度优势，且改路由器 DNS 对全家设备生效。**这是过度设计的高发地**——界面只提示，不代替用户决策。
3. **Enterprise 组策略可能锁定。** 域环境下 GPO 可以禁止修改 DNS，改了也可能被策略刷回。检测到写后回读不一致时，明确报"设置未生效，可能被组策略限制"，而不是假装成功。

### 4.3 内置 DNS 服务清单（IPv4 + IPv6 全覆盖）

内置清单分三组：**系统项**（还原用）、**国内**、**国外**。每一项都成对提供 IPv4 与 IPv6 地址，用户选择时按需写入。

#### A. 系统项

| 名称 | IPv4 | IPv6 | 说明 |
|---|---|---|---|
| 系统默认（DHCP） | — | — | 还原项，清空手动设置回到自动获取 |

#### B. 国内服务商

| 名称 | IPv4（主 / 备） | IPv6（主 / 备） | 特点 |
|---|---|---|---|
| 阿里 AliDNS | `223.5.5.5` / `223.6.6.6` | `2400:3200::1` / `2400:3200:baba::1` | 国内覆盖最广，支持 DoT/DoH，ECS 调度 |
| 腾讯 DNSPod | `119.29.29.29` / `182.254.116.116` | `2402:4e00::` | 腾讯系产品与游戏站点优化，防域名污染 |
| 百度 DNS | `180.76.76.76` | `2400:da00::6666` | 百度系站点优化 |
| 114DNS（普通） | `114.114.114.114` / `114.114.115.115` | — | 老设备兼容性最好，不支持加密 |
| 114DNS（安全） | `114.114.114.119` / `114.114.115.119` | — | 拦截钓鱼病毒木马 |
| 114DNS（家庭） | `114.114.114.110` / `114.114.115.110` | — | 拦截成人内容 |
| 360 安全 DNS（电信/移动） | `101.226.4.6` / `218.30.118.6` | — | 按运营商分流，需选对应线路 |
| 360 安全 DNS（联通） | `123.125.81.6` / `140.207.198.6` | — | 同上 |
| CNNIC SDNS | `1.2.4.8` / `210.2.4.8` | `2001:dc7:1000::1` | 国家互联网信息中心，无广告 |
| OneDNS（拦截版） | `117.50.11.11` / `52.80.66.66` | `2400:7fc0:849e:200::4` / `2404:c2c0:85d8:901::4` | 微步在线，过滤广告与恶意站点 |
| OneDNS（纯净版） | `117.50.10.10` / `52.80.52.52` | `2400:7fc0:849e:200::8` / `2404:c2c0:85d8:901::8` | 不过滤，直接返回真实结果 |
| 字节跳动公共 DNS | `180.184.1.1` / `180.184.2.2` | — | 抖音/头条系站点优化 |

#### C. 国外服务商

| 名称 | IPv4（主 / 备） | IPv6（主 / 备） | 特点 |
|---|---|---|---|
| Cloudflare | `1.1.1.1` / `1.0.0.1` | `2606:4700:4700::1111` / `2606:4700:4700::1001` | 独立基准测试中最快，隐私优先，24 小时清除日志 |
| Cloudflare（家庭版） | `1.1.1.3` / `1.0.0.3` | `2606:4700:4700::1113` / `2606:4700:4700::1003` | 拦截成人内容 |
| Google | `8.8.8.8` / `8.8.4.4` | `2001:4860:4860::8888` / `2001:4860:4860::8844` | 全球最大，24–48 小时匿名日志 |
| Quad9 | `9.9.9.9` / `149.112.112.112` | `2620:fe::fe` / `2620:fe::9` | 默认拦截恶意域名，瑞士基金会，DNSSEC 强制校验 |
| Quad9（不拦截） | `9.9.9.10` | `2620:fe::10` | 只校验 DNSSEC，不过滤，适合怕误杀 |
| OpenDNS（Cisco） | `208.67.222.222` / `208.67.220.220` | `2620:119:35::35` / `2620:119:53::53` | 家长控制与域名过滤 |
| AdGuard（默认） | `94.140.14.14` / `94.140.15.15` | `2a10:50c0::ad1:ff` / `2a10:50c0::ad2:ff` | 拦截广告与跟踪器，全球 100+ 节点 |
| AdGuard（家庭） | `94.140.14.15` / `94.140.15.16` | `2a10:50c0::bad1:ff` / `2a10:50c0::bad2:ff` | 追加成人内容拦截 + 安全搜索 |
| AdGuard（无过滤） | `94.140.14.140` / `94.140.14.141` | `2a10:50c0::1:ff` / `2a10:50c0::2:ff` | 只加速，不过滤 |
| CleanBrowsing（安全） | `185.228.168.9` / `185.228.169.9` | `2a0d:2a00:1::2` / `2a0d:2a00:2::2` | 拦截钓鱼、垃圾邮件、恶意软件 |
| CleanBrowsing（家庭） | `185.228.168.168` / `185.228.169.168` | `2a0d:2a00:1::1` / `2a0d:2a00:2::1` | 追加成人内容拦截 |
| Control D | `76.76.2.0` / `76.76.10.0` | `2606:1a40::` / `2606:1a40:1::` | 多配置文件可选，免费额度够用 |
| NextDNS | `45.90.28.0` / `45.90.30.0` | `2a07:a8c0::` / `2a07:a8c1::` | 可自定义屏蔽清单与统计，免费 30 万次/月 |
| Mullvad | `194.242.2.2` / `193.19.108.2` | `2a07:e340::2` | 零日志，含 DoH/DoT |
| Yandex | `77.88.8.8` / `77.88.8.1` | `2a02:6b8::feed:0ff` / `2a02:6b8:0:1::feed:0ff` | 俄罗斯服务，国内延迟高 |
| DNS.WATCH | `84.200.69.80` / `84.200.70.40` | `2001:1608:10:25::1c04:b12f` / `2001:1608:10:25::9249:d69b` | 无过滤、无日志、不记录 IP |
| DNS.SB | `185.222.222.222` / `45.11.45.11` | `2a09::` / `2a11::` | 欧洲，非营利 |
| Comodo Secure | `8.26.56.26` / `8.20.247.20` | — | 恶意软件拦截 |
| UltraDNS（Verisign） | `156.154.70.1` / `156.154.71.1` | `2610:a1:1018::1` / `2610:a1:1019::1` | 域名安全，家庭/企业过滤可选 |
| Hurricane Electric | `74.82.42.42` | `2001:470:20::2` | 老牌 Anycast |

> **数据准确性说明**：上表地址均来自服务商官网或官方文档，其中 AdGuard 的 IPv6 采用官方 `adguard-dns.io/zh_cn/public-dns.html` 公布的 `2a10:50c0::…` 系列。部分第三方汇总站点写作 `2a00:5a60::…`，**与官方不符，未采用**。若日后发现某条地址失效，以官网为准并在界面标注"待核实"，不要静默替换。

#### 界面呈现：v4 / v6 独立可控

这是本节的核心交互要求——**不能只写 IPv4**，也不能默认强行写 IPv6。

```
┌─ DNS 设置 ──────────────────────────────────────────────┐
│ 网卡    [WLAN ▾]        当前：自动获取 (DHCP)            │
│                                                             │
│ 服务商  [搜索或筛选…                                     ]  │
│         ┌──────────────────────────────────────────┐      │
│         │ ● 阿里 AliDNS        国内 · 支持加密     │      │
│         │   223.5.5.5 / 223.6.6.6                 │      │
│         │   2400:3200::1 / 2400:3200:baba::1      │      │
│         │ ○ Cloudflare         全球 · 最快 · 无日志 │      │
│         │   1.1.1.1 / 1.0.0.1                    │      │
│         │   2606:4700:4700::1111 / ::1001         │      │
│         └──────────────────────────────────────────┘      │
│                                                             │
│ 地址族  ☑ IPv4    ☑ IPv6                                  │
│         └─ 取消 IPv6 时该族置为「自动获取」                │
│                                                             │
│ 预览：将在 WLAN 上设置                                    │
│   IPv4 → 223.5.5.5, 223.6.6.6                             │
│   IPv6 → 2400:3200::1, 2400:3200:baba::1                  │
│                                                             │
│                              [ 还原为自动 ]  [ 应用 ]        │
└─────────────────────────────────────────────────────────────┘
```

**三条交互规则：**

1. **地址族独立勾选**，默认 IPv4 勾选、IPv6 不勾选。理由：多数用户开着 IPv6 但没有可用的 IPv6 DNS，强行写入 IPv6 反而制造"部分域名解析慢"的怪问题。需要时用户自己勾。
2. **取消勾选某族 = 该族还原为自动获取**，而不是"保持上次的 IPv6 地址"。这消除了"上次开了 IPv6，这次关了 IPv4 结果 IPv6 还留着"的隐性残留。
3. **写前预览，写后回读**。预览区明确显示即将写入的内容；写入后重新读 `Get-DnsClientServerAddress` 校验，不一致时报告（见 §4.2 第 3 条的组策略场景）。

#### 自定义 DNS

用户手填时按族分别校验：

- **IPv4 校验**：四段 0–255，端口可选（`1.1.1.1:853`）
- **IPv6 校验**：支持压缩写法与 `%` zone-id（link-local 场景），写入前须能被 `IpAddr::from_str` 解析
- **族不匹配直接拒绝**：把 IPv4 地址填进 IPv6 输入框时在输入框标红，不静默丢弃
- **至少填一个族**，允许只填 v4 或只填 v6
- 保留用户自定义列表为独立的「我的 DNS」，支持重命名与拖拽排序，不与内置清单混在一起

#### 自定义项的 DNS 泄漏提示

用户填了一个只监听在局域网的 DNS（如 AdGuard Home、Pi-hole 的 `192.168.1.2`）时，如果切换 Wi-Fi 导致 IP 变了，解析会直接失败。**这不是 bug，但用户一定会当成 bug。** 界面在自定义 DNS 卡片上标注：

> 自定义 DNS 不可达时，本工具会显示「当前 DNS 无响应」并提示切回系统默认，不会自动回滚（避免误判你正在用防火墙拦截）。

### 4.4 DoH（加密 DNS）——列为 v2

Windows 11 支持 DoH，但机制是"注册模板 + 强开标志"两步，不是单纯填地址：

```powershell
Add-DnsClientDohServerAddress -ServerAddress 1.1.1.1 `
  -DohTemplate "https://cloudflare-dns.com/dns-query" `
  -AllowFallbackToUdp $False -AutoUpgrade $True
```

再在 `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\InterfaceSpecificParameters\{接口GUID}\DohInterfaceSettings\Doh\1.1.1.1` 下写 `DohFlags = 1` 强制启用。

**内置清单里的加密端点**（v2 实施时使用，用户可自行填写）：

| 服务商 | DoH | DoT |
|---|---|---|
| 阿里 AliDNS | `https://dns.alidns.com/dns-query` | `dns.alidns.com` |
| 腾讯 DNSPod | `https://doh.pub/dns-query` | `dot.pub` |
| Cloudflare | `https://cloudflare-dns.com/dns-query` | `cloudflare-dns.com` |
| Google | `https://dns.google/dns-query` | `dns.google` |
| Quad9 | `https://dns.quad9.net/dns-query` | `dns.quad9.net` |
| AdGuard | `https://dns.adguard-dns.com/dns-query` | `dns.adguard-dns.com` |

**v1 不做 DoH 的理由**：涉及 HKLM 写入（比 HKCU 更敏感）、依赖 Windows 11 特性（Win10 支持不完整）、且 Chrome/Edge 自带 DoH 会在应用层覆盖系统设置，做了容易被误解为"没生效"。**这一条要在界面上说清楚**——用户开了 Chrome 的"安全 DNS"，系统 DNS 设置对 Chrome 就是无效的，这和 hosts 被绕过是同一个坑。

**但 v1 必须做的一件事**：在 DNS 面板提供一个只读提示区，列出上表端点供用户复制粘贴。因为用户配了加密 DNS 后一定会来找本工具问"能不能填 DoH 地址"，与其让用户去网上搜，不如直接把地址摆在界面上。

---

### 4.5 图标设计

#### 4.5.1 设计约束

图标要在三个互相冲突的尺寸下同时成立，这是全部设计决策的出发点：

| 场景 | 尺寸 | 约束 |
|---|---|---|
| 桌面图标 / 安装器 | 256px | 可容纳细节与层次 |
| 任务栏 | 32 / 48px | 剪影必须可辨 |
| **系统托盘** | **16 / 20 / 24px** | **硬约束：任何 1px 以下的细节都会消失** |

叠加上"极简现代"的要求后，有效解空间很窄：笔画不低于 2px（小尺寸下）、圆角不低于半径的一半、元素不超过 4 个。

#### 4.5.2 方案演进

先做了四稿结构迥异的候选（刻意不微调同一形状，因为那样失效点会重叠）：

| 稿 | 结构 | 大尺寸 | 16–20px | 结论 |
|---|---|---|---|---|
| **A 层** | 三条横条，中层换色 | 略像菜单按钮 | **三条横条仍清晰** | ✅ 唯一小尺寸不塌 |
| B 节点 | 对角连线 + 三节点 | 好 | 连线淡到消失，退化为散点 | 一般，20px 下像通用「分享」 |
| C 环 | 中心点 + 环形轨道 | 好 | 30% 透明度的环直接消失，只剩四散点 | ❌ 退化最严重 |
| D 速 | 连续 S 形笔画 | **最有个性** | 两段弧粘连成团 | 大尺寸赢，小尺寸输 |

**选 A。** 但 A 的原始形态（三条等宽横条）视觉上就是「汉堡菜单」，这是必须修掉的缺陷。

#### 4.5.3 修掉菜单感：右边缘对齐

做法是把三条横条的**右边缘**统一对齐到同一 x，左边缘做参差（52 / 40 / 46）：

```
原始（等宽 → 像菜单）        修正（右对齐 → 像层叠）
 ▬▬▬▬▬▬▬▬                     ▬▬▬▬▬▬▬▬
 ▬▬▬▬▬▬▬▬              →      ▬▬▬▬▬▬
 ▬▬▬▬▬▬▬▬                     ▬▬▬▬▬▬
```

右对齐产生一个不齐的左轮廓，视觉上读作「一层层的堆叠」而不是「并排的列表项」。中层用强调色，语义是「当前生效的层」——正好对应 §0.2 的三层模型。

#### 4.5.4 规格表

设计网格 120 单位，导出时按比例缩放：

| 元素 | 几何 | 说明 |
|---|---|---|
| 应用图标底板 | 120×120，圆角 26 | 圆角率 21.7% |
| 横条高度 | 12 | 垂直中心 y = 42 / 60 / 78 |
| 横条圆角 | 6 | = 高度的一半，即胶囊形 |
| 横条右边缘 | 统一 x = 86 | 脚本自检：三条右端像素均为 183 |
| 横条左边缘 | x = 34 / 46 / 40 | 参差，消除菜单感 |
| 上 / 下层 | `#FFFFFF` | |
| 中层强调色 | `#EF9F27`（琥珀） | 暖色，与近黑底形成高对比 |
| 应用图标底色 | `#2C2C2A` | 近黑暖灰，非纯黑，降低刺眼感 |

**为什么强调色选琥珀而不是蓝？** 近黑底上蓝色对比不足（深蓝接近底色），且蓝色在网络工具里过度泛滥。琥珀在深底上有足够亮度，小尺寸下三个层级的关系也更容易分辨。

#### 4.5.5 托盘图标必须做双版本

**单色托盘图形在 Windows 上必然有一态是隐形的**——白色图形在浅色任务栏上完全看不见。Windows 10/11 任务栏跟随系统主题分深浅两态，因此托盘图标必须出两版：

| 版本 | 图形配色 | 用于 |
|---|---|---|
| `tray-*.png` / `sunet-tray-dark.ico` | 白色 + 琥珀 | 深色任务栏（多数人默认） |
| `tray-light-*.png` / `sunet-tray-light.ico` | 近黑 + 琥珀 | 浅色任务栏 |

运行时判定：

```
读取 HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize
    → AppsUseLightTheme (DWORD)
      0 / 不存在 → 用深色版
      1         → 用浅色版
```

**这个判定要放在托盘初始化时，并且监听主题变更**（`WM_SETTINGCHANGE` 广播 `ImmersiveColorSet`）后重建图标，否则用户在 Windows 设置里切换主题后，托盘图标会变成隐形。

透明底版本的图形比带底板版放大约 1.28 倍（`k = 1.28`），否则同样的图形在无底板时会显得偏小、视觉重量不足。

#### 4.5.6 交付资产

`outputs/netbox-design/icons/` 共 29 个文件，总计 73 KB：

```
icon.svg / icon-{16,20,24,32,40,48,64,128,256,512}.png   应用图标（含底板）
icon-light-{32,128,256}.png                             浅色底适配版
tray.svg / tray-{16,20,24,32,48}.png                    托盘深色版（透明底）
tray-light-{16,20,24,32,48}.png                        托盘浅色版（透明底）
sunet.ico                                             多尺寸 ICO（16–256）
sunet-tray.ico / sunet-tray-dark.ico / sunet-tray-light.ico
```

生成脚本：`outputs/netbox-design/gen_icons.py`，改配色或几何只需改文件顶部的常量，改完重跑即可全量再生。

#### 4.5.7 未采用的方案

- **把「速」或「S」做成字形标**：中文或英文字母在 16px 下笔画会粘连，且与产品功能无直接语义关联
- **用渐变 / 阴影 / 发光**：渐变在小尺寸下会变成脏边，且与"极简"要求冲突。全部使用纯色填充
- **多色图标（≥3 色）**：托盘 16px 下多种颜色互相干扰，识别率反降
- **动态图标**（切换方案时变色）：Tauri 支持但需额外状态同步，且收益不足以覆盖复杂度

---

## 5. 托盘常驻与开机自启

### 5.1 托盘设计

```rust
use tauri::{
    menu::{Menu, MenuItem, CheckMenuItem, PredefinedMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState},
    Manager,
};

let status   = MenuItem::with_id(app, "status", "当前：未启用任何方案", false, None::<&str>)?;
let sep1     = PredefinedMenuItem::separator(app)?;
let profile  = MenuItem::with_id(app, "profile", "切换方案…", true, None::<&str>)?;
let proxy    = CheckMenuItem::with_id(app, "proxy", "系统代理", true, false, "启用系统代理")?;
let clearp   = MenuItem::with_id(app, "clear_proxy", "清除代理设置  Ctrl+Alt+S", true, None::<&str>)?;
let dns      = MenuItem::with_id(app, "dns", "切换 DNS…", true, None::<&str>)?;
let hosts    = MenuItem::with_id(app, "hosts", "编辑 hosts…", true, None::<&str>)?;
let sep2     = PredefinedMenuItem::separator(app)?;
let autostart= CheckMenuItem::with_id(app, "autostart", "开机自启动", true, false, "开机自动启动")?;
let open     = MenuItem::with_id(app, "open", "打开主窗口", true, None::<&str>)?;
let quit     = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

let menu = Menu::with_items(app, &[
    &status, &sep1, &profile, &proxy, &clearp, &dns, &hosts,
    &sep2, &autostart, &open, &quit,
])?;

TrayIconBuilder::with_id("main-tray")
    .icon(load_tray_icon(&app.handle()))
    // 图标必须内嵌字节，不能用相对路径；且必须按任务栏主题选深/浅色版，详见 §4.5.5
    .tooltip("SuNet — 当前：默认配置")
    .menu(&menu)
    .show_menu_on_left_click(false)              // 左键留给"显示/隐藏窗口"
    .on_menu_event(|app, event| { /* 路由到 cmd::* */ })
    .on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event {
            let app = tray.app_handle();
            let win = app.get_webview_window("main").unwrap();
            if win.is_visible().unwrap_or(false) {
                let _ = win.hide();
            } else {
                let _ = win.unminimize();
                let _ = win.center();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
    })
    .build(app)?;
```

**交互约定：**

| 操作 | 行为 |
|---|---|
| 左键单击 | 显示 / 隐藏主窗口（窗口显示时定位到屏幕中心） |
| 右键单击 | 弹出菜单 |
| 双击托盘 | 同左键 |
| 鼠标悬停 | Tooltip 显示当前生效方案 + 代理/DNS 状态摘要 |
| 点窗口关闭按钮 | **隐藏到托盘**（不是退出），`api.prevent_close()` |
| 托盘"退出" | 真正退出，触发还原逻辑（§6.3） |

**图标必须用 `include_bytes!` 内嵌。** 前端 `TrayIcon.new()` 的 `icon` 参数只接受绝对路径，打包后路径不存在会导致图标透明——这是社区里已确认的坑（Juejin Tauri 2 实战文章明确提到）。方案里统一走 Rust 侧构建，前端不用托盘 API。

### 5.2 开机自启：计划任务不需要提权

```powershell
# 一次性创建（安装器中执行）
schtasks /Create `
  /TN "SuNet" `
  /TR "\"C:\Program Files\SuNet\SuNet.exe\" --minimized" `
  /SC ONLOGON `
  /RL LIMITED `
  /RU "%USERNAME%" `
  /F
```

**注意 `/RL LIMITED`——这是本版本相对前一版的改动。** 主进程现在是 `asInvoker`，用不上 HIGHEST；写 HIGHEST 反而会让 UAC 弹回来（`Run with highest privileges` 的任务被非提权进程触发时，若任务需要更高权限则会走 UAC 协商）。

参数取舍：

- **`/RL LIMITED`** — 按普通权限运行。主进程不需要提权，这是模型 C 的一致性要求
- **`/SC ONLOGON` 而非 `/SC ONSTART`** — 登录后启动，才能拿到用户会话（托盘需要交互式会话；且 `HKCU` 必须属于交互用户）
- **`/RU "%USERNAME%"`** — 必须是当前交互用户。写成 SYSTEM 或其他账户会导致：托盘图标不显示（Session 0 隔离）、`HKCU` 指向错误用户
- **`/IT`（交互式运行）** — `schtasks` 下 `/RU` 与当前登录用户一致时默认交互运行；显式加 `/IT` 更稳妥
- **`/TR` 里用 `\"` 转义并加 `--minimized`** — 开机不弹窗口

**删除自启**：`schtasks /Delete /TN "SuNet" /F`

**为什么不用 `Run` 键 + `tauri-plugin-autostart`？** 现在主进程是 `asInvoker`，理论上 `Run` 键（`HKCU\...\Run`）就够了，且更简单。但仍选计划任务，理由是：

1. `Run` 键方式在部分企业策略下被组策略禁用或重定向
2. 计划任务可以设置「登录时」而非「系统启动时」，语义更准确（`ONSTART` 时用户未登录，拿不到会话）
3. 后续若要加「每日定时同步 hosts 订阅」，同一个任务调度器就能承载，不必再引入一套机制

`tauri-plugin-autostart` 作为 v2 可选保留。

**「开机自动恢复上次方案」的实现差异**：这一项**需要**提权（要写 hosts 和 DNS），所以它不能放在开机任务里直接跑。正确做法是：开机任务只启动主进程（不恢复），主进程启动后若检测到 `--restore` 标记，再按 §1.4.4 的方式按需拉起提权子进程执行恢复逻辑。**UAC 在用户登录后弹一次，而不是在登录前从任务计划程序里弹。**

### 5.3 启动参数与单实例

```
SuNet.exe              → 正常启动，显示主窗口
SuNet.exe --minimized  → 启动即隐藏到托盘（自启用）
SuNet.exe --restore    → 启动后自动恢复上次生效的方案
```

**单实例**：`tauri-plugin-single-instance`。第二次启动时，把参数转发给已有实例并唤起其窗口。否则多实例会互相覆盖 hosts 区块和代理状态——这是必须防的。

---

### 5.4 托盘状态模型与通知规范

§4.5.7 已决定**不做动态图标**（切换方案不改图标颜色）。那么"用户如何在不打开主窗口的情况下知道当前状态"就必须由另外三样东西承担。本节把它们定死，避免实现时各自发挥。

#### 5.4.1 状态聚合

三层状态聚合成一个托盘状态对象，主进程在每次变更后重算并推送到托盘与前端：

```rust
pub struct TrayStatus {
    pub profile_name: Option<String>,   // 当前生效方案；None = 未启用任何方案
    pub hosts_count: usize,             // 托管区块内有效条目数
    pub proxy: ProxyStatus,             // Off | On{host,port} | Pac | Unknown
    pub dns: DnsStatus,                 // 分族：V4Auto | V4Manual(addrs) / V6Auto | V6Manual(addrs)
    pub dirty: bool,                    // 是否存在未回滚的变更（§6.3）
}
```

#### 5.4.2 Tooltip 文案规范

单行，`·` 分隔，字段**固定顺序**，总长 ≤ 127 字符（Windows 通知区域 tooltip 上限）。超长时**截断 DNS 地址部分**，不截断代理状态——代理是唯一会"全局断网"的层，它的状态优先级最高：

```
SuNet · 开发加速 · 代理 127.0.0.1:7890 · DNS 手动(v4) 自动(v6) · hosts 42 条
SuNet · 未启用任何方案 · 代理 关闭 · DNS 自动(v4) 自动(v6)
```

#### 5.4.3 菜单勾选状态必须反映真实状态

`CheckMenuItem` 的勾选**由代码在每次状态变更后设置**，不能显示配置文件里的值。菜单弹出前重新读一次真实状态（`proxy_get` / `dns_get`），保证"菜单说开着、实际已经关了"这类不一致不可能出现。

这与 §5.5.3「'已启用'只允许在注册成功后显示」是同一条原则的不同落点。

菜单首项「当前：…」是**禁用项**（`enabled = false`），只作状态展示，不可点击。

#### 5.4.4 气泡通知规范

| 事件 | 通知 | 时长 | 可操作性 |
|---|---|---|---|
| 方案切换成功 | ✅ | 3s | 无 |
| 方案切换失败并已回滚 | ✅ | 8s | 「查看详情」→ 日志 |
| 切换失败且需人工介入（Recovery） | ✅ | **常驻** | 「查看差异」+「打开备份目录」 |
| 全局热键清除代理成功 | ✅ | 5s | **「撤销」**（§5.5.7） |
| 热键注册失败 | ✅ | 8s | 「去设置」 |
| 订阅同步成功且内容有变化 | ✅ | 3s | 「查看变更」 |
| 订阅同步失败 | ✅ | 8s | 「重试」 |
| 开机自动恢复完成 | ✅ | 3s | 无 |
| 常规状态变化（开关窗口、切换标签页） | ❌ | — | — |

**原则：只对"用户可能没预期到的结果"发通知。** 用户刚在界面上点的按钮，成功不弹气泡（界面内已有反馈）；只有失败、后台自动发生的事、以及带时效撤销窗口的操作才弹。

#### 5.4.5 通知不可用时的降级

专注助手、勿扰模式、或用户在系统里关闭了通知，气泡会被静默丢弃。降级链路：

```
气泡发送失败 / 被系统抑制
  ├─ 主窗口若可见 → 窗口内顶部横幅替代气泡
  ├─ 主窗口若隐藏 → 托盘 tooltip 前置一个「⚠ 」前缀，直到用户打开窗口
  └─ 无论哪种，都写一条 info 日志（§14.1）
```

**带时效的「撤销」不能只依赖气泡。** §5.5.7 已在主界面保留常驻的「撤销上次清除」入口，气泡只是加速路径——这条必须保持，因为用户看不到气泡时，撤销能力不能一起消失。

---

### 5.5 快捷键：全局热键设计

#### 5.5.1 必须是全局热键，不是窗口内热键

这是本节的前提，容易被忽略：**程序常驻托盘，主窗口绝大多数时间是隐藏的。** 窗口内热键（前端 `keydown` 监听）在窗口隐藏时不生效，那样的快捷键毫无意义。

所以必须走 `RegisterHotKey` 注册**系统级**热键，由系统投递 `WM_HOTKEY` 消息。这带来一个必须说清的技术特性：

> `WM_HOTKEY` 投递到**注册者自己的消息队列**，与当前焦点窗口是谁无关。

这就是"窗口隐藏也能用"的依据，也是本设计成立的前提。

**明确不采用低级键盘钩子（`WH_KEYBOARD_LL`）。** 钩子虽然能"看到"所有按键，但有三个问题：能看到用户的全部输入（隐私风险）；无法阻止其他程序同时响应（多个软件绑同一组合会都触发）；无法在系统热键表里登记，排查工具看不到。`RegisterHotKey` 是官方通道，行为可预期。

#### 5.5.2 默认键位与冲突面评估

**默认 `Ctrl + Alt + S`**，可由用户在设置中自行修改。

选三键而非两键，是因为两键组合在国内机器上几乎必然冲突。已核实的常见占用：

| 组合 | 占用者 | 冲突程度 |
|---|---|---|
| `Alt + A` | 微信 | 极高 |
| `Ctrl + Alt + A` | QQ 截图、钉钉 | 高 |
| `Ctrl + Alt + W` | 微信 | 高 |
| `Ctrl + Alt + 方向键` | 网易云音乐、Snipaste | 高 |
| `F1` | Snipaste、部分输入法 | 高 |
| **`Ctrl + Alt + S`** | **无常见占用** | **低** |

**必须避开的系统保留组合**（注册会被拒或行为异常）：

```
Ctrl + Alt + Del      Win + L            Win + U            Win + E
Win + D               Win + I            Win + S            Win + G
Win + P               Win + Tab          Win + 方向键        Alt + Tab
Win + Ctrl + Shift + B
```

**不要用 `Win` 键作为主修饰键。** 它在 Windows 11 里保护级别最高，绝大多数组合被系统保留，且桌面、任务视图、设置等全部内置功能占用它。

#### 5.5.3 注册失败必须处理——最常见的实现 bug

`RegisterHotKey` 在组合已被占用时返回 0，`GetLastError()` 为 **`ERROR_HOTKEY_ALREADY_REGISTERED` (1409)**。

**大量软件犯的错是：不检查返回值。** 注册失败后继续静默运行，用户按下快捷键毫无反应，而设置里显示"已启用"，用户完全无从判断问题在哪。

本设计的处理：

```
注册流程：
  1. 用户在设置里选定组合（或使用默认 Ctrl+Alt+S）
  2. is_available(组合) 预检 —— 见 §5.5.4
     ├─ 不可用 → 输入框标红 + 「该组合已被其他程序占用（E5001），请更换」
     │            给出「重新检测」与「查看排查方法」两个出口
     │            ※ 不显示占用者程序名 —— 官方 API 拿不到，见 §5.5.4 第 3 点
     └─ 可用 → 进入注册
  3. app.global_shortcut().register(...)
     ├─ Ok   → 持久化 binding 到 config.json，UI 显示「已启用」
     └─ Err  → 按错误码分流（§14.3）：
               1409 → E5001 已被占用；被保留 → E5002；被安全软件拦 → E5003
               气泡提示 + 自动打开设置界面；配置里仍保留 binding，
               但 UI 显示「未生效」而非「已启用」
```

**「已启用」这个状态只允许在注册成功后才显示。** 界面上的开关状态必须反映真实的注册结果，不能读配置文件就显示已启用。

#### 5.5.4 冲突预检：试探性注册

Win32 **没有**「查询某组合是否已被占用」的接口。但可以借 `RegisterHotKey` 的独占语义做软探测：

```rust
const PROBE_ID: i32 = 0x5F27;   // 选用大概率不与业务 ID 冲突的值

fn is_hotkey_available(mods: Modifiers, code: Code) -> bool {
    unsafe {
        // 传 hWnd = NULL：不需要窗口，只探测系统热键表
        let ok = RegisterHotKeyW(None, PROBE_ID, mods.0, code.keycode() as u32);
        if ok {
            UnregisterHotKey(None, PROBE_ID);   // 探测必须立即释放，否则自己占住了
            true
        } else {
            false   // GetLastError() == 1409 即已被占用
        }
    }
}
```

**三点注意事项：**

1. **探测成功后必须立即 `UnregisterHotKey`**，否则自己把这个组合占了。
2. **探测与应用注册之间存在时间窗**，期间其他程序仍可能抢占。所以正式注册失败时还要再走一次错误处理，不能因为"预检通过"就假定注册一定成功。
3. **占用者程序名无法通过官方 API 直接获得。** 只能提示用户用 Hotkey Detective / Process Explorer 排查。**不要谎称能显示占用者**——这是做不到的。

#### 5.5.5 触发时机：只响应 Pressed

```rust
.with_handler(|app, shortcut, event| {
    if event.state() != ShortcutState::Pressed {
        return;                       // 必须过滤 Released，否则一次按键触发两次
    }
    // …执行清代理
})
```

`WM_HOTKEY` 在按下与释放时各投递一次，插件把两者都传给 handler。**不过滤会导致一次按键执行两遍清代理**——虽然结果一样，但会产生两次通知与两次 `ApplyReport`，日志也会重复。

#### 5.5.6 「彻底清零」的实现与代价

按你的选择，快捷键执行的是**彻底清零**，而非快照还原：

```
清除代理（彻底清零）：
  1. 读取当前 ProxyEnable / ProxyServer / AutoConfigURL / ProxyOverride
  2. 写入 ProxyEnable = 0
  3. 删除 AutoConfigURL 键值（若存在）—— 这一步是必需的！
     因为 AutoConfigURL 存在时 PAC 会接管，ProxyEnable=0 并不生效，
     "清零"就是假的
  4. 清空 ProxyServer（REG_SZ 置空字符串）
  5. 保留 ProxyOverride —— 它是绕过列表，保留无害且有益，
     清空反而会让 *.local、内网地址这些本该直连的域名走代理
  6. InternetSetOptionW(39 SETTINGS_CHANGED) + (37 REFRESH) 通知
  7. 回读校验 ProxyEnable == 0
```

**第 3 步是最容易漏的。** 很多人以为 `ProxyEnable=0` 就等于关了代理，但如果系统里存在 `AutoConfigURL`（企业 PAC 配置），浏览器仍会走 PAC，界面上显示"已关闭"但实际没关。这是"彻底清零"必须包含 `AutoConfigURL` 的原因。

**这个操作的代价，必须说清楚：** 它不区分代理的来源。你公司的 VPN 客户端、某些企业网关客户端、以及浏览器侧的 PAC 配置，都写在同一个注册表键下。彻底清零会把它们一起抹掉，需要用户重新配置公司网络后才能恢复。

**因此本设计强制配套一个撤销机制。**

#### 5.5.7 撤销机制：把误触代价降到接近零

`Ctrl + Alt + S` 是好按的键，误触的概率不低；而误触的代价是断网。撤销机制让这个代价接近于零。

```
按 Ctrl+Alt+S：
  ├─ 内存暂存 clear_state = { ProxyEnable, ProxyServer, AutoConfigURL(有无), ProxyOverride }
  ├─ 执行彻底清零
  ├─ 托盘气泡：「已清除系统代理设置  [撤销]  5 秒后自动消失」
  └─ 用户点「撤销」→ 把 clear_state 原样写回 + 通知系统 + 气泡「已撤销」
     超时未点 → 丢弃暂存（但仍写入 last_proxy_clear 到日志，便于排查）
```

撤销必须满足三个约束，缺一不可：

| 约束 | 原因 |
|---|---|
| 暂存在**内存**，不落盘 | 落盘会变成另一份配置来源，程序崩溃后可能被误用 |
| 只保留**最近一次** | 用户不需要撤销两次操作之后的操作 |
| 写回时同样走 `InternetSetOptionW` 通知 | 否则浏览器仍沿用旧缓存，撤销"看起来没生效" |

撤销入口做两处：托盘气泡按钮 + 主界面代理页的「撤销上次清除」链接（气泡消失后仍可用）。

#### 5.5.8 与托盘菜单的关系

托盘右键菜单同时提供「清除代理」（`Ctrl+Alt+S`）作为等价路径，**两条路径调用同一个函数**，共享互斥锁与快照逻辑。

这样设计的理由：全局热键可能被占用而注册失败，但功能本身必须始终可达——**快捷键是加速器，不是唯一入口**。快捷键不可用时用户仍有两条路（托盘菜单、主界面），不会出现"功能被热键占用锁死"的情况。

#### 5.5.9 生命周期与异常处理

| 场景 | 处理 |
|---|---|
| 用户改了组合 | 先 `unregister` 旧组合，再 `register` 新组合；任一步失败则回滚到旧组合 |
| 程序退出 | 进程结束由系统自动释放热键，无需手动；插件在 `Drop` 中做清理 |
| **僵尸占用** | 其他程序崩溃后热键注册未及时释放，组合被"不存在的进程"占着。设置界面提供「重新检测」按钮，用户可换组合绕过 |
| 中文输入法 | `Ctrl+Alt+S` 在部分输入法下可能被截获。实现时监听 `WM_INPUT` 无意义（全局热键在输入法之前），但需在文档中提示：**若输入法吞键，改用纯 `Ctrl+Shift+字母` 组合** |
| 安全软件拦截 | 极少数杀软会阻止全局热键注册。捕获注册失败并提示，不尝试绕过 |

#### 5.5.10 配置结构

```jsonc
{
  "settings": {
    // ...
    "shortcuts": {
      "clear_proxy": {
        "enabled": true,
        "binding": "Ctrl+Alt+S"        // 用户可改
        // 注意：注册是否成功是运行时状态，不写进配置文件——
        // 读配置文件就显示"已启用"正是 §5.5.3 要避免的那个 bug。
        // 前端通过 shortcut_status 命令拿真实注册结果。
      }
    }
  }
}
```

`binding` 用统一字符串格式（`Ctrl` / `Alt` / `Shift` / `CmdOrCtrl` + `+` 分隔），与 `tauri-plugin-global-shortcut` 的解析格式一致，避免自建一套语法。

#### 5.5.11 权限：与 §1.4 模型 C 完全契合

**清除代理不需要管理员权限**（`HKCU` 用户 hive 可写，见 §1.4.1）。

所以：**这个快捷键触发的是全程静默操作，不会弹 UAC。** 这是模型 C 的直接收益——如果沿用整程序提权，每按一次快捷键都会弹一次 UAC，这个功能就废了。

```rust
// 主进程直接执行，不经过 elevation.rs
fn clear_proxy(app: &AppHandle) -> Result<ClearReport> {
    let _guard = PROXY_LOCK.lock()?;
    let before = proxy::read_state()?;          // HKCU 读写，无需提权
    proxy::hard_clear()?;                        // ProxyEnable=0 + 删 AutoConfigURL
    wininet::notify_changed()?;                  // InternetSetOptionW 39/37
    let ok = proxy::verify_disabled()?;          // 回读校验
    app.emit("proxy-cleared", ClearReport { before, ok })?;
    Ok(…)
}
```

**这也意味着全局热键注册本身不需要管理员权限**，与 §1.4.9 的"主进程 asInvoker"一致。若实现阶段发现某台机器上 `RegisterHotKey` 注册失败（部分安全软件或严格策略环境），应在错误提示中明确区分「热键注册失败」与「权限不足」两种原因，不要混为一谈。

---

## 6. 切换引擎：事务式应用与回滚

这是整个工具的**核心**，也是最容易写垮的部分。三个操作任一失败，如果不做回滚，用户会卡在"hosts 改了、代理开了、DNS 没改"的中间态，而且不知道如何恢复。

### 6.1 状态机

```
        ┌─────────┐
        │  Idle   │◀──────────────────────┐
        └────┬────┘                       │
             │ apply(profile)             │ rollback() 全部成功
             ▼                            │
        ┌─────────┐                       │
        │ Backup  │ 快照三类原始状态       │
        └────┬────┘                       │
             ▼                            │
        ┌─────────┐                       │
        │ Apply   │ hosts → proxy → dns   │  逐项：执行 → 回读校验
        │ ①hosts  │                       │
        │ ②proxy  │                       │
        │ ③dns    │                       │
        └────┬────┘                       │
             ▼                            │
        ┌─────────┐                       │
        │  Verify │ 代理 TCP 探测          │
        └────┬────┘                       │
             │ 成功                        │
             ▼                            │
        ┌─────────┐                       │
        │ Committed│──────────────────────┘
        └─────────┘
             │ 任一步失败
             ▼
        ┌──────────┐
        │ Rollback │ 逆序还原：dns → proxy → hosts
        └────┬─────┘
             │ 回滚也失败
             ▼
        ┌──────────┐
        │ Recovery │ 弹出"需人工介入"，展示完整差异清单 + 一键打开备份目录
        └──────────┘
```

**逆序还原**是必须的：先恢复解析层（dns/hosts），再恢复传输层（proxy）。反过来可能出现"hosts 已还原但代理还开着且代理进程已死"的组合，导致连备份文件都打不开。

```rust
// DNS 必须分族建模 —— 界面上 v4 / v6 是两个独立开关
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsState {
    pub interface_alias: String,
    pub interface_guid: String,
    pub v4: DnsFamilyState,
    pub v6: DnsFamilyState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsFamilyState {
    pub is_dhcp: bool,          // true = 自动获取（不显示具体地址）
    pub servers: Vec<IpAddr>,   // is_dhcp 为 false 时有效
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DnsPreset {
    pub id: String,
    pub name: String,
    pub region: DnsRegion,           // Domestic | Global
    pub features: Vec<DnsFeature>,   // NoLog, AdBlock, MalwareBlock, Family, Encrypted
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub doh: Option<String>,         // v2 启用
    pub dot: Option<String>,
    pub is_custom: bool,             // 用户自定义项为 true
    pub is_favorite: bool,
}
```

### 6.2 快照内容

```rust
pub struct Snapshot {
    pub id: String,
    pub created_at: chrono::DateTime<Utc>,
    pub reason: String,              // "切换到方案 X" / "程序启动恢复"
    pub hosts_raw: Option<Vec<u8>>,  // 原文件完整字节（可能为 None：文件不存在）
    pub hosts_existed: bool,
    pub proxy: Option<ProxyState>,   // ProxyEnable/ProxyServer/ProxyOverride/AutoConfigURL
    pub proxy_existed: bool,
    // 每张被改过的网卡 × 每族独立记录
    pub dns: Vec<DnsInterfaceSnapshot>,
}

pub struct DnsInterfaceSnapshot {
    pub interface_alias: String,
    pub interface_guid: String,
    pub v4: DnsFamilyState,   // 精确到「原先是 DHCP 还是手动、地址是什么」
    pub v6: DnsFamilyState,
}
```

存 `hosts_raw` 原始字节而非结构化内容——还原时追求的是"逐字节回到原样"，任何重新序列化都可能引入差异。

快照落盘 `%APPDATA%\SuNet\snapshots\`，保留最近 20 份，自动清理。

### 6.3 退出与崩溃恢复

**退出时（正常路径）**：

```
用户配置项：退出时行为
  ● 保持当前状态（默认）          ← 与 config.json 的 on_exit: "keep" 一致
  ○ 还原为启动前状态
  ○ 若本次运行期间应用过方案，则还原
```

`保持当前状态` 是默认值——用户开了代理就是想一直在用，退出应用不等于关代理。这个默认值必须是这样，但要让用户看见。

**崩溃恢复（异常路径）**：

```
启动时检测：
  ~/.SuNet/session.marker 存在 且 内容为 { pid, started_at, dirty: true }
  → 说明上次异常退出
  → 读取最近一份快照
  → 顶部常驻黄条：「上次异常退出，已检测到未保存的网络配置变更。
                 [查看差异] [还原] [忽略]」
```

**不做自动还原**，因为无法区分"崩溃时用户正在用代理"和"崩溃时是切换到一半"。给信息 + 给选择，比擅自改系统状态安全。

---

## 7. 数据与配置

### 7.1 配置文件

`%APPDATA%\SuNet\config.json`

```jsonc
{
  "schema_version": 1,
  "settings": {
    "start_minimized": true,
    "autostart": { "enabled": true, "method": "scheduled_task" },
    "on_exit": "keep",                 // keep | restore_if_changed | always_restore
    "proxy_probe_before_enable": true,
    "language": "zh-CN",
    "theme": "follow_system",          // follow_system | light | dark
    "hosts_backup_keep": 20,
    "shortcuts": {
      "clear_proxy": { "enabled": true, "binding": "Ctrl+Alt+S" }
    }
  },
  "dns_presets": {
    // 内置清单（约 40 项，见 §4.3）编译进二进制，不进配置文件
    // 配置里只存「用户自定义」与「收藏」
    "custom": [
      {
        "id": "u-1",
        "name": "家里 AdGuard Home",
        "v4": ["192.168.1.2"],
        "v6": [],
        "doh": null,          // v2 启用
        "is_favorite": true
      }
    ]
  },
  "sources": [
    {
      "id": "src-github",
      "name": "GitHub 加速",
      "url": "https://cdn.jsdelivr.net/gh/Clov614/SteamHostSync@main/Hosts_github",
      "mirror": "https://cdn.statically.io/gh/Clov614/SteamHostSync@main/Hosts_github",
      "interval_hours": 24,
      "enabled": true,
      "last_sync": null,
      "last_status": "never"
    }
  ],
  "profiles": [
    {
      "id": "uuid-1",
      "name": "默认 · 直连",
      "is_builtin": true,
      "hosts": { "entry_ids": [], "source_ids": [] },
      "proxy": { "enabled": false, "host": "", "port": 0, "bypass": "localhost;127.*;<local>" },
      "dns": { "mode": "dhcp" }
    },
    {
      "id": "uuid-2",
      "name": "开发加速",
      "hosts": { "entry_ids": ["e1", "e2"], "source_ids": ["src-github"] },
      "proxy": { "enabled": true, "host": "127.0.0.1", "port": 7890, "bypass": "localhost;127.*;<local>" },
      "dns": {
        "mode": "manual",
        "interface_alias": "WLAN",
        "preset_id": "ali",           // 引用 §4.3 内置清单
        "v4": ["223.5.5.5", "223.6.6.6"],
        "v6": ["2400:3200::1", "2400:3200:baba::1"],
        "doh": null
      }
    }
  ],
  "active_profile_id": "uuid-1"
}
```

**DNS 配置的三点设计说明：**

1. **内置清单不进配置文件。** 约 40 条预设是产品的一部分，编译进二进制；配置文件只存用户的自定义项与收藏状态。这样升级软件能自动获得修正过的地址，不需要迁移用户配置。
2. **方案里同时存 `preset_id` 与展开后的 `v4`/`v6`。** 只存 `preset_id` 会导致内置地址日后被修正时，已存方案的行为跟着变且用户不知情；只存展开值则无法在界面显示"这是阿里"。两者都存，界面显示 `preset_id` 的名称，实际写入用展开值——快照时另外记录当时的展开值，保证回滚能精确还原。
3. **`v6` 缺省为空数组 = 该族保持 DHCP。** 这与 §4.3 交互规则第 2 条一致：方案层不允许留下"上次开了 IPv6"的隐性残留。

**内置方案 `默认 · 直连` 不可删除**（`is_builtin: true`）——它是任何回滚的终点。删掉它，回滚就没有安全的落点。

### 7.2 并发控制：进程内锁 + 跨进程锁（两层都要）

**只有一条 `Mutex` 是不够的。** 模型 C（§1.4）下，hosts/DNS 的写入发生在**另一个进程**（提权子进程）里，进程内的 `Mutex<()>` 对它没有任何约束力。两层锁各管一段：

| 层 | 机制 | 保护对象 |
|---|---|---|
| 进程内 | `static APPLY_LOCK: Mutex<()>` | 同一进程内并发的 `apply_*` / `rollback` 命令（Tauri 命令本来就是并发调度的） |
| **跨进程** | `CreateMutexW` + 命名互斥体 | **hosts / DNS 写入临界区**，覆盖主进程与提权子进程 |

```rust
// 进程内：所有 apply_* / rollback 命令先取这把锁
static APPLY_LOCK: Mutex<()> = Mutex::new(());

// 跨进程：命名互斥体，名字固定
//   用 Local\ 前缀（当前会话内可见）即可；Global\ 需要额外权限，跨会话也无必要
const IPC_MUTEX: &str = r"Local\SuNet.HostsDns.CriticalSection";

fn with_critical_section<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    let h = unsafe { CreateMutexW(None, false, w!(IPC_MUTEX))? };
    let _guard = HandleGuard(h);                 // Drop: ReleaseMutex + CloseHandle
    match unsafe { WaitForSingleObject(h, 10_000) } {
        WAIT_OBJECT_0  => f(),
        WAIT_ABANDONED => { log::warn!("上一个持有者异常退出，已接管临界区"); f() }
        WAIT_TIMEOUT   => bail!("另一个 SuNet 进程正在修改 hosts/DNS，请稍后重试"),
        _              => bail!("获取临界区失败"),
    }
}
```

**四个要点：**

1. **`WAIT_ABANDONED` 不是错误。** 它表示上一个持有者进程在持锁期间崩溃或被杀。内核会自动释放命名互斥体，此时应当**继续获取并记一条 warning**，而不是一直等下去。
2. **提权子进程必须走同一把命名锁**，不能因为"我是管理员"就跳过——它和主进程写的是同一份 hosts 文件。
3. **锁的粒度只包住写入临界区。** 不要把 §3.4 的 TCP 探测（800ms 超时）包进锁内，否则一次代理探测会把 hosts 写入阻塞近一秒，用户侧表现为"点了没反应"。
4. **锁要包住「读—改—写」全过程**，不能只包写入那一步。只锁写入的话，两个流程仍可能读到同一份旧内容然后各自覆盖。

**与 `single-instance` 插件的分工**：`single-instance` 防止开出第二个 GUI 窗口；命名互斥体保护写入临界区，覆盖"主进程 + 提权子进程"这对组合。**两者都不能省**——单实例插件管不到提权子进程，命名锁也管不到"用户手动双击了两次 exe"这种绕过插件的情形。

---

## 8. Tauri 命令接口

```rust
// 权限标注约定（对应 §1.4.1 的逐项拆解）：
//   [无提权] = 主进程可直接执行
//   [需提权] = 必须走 ShellExecuteW + runas 拉起 --task 子进程
// 注意：所有命令都在主进程执行；需提权的命令内部自动走提权分流，
// 前端不需要感知权限差异，只需要在返回的 ApplyReport 里看是否弹过 UAC。

// ---- 状态 ----
#[tauri::command] async fn get_app_state(app: AppHandle) -> Result<AppState>;
// AppState { current_profile_id, proxy_enabled, dns_state, hosts_stats, privilege_ok, autostart_enabled }

// ---- hosts ----
#[tauri::command] async fn hosts_list(app: AppHandle) -> Result<Vec<HostsEntry>>;
#[tauri::command] async fn hosts_upsert(app: AppHandle, entry: HostsEntry) -> Result<()>;   // [需提权]
#[tauri::command] async fn hosts_delete(app: AppHandle, id: String) -> Result<()>;      // [需提权]
#[tauri::command] async fn hosts_preview(app: AppHandle, entries: Vec<HostsEntry>) -> Result<String>;
// 返回将要写入的完整托管区块文本，供用户写入前确认
#[tauri::command] async fn sources_sync(app: AppHandle, id: String) -> Result<SyncResult>; // [无提权] 拉远程 + 写库
#[tauri::command] async fn sources_sync_all(app: AppHandle) -> Result<Vec<SyncResult>>;

// ---- proxy ----
#[tauri::command] async fn proxy_get(app: AppHandle) -> Result<ProxyState>;
#[tauri::command] async fn proxy_set(app: AppHandle, cfg: ProxyConfig, probe: bool) -> Result<()>;  // [无提权] HKCU
#[tauri::command] async fn proxy_clear(app: AppHandle) -> Result<ClearReport>;  // [无提权] 彻底清零 + 可撤销
#[tauri::command] async fn proxy_undo_clear(app: AppHandle) -> Result<()>;      // [无提权] 撤销上次清除

// ---- 全局热键 ----
#[tauri::command] async fn shortcut_check(app: AppHandle, binding: String) -> Result<bool>;  // 探测残抢
#[tauri::command] async fn shortcut_set(app: AppHandle, binding: String) -> Result<()>;     // 改定
#[tauri::command] async fn shortcut_status(app: AppHandle) -> Result<ShortcutStatus>;     // 真实注册状态

// ---- dns ----
#[tauri::command] async fn dns_interfaces(app: AppHandle) -> Result<Vec<NetInterface>>;
#[tauri::command] async fn dns_get(app: AppHandle, alias: String) -> Result<DnsState>;
// DnsState 内含 v4 / v6 两族各自的地址与是否 DHCP，分族返回以便界面独立展示
#[tauri::command] async fn dns_set(app: AppHandle, req: DnsSetRequest) -> Result<DnsApplyReport>;  // [需提权]
// DnsSetRequest { alias, v4: Option<Vec<Ipv4Addr>>, v6: Option<Vec<Ipv6Addr>> }
//   v4 = None → 该族不改动；Some(空 vec) → 该族还原为 DHCP 自动
#[tauri::command] async fn dns_reset(app: AppHandle, alias: String, family: Option<AddressFamily>) -> Result<()>;
// [需提权] family = None → 两族全部还原；Some(V4|V6) → 只还原该族（§4.3 交互规则 2）
#[tauri::command] async fn dns_flush(app: AppHandle) -> Result<()>;
#[tauri::command] async fn dns_presets(app: AppHandle, query: String) -> Result<Vec<DnsPreset>>;
// 内置 + 用户自定义清单，支持按名称/地区/特性筛选；DnsPreset 内含 v4/v6 地址列表

// ---- profile / 事务 ----
#[tauri::command] async fn profile_list(app: AppHandle) -> Result<Vec<Profile>>;
#[tauri::command] async fn profile_save(app: AppHandle, p: Profile) -> Result<()>;
#[tauri::command] async fn profile_apply(app: AppHandle, id: String) -> Result<ApplyReport>;
#[tauri::command] async fn rollback(app: AppHandle) -> Result<ApplyReport>;  // [视内容] 纯 hosts/DNS 变更才需提权
#[tauri::command] async fn snapshots_list(app: AppHandle) -> Result<Vec<SnapshotMeta>>;
#[tauri::command] async fn snapshots_diff(app: AppHandle, id: String) -> Result<DiffReport>;
#[tauri::command] async fn snapshots_open_dir(app: AppHandle) -> Result<()>;

// ---- 系统 ----
#[tauri::command] async fn autostart_set(app: AppHandle, enable: bool) -> Result<()>;
#[tauri::command] async fn system_open_backup(app: AppHandle) -> Result<()>;
#[tauri::command] async fn system_resolve(app: AppHandle, host: String) -> Result<ResolveResult>;
// 诊断：Resolve-DnsName + ping 结果，区分「命中 hosts」/「走 DNS」
```

`ApplyReport` 必须诚实返回每一项的实际结果，而不是一个笼统的成功标志：

```rust
pub struct ApplyReport {
    pub ok: bool,
    pub steps: Vec<ApplyStep>,
    pub rolled_back: bool,
    pub recovery_required: bool,   // true 时 UI 进入 Recovery 模式
}

pub struct ApplyStep {
    pub kind: String,          // "hosts" | "proxy" | "dns"
    pub applied: bool,
    pub verified: bool,        // 回读校验是否通过
    pub message: String,       // 人话解释，含原始错误码
}
```

---

## 9. 界面结构

```
┌──────────────────────────────────────────────────────────┐
│  速网 SuNet    [方案: 开发加速 ▾]               ● 已提权   │
├──────────────────────────────────────────────────────────┤
│                                                          │
│   ┌ 方案 ┐ ┌ Hosts ┐ ┌ 代理 ┐ ┌ DNS ┐ ┌ 日志 ┐          │
│   └──────┘ └───────┘ └──────┘ └─────┘ └──────┘          │
│                                                          │
│  （单标签页内容区）                                       │
│                                                          │
├──────────────────────────────────────────────────────────┤
│ hosts 42 条 · 代理 关闭 · DNS 自动(v4) 自动(v6)  [全局还原] │
└──────────────────────────────────────────────────────────┘
```

**状态栏的 DNS 段必须分族显示**：`DNS 自动(v4) 自动(v6)` 而不是笼统的 `DNS 系统默认`。只显示一个总状态会让用户误以为两族是一体的——而 §4.3 的整个交互设计都建立在「v4 / v6 独立可控」这个前提上。

**UI 硬性要求：**

- **状态栏常驻三态指示**（hosts / 代理 / DNS），任何时刻都能看出当前真实状态，不进标签页也能看到
- **深浅色跟随系统**，可手动覆盖。用 CSS 变量集中管理，所有颜色引用变量
- **危险操作二次确认**：删除方案、忽略崩溃恢复、清空 hosts 托管区块
- **"预览将要写入的 hosts 内容"** 在应用前展示，避免盲操作
- **DNS 面板的 v4 / v6 双勾选 + 写前预览 + 写后回读**，见 §4.3
- 界面语言简体中文；术语与 Windows 界面一致（"代理"不写"Proxy 代理"）

### 9.1 UI 设计规范（设计令牌）

界面只有 3 个表单页 + 1 个方案页，不需要组件库，但**必须有统一的令牌**，否则深浅色切换一定会露出"某些颜色没跟着变"的破绽。

**颜色（CSS 变量，两套值）**

```css
:root {
  color-scheme: light dark;
  --bg:        #f8fafc;   /* 页面底 */
  --bg-elev:   #ffffff;   /* 卡片 / 表格 */
  --bg-sunken: #f1f5f9;   /* 输入框 / 代码块 */
  --fg:        #0f172a;   /* 主文字 */
  --fg-muted:  #64748b;   /* 次要说明 */
  --fg-faint:  #94a3b8;   /* 占位符 */
  --border:    #e2e8f0;
  --accent:    #EF9F27;   /* 与图标中层同色，品牌一致 */
  --accent-fg: #1c1917;   /* 琥珀底上用深色文字，保证对比度 */
  --danger:    #dc2626;
  --warning:   #d97706;
  --success:   #16a34a;
}
[data-theme="dark"] {
  --bg:        #0f172a;
  --bg-elev:   #1e293b;
  --bg-sunken: #172033;
  --fg:        #e2e8f0;
  --fg-muted:  #94a3b8;
  --fg-faint:  #64748b;
  --border:    #334155;
  --accent:    #F0A93A;   /* 深色下略提亮，维持对比 */
  --accent-fg: #1c1917;
  --danger:    #f87171;
  --warning:   #fbbf24;
  --success:   #4ade80;
}
```

**硬性规则：**

- 所有颜色**只能引用变量**。任何一处硬编码十六进制色，都会在深色模式里变成"白底白字"或"黑底黑字"
- 在 `<html>` 上写 `data-theme` 并声明 `color-scheme`，让滚动条、表单控件、日期选择器等原生元素一起切换
- 主题三态：`follow_system`（默认）/ `light` / `dark`；切换按钮用**太阳/月亮图标**而非文字；选择写入 `localStorage`
- 选 `follow_system` 时监听 `matchMedia('(prefers-color-scheme: dark)')` 的 `change` 事件
- **禁止纯黑 `#000` 底 + 纯白 `#fff` 字的极端对比**；深色底用低饱和深蓝灰

**排版与尺寸**

| 项 | 值 |
|---|---|
| 字体栈 | `system-ui, "Segoe UI Variable Text", "Microsoft YaHei UI", "微软雅黑", sans-serif` |
| 等宽（hosts 编辑器 / 日志 / 地址） | `"Cascadia Mono", Consolas, "SF Mono", monospace` |
| 基础字号 / 行高 | 14px / 1.6 |
| 标题 | 区块 16px 600；页面 20px 600 |
| 数字与 IP 地址 | **一律等宽**，避免选中时字宽跳动，也便于逐字符核对 |
| 间距基准 | 4px 网格（4 / 8 / 12 / 16 / 24 / 32） |
| 圆角 | 控件 6px；卡片 8px；弹层 10px |
| 动效 | 120–180ms `ease-out`；`prefers-reduced-motion: reduce` 时全部关闭 |
| 主窗口 默认 / 最小 | 880×620 / 720×520 |

**交互约束：**

- 应用内导航一律**原生跳转**，不用 `target="_blank"` 开外部浏览器
- 危险操作（删除方案、忽略崩溃恢复、清空托管区块）**二次确认**；确认按钮用 `--danger`，且默认焦点**不落在**危险按钮上
- 写前预览（hosts 区块全文、DNS 分族预览）是**必做项**，不是可选项
- 所有失败提示必须带**原始错误码**（§14.3），不允许只给一句"操作失败"

---


---

## 10. 安全考量

| 项 | 处置 |
|---|---|
| 篡改风险 | 所有写 hosts / 注册表 / DNS 的操作都在**用户显式触发**时发生，无任何后台自动行为 |
| 静默变更 | 每次变更产生 `ApplyReport` 并记入日志；托盘 tooltip 显示当前状态，可随时看到"谁改的" |
| 崩溃残留 | §6.3 崩溃恢复检测 + 快照文件，异常退出后配置不会锁死在"代理开着"状态 |
| 提权面 | **提权子进程不加载 WebView2**（进程分流在 `main()` 最前面）——浏览器渲染引擎的漏洞面不会被提到管理员权限 |
| 提权状态 | 无常驻提权进程；提权子进程完成任务立即退出，不形成长期高危进程 |
| 日志 | `%APPDATA%\SuNet\logs\` 按天切分，默认保留 7 天；日志中代理地址等敏感项脱敏 |
| 网络请求 | 仅远程订阅需要出网，域名白名单化；不上报任何遥测 |
| 卸载 | 卸载时**明确询问**是否还原 hosts / 代理 / DNS。默认还原——留下一个开着的全局代理是严重后果 |
| **远程订阅内容** | 订阅源是**不可信输入**：写进 hosts 的 IP 直接决定域名解析结果，一个被劫持的源可以把银行域名指到攻击者 IP。见 §10.1 |
| 提权 IPC 载荷 | 载荷文件含随机 uuid、子进程先校验属主与 ACL、用完即删、只做固定分派不做动态求值（§1.4.10） |
| 跨进程写入 | hosts/DNS 临界区由命名互斥体保护，防止主进程与提权子进程互相踩踏（§7.2） |

**卸载时必须还原**是很多同类工具的缺失环节，也是最容易造成用户损失的地方。

### 10.1 远程订阅内容的校验规则（不可省略）

**订阅源是不可信输入。** 这与"从公共仓库直接装包"是同一类风险：内容会被写进一个决定系统解析行为的文件里。校验必须逐条做，不能只做"能不能解析成行"：

| 规则 | 说明 |
|---|---|
| 仅接受 `https://` | 拒绝 `http://`、`file://`、`ftp://`；禁止 302 跳到其他协议 |
| 逐行语法校验 | 只接受 `IP 域名 [域名…] [# 注释]`；拒绝通配符 `*`、拒绝正则、拒绝含控制字符的行、拒绝单行 > 2048 字符 |
| IP 合法性 | 必须能被 `IpAddr::from_str` 解析；拒绝 `0.0.0.0` 之外的特殊地址（除非用户显式启用批量屏蔽，§2.5） |
| 数量上限 | 单源 ≤ 5 000 条；超出报 `E6002` 并**整源拒绝**，不悄悄截断 |
| **变更需确认** | 首次导入，或本次变更条目数 > 总量 20% 时，展示「新增 / 修改 / 删除」三栏 diff，用户确认后才写入 |
| **敏感域名保护** | 内置一份关键域名清单（银行、支付、政务、邮箱、常见 CA），订阅若试图**修改**这些域名 → 高亮为危险变更，默认不勾选，需逐条确认 |
| 失败不清空 | 源不可达时保留上一次内容并标注同步时间（§2.6 已约定，此处作为安全规则重申） |

**为什么"变更需确认"不是打扰：** hosts 是静默生效的，用户不会收到任何提示就完成了一次解析重定向。在订阅这个场景里，唯一能拦住恶意变更的时机，就是"写入前让用户看一眼 diff"。

---

## 11. 实施路线

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| **M0 骨架** | Tauri 2 + 托盘 + 单实例 + asInvoker 主进程 + `--task` 提权分流 + 计划任务自启 | 启动无 UAC；关闭按钮隐藏到托盘；开机静默自启无 UAC；重复启动不产生第二实例；`--task 测试` 能产生一次 UAC 并执行任务；`Ctrl+Alt+S` 在后台能清除代理且无 UAC；hotkey 被占用时 UI 如实报错而非静默失败；提权任务通过文件交换回传 JSON 结果，超时能被中断；命名锁在持有者被杀后可接管（`WAIT_ABANDONED`） |
| **M1 Hosts** | 托管区块 + 原子写 + 编码处理 + `DnsFlushResolverCache` + 回读校验 | 反复切换 100 次，hosts 文件不膨胀、中文注释不乱码、条目正确生效（`ping` 验证，非 `nslookup`） |
| **M2 Proxy** | 注册表读写 + `InternetSetOptionW` 通知 + 快照还原 + 彻底清零 + 5 秒撤销 + 全局热键 | Chrome 未重启即生效；关闭后原 PAC 配置完整还原；代理进程未启动时拒绝开启并提示；快捷键在彻底清零下不残留 `AutoConfigURL`；5 秒内撤销能完全恢复；热键在主窗口隐藏（无焦点窗口）时仍可用 |
| **M3 DNS** | 网卡枚举 + 分族设/还原 + 缓存清理 + 组策略回读检测 + 约 40 项内置 v4/v6 清单 + 测速 | 切 DNS 后 `Resolve-DnsName` 返回新服务器结果；v6 取消勾选后确为 DHCP；还原后为 DHCP；虚拟网卡不出现在列表；清单中每条地址实测可响应 |
| **M4 方案** | Profile 持久化 + 事务式切换 + 逆序回滚 + Recovery | 三个步骤中任一注入失败，都能正确回滚到一致状态，无中间态残留；回滚自身失败时进入 Recovery 并展示差异清单；apply / rollback 各自幂等 |
| **M5 增强** | 远程订阅 + 自启计划任务 + 崩溃恢复 + 日志 | 主源不可达时自动切镜像；订阅拉取失败不清空本地；恶意订阅内容被逐条拒绝（`E6002`）；首次导入必须经过 diff 确认 |
| **M6 打包** | NSIS perMachine 安装器 + 卸载时还原 + 代码签名 | 干净 VM 上安装/卸载全流程；卸载后系统网络状态与安装前一致 |

**M0–M3 是可用版本**，M4 才是产品的完整形态，M5/M6 可以延后。

### 11.1 性能与资源目标

可测的指标，M6 验收：

| 指标 | 目标 | 说明 |
|---|---|---|
| 冷启动到托盘可用（SSD） | ≤ 800 ms | 主进程不加载任何网络操作，托盘先于窗口就绪 |
| 主窗口首次显示 | ≤ 1.2 s | WebView2 初始化放到后台预热 |
| 空闲内存占用 | ≤ 60 MB | 主进程 + WebView2 合计 |
| 空闲 CPU | ≈ 0% | 无轮询；订阅同步走定时器，不忙等 |
| 安装包体积 | ≤ 8 MB | NSIS 压缩后 |
| 切换方案（三层，无 UAC） | ≤ 2 s | 不含系统 UAC 对话框的停留时间 |
| 单次 hosts 写入（≤ 5 000 条） | ≤ 150 ms | 含原子替换与缓存刷新 |
| 代理开关（含前置探测） | ≤ 1.2 s | 探测超时 800 ms 是上限 |
| 提权子进程往返 | ≤ 1.5 s | 不含用户点击 UAC 的停留时间 |

**三条不能为达标而牺牲的原则：** 不砍 §3.4 的连通性探测；不砍 §2.2 的原子替换（改直写）；不放弃 `WAIT_ABANDONED` 之类的异常分支处理。

---


---

## 12. 已识别风险清单

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | 杀软/安全软件锁定 hosts 文件 | 写入失败或被自动还原 | 写前回读校验；失败时明确报出，提示临时关闭防护；**不尝试绕过防护** |
| 2 | 浏览器 DoH / 安全 DNS 绕过系统设置 | 改了 DNS/hosts 但 Chrome 无效 | 界面明示；提供"检测浏览器 DoH"诊断项 |
| 3 | 代理开启但代理软件未运行 | 全局断网 | 前置 TCP 探测 + 开启后连通性复检 + 可选自动回滚 |
| 4 | 提权子进程未分流，意外进入完整 Tauri 启动 | 提权状态下加载 WebView2 | 分流放在 `main()` 第一行；验证提权子进程的二进制不含 WebView2 依赖 |
| 5 | 用 `CreateProcess` 而非 `runas` 触发提权 | 静默返回 740 错误，用户误以为权限紧缩 | 统一用 `ShellExecuteW` + `runas`，M0 阶段实机验证 |
| 5a | 代理开关时意外弹 UAC | 干扰 | 提权前先自绘说明对话框，再发起 `runas` |
| 5b | 全局热键被其他软件占用（微信/QQ/截图工具）| 按无反应，用户误以为功能坏了 | 首次注册前探测（试探性注册 + 立即释放）；失败时报 `E5001` 而非静默失败；双入口（托盘菜单 + 主界面）保证功能不被热键锁死 |
| 5c | 只写 `ProxyEnable=0` 但遗留 `AutoConfigURL` | PAC 仍接管，界面显示已关闭实际未关 | 清除必须同时删除 `AutoConfigURL`（§5.5.6） |
| 5d | 快捷键误触导致断网 | 用户正在用代理时误压 | 5 秒撤销窗口（内存暂存）；托盘菜单同时提供相同功能 |
| 5e | `WM_HOTKEY` 在键下与释放时各投递一次 | 一次按键执行两遍 | handler 中 `if event.state() != Pressed { return }` |
| 6 | 企业组策略锁定 DNS/DNS 设置 | 改了被刷回 | 写后回读，不一致时明确报告而非假装成功 |
| 7 | Chromium 应用独立 DNS 缓存 | hosts 改了浏览器仍走旧记录 | 提供"清理浏览器 DNS 缓存"指引（`chrome://net-internals/#dns`） |
| 8 | 卸载后残留代理/hosts | 用户网络长期异常 | 卸载流程强制询问还原 |
| 9 | IPv6 优先导致 hosts 的 IPv4 条目被绕开 | 解析到 AAAA 记录 | 双栈条目支持；诊断命令中区分 `-4` |
| 9a | **写了 IPv6 DNS 但本机没有可用 IPv6 通路** | 部分域名解析超时/变慢，看起来像"改了 DNS 反而坏了" | v6 默认不勾选；写入前检测网卡是否真有可用 IPv6 地址（`Get-NetIPAddress -AddressFamily IPv6` 有无全局地址），无则提示「当前网卡无 IPv6 通路，写入 IPv6 DNS 只会导致解析变慢」 |
| 9b | **取消 IPv6 勾选后残留上次写入的 IPv6 地址** | 用户以为已还原，实际仍走手动 IPv6 DNS | 取消勾选 = 该族调 `-ResetServerAddresses`，不是"保持原值"（§4.3 规则 2） |
| 9c | 第三方汇总站点的 IPv6 地址与官方不符 | 写入后 DNS 完全不可用 | 内置清单以官网为准（AdGuard 已踩过：`2a00:5a60::` 系错误，正确为 `2a10:50c0::`）；提供「测速」按钮，写入前先探测该地址是否响应 |
| 9d | 部分服务无 IPv6 地址 | 用户以为漏了 | 清单中显式标「无 IPv6」，不静默留空 |
| 10 | 多实例并发写配置 | 状态互相覆盖 | 全局互斥锁 + `single-instance` 插件 |
| 11 | `windows` crate 版本与 `netioapi.h` 不匹配 | C 方案受阻 | v1 用 PowerShell 方案，已预留接口可替换 |
| 12 | 未做代码签名 | SmartScreen 拦截、体积信任问题 | 发布前签名；v1 自用阶段手动放行 |

---

## 13. 与参考项目的关系

**SteamHostSync（Clov614/SteamHostSync）** 的定位是第①层专用工具：Go 编写，从远程仓库拉取预生成的 hosts，写入本地，建议搭配 SwitchHosts 做多方案与自动更新。

| 维度 | SteamHostSync | 速网 SuNet |
|---|---|---|
| 覆盖层 | 仅 Hosts | Hosts + Proxy + DNS |
| 方案管理 | 靠外部 SwitchHosts | 内置原子方案 |
| 远程订阅 | ✅ 内置 | ✅ 内置（多源 + 镜像 + 失败不清空） |
| 写 hosts 方式 | 整文件覆盖 | 托管区块 + 原子替换 + 编码兜底 |
| 系统代理 | ❌ | ✅ |
| 系统 DNS | ❌ | ✅ IPv4 + IPv6，约 40 项内置清单 |
| 失败回滚 | ❌ | ✅ 事务式 |
| 托盘常驻 | ❌ | ✅ |
| 技术栈 | Go CLI | Rust + Tauri 2 GUI |

**可以直接引用的公开资源**：SteamHostSync 的预生成 hosts 源（`Hosts` / `Hosts_steam` / `Hosts_github` 等）与其 jsDelivr / Statically 双源分发思路，本方案的 `sources` 结构与其兼容，可直接导入作为首个订阅源验证链路。

---

## 14. 日志、诊断与错误处理

### 14.1 日志

`%APPDATA%\SuNet\logs\sunet-YYYY-MM-DD.log`，按天切分，默认保留 7 天（`settings.log_keep_days`）。

**级别**：`error` / `warn` / `info` / `debug`，默认 `info`。设置里可切 `debug`（会记录完整载荷，调试后建议切回）。

**必记事件（不允许省略）：**

| 事件 | 级别 | 记录内容 |
|---|---|---|
| hosts 写入 | info | 条目数、写入前/后文件哈希、是否走了回退路径、耗时 |
| 代理变更 | info | 变更前后 `ProxyEnable` / `ProxyServer` / `AutoConfigURL`（地址脱敏）、是否探测、探测结果 |
| 代理彻底清零 | info | 清除前的完整值（脱敏）、是否被撤销、撤销时间 |
| DNS 变更 | info | 网卡别名、地址族、写入地址（脱敏）、回读结果 |
| 提权任务 | info | task 名、退出码、耗时；只记载荷/结果**文件路径**，不记内容 |
| 热键注册 / 释放 | info | 组合、成功或失败、错误码 |
| 订阅同步 | info | 源、HTTP 状态、条目数变化、是否采用 |
| 回滚 / Recovery | warn | 失败步骤、原始错误码、回滚结果 |
| 崩溃恢复检测 | warn | 检测到的 marker 内容与对应快照 id |

**脱敏规则**（默认开启，`settings.log_redact = true`）：

- 代理地址：**保留端口**，主机部分打码为 `***`（端口不脱敏，排查时常常要用它）
- hosts / DNS 中的**内网地址**打码（`10.` / `172.16-31.` / `192.168.` / link-local / `.local` / `.lan`）
- 用户名与用户目录路径：`C:\Users\<实际用户名>\` → `C:\Users\<user>\`
- 订阅 URL 的 query 段（部分私有源会带凭据）：整段打码

### 14.2 导出诊断包

主界面「帮助 → 导出诊断包」，生成一个 zip：

```
diagnostic-<timestamp>.zip
├─ logs/                 最近 3 天日志（按脱敏设置处理）
├─ state.json            当前三层状态 + 方案列表（脱敏）
├─ environment.json      OS 版本号、Tauri / Rust 版本、工作目录、是否提权、
│                        网卡列表（名称 + 地址族，不含 IP 细节）
├─ config.schema.json    当前配置的 schema 版本（便于判断迁移问题）
└─ README.txt            说明包含什么、已脱敏什么
```

**默认脱敏后导出**，且导出前把将要包含的内容清单给用户看一遍——避免用户不知情地把内网拓扑贴到公开 issue 里。

### 14.3 错误码与错误处理约定

错误码分六段，**用户可见文案必须带码**，让用户求助时一句话说清问题：

| 段 | 范围 | 示例 |
|---|---|---|
| `E1xxx` | 权限与提权 | `E1001` 用户取消了 UAC；`E1002` 提权后仍被拒（组策略 / ACL）；`E1003` 提权任务超时；`E1004` 载荷校验失败 |
| `E2xxx` | hosts | `E2001` 文件被占用；`E2002` 编码无法识别（拒绝写入）；`E2003` 托管标记损坏（有 BEGIN 无 END）；`E2004` 回读校验不一致 |
| `E3xxx` | 代理 | `E3001` 前置探测失败（拒绝开启）；`E3002` 开启后外网不通；`E3003` 注册表写入被拒；`E3004` 通知失败（**降级为 warning，不中断事务**） |
| `E4xxx` | DNS | `E4001` 网卡不存在 / 已断开；`E4002` 地址格式非法；`E4003` 组策略锁定（写后回读不一致）；`E4004` 当前网卡无 IPv6 通路 |
| `E5xxx` | 热键 | `E5001` 组合已被占用（1409）；`E5002` 组合被系统保留；`E5003` 被安全软件拦截 |
| `E6xxx` | 订阅 | `E6001` 主源与镜像均不可达（**保留本地，不清空**）；`E6002` 内容校验失败；`E6003` 条目数超上限 |

**三条处理约定：**

1. **能降级的降级，不要中断事务。** 典型是 `E3004`——注册表已经写成功，功能上生效，只是即时性打折，此时写回原值反而更糟，记 warning 继续。
2. **不能假装的绝不假装。** 回读校验不一致（`E2004` / `E4003`）必须报失败，不允许"写入调用没抛异常"就当成成功。
3. **区分"权限不足"与"功能被占用"。** `E5xxx` 与 `E1xxx` 最容易混为一谈——热键注册失败与权限毫无关系，提示里**不能**写"请以管理员身份运行"（§5.5.11 已强调）。

---

## 15. 首次运行、配置迁移与版本策略

### 15.1 首次运行（不弹向导，也不抓「原始设置」）

首次启动**不弹任何引导**，直接进主界面。默认基线就是干净的直连状态：
不写 hosts 托管区块、不开系统代理、不改 DNS；要什么由用户主动开启。

> **2026-10-08 用户决策（覆盖本节旧版）**：原先的「① 检测当前系统状态　→　② 提示不会动你没主动点过的东西
> →　③ 把当前状态存成方案 + 静默写入基线快照」整段引导已移除。
> 理由：默认什么都没做，那份"接管前"方案与快照没有实际价值，只是给首次启动多加一步；
> "不会主动改系统"的承诺由内置「默认 · 直连」方案 + 任何写入都必须用户主动点击来保证。
> 现状：首启只写一个 `initialized = true` 标记（`cmd/system.rs::first_run_finish`），
> 不再枚举网卡（旧的 `first_run_report` 命令已删除）、不建方案、不写快照。

### 15.2 配置迁移与版本策略

`config.json` 带 `schema_version`。升级时：

```
读取 config.json
  ├─ schema_version == 当前版本 → 直接用
  ├─ schema_version < 当前版本  → 逐级执行迁移函数 v1→v2→v3…（不跳级）
  │     · 迁移前先备份为 config.v<n>.bak.json（与快照目录分开存放）
  │     · 迁移失败 → 保留原文件，以只读模式启动并明确提示（绝不清空用户配置）
  └─ schema_version > 当前版本  → 用户装过更高版本
        · 拒绝写入，以只读模式启动，提示"配置文件由更新版本创建"
        · 绝不做"向下兼容猜测"——猜错的代价是用户丢失全部方案
```

**三条原则：**

- 迁移函数必须**纯函数、可单元测试**（输入旧 JSON，输出新 JSON），不依赖系统状态
- 迁移**只增不减**：宁可留下废弃字段，也不要在迁移里删用户数据
- 内置 DNS 清单**不进配置文件**（§7.1），因此修正地址不需要任何迁移——这是当初把清单编译进二进制换来的直接收益

### 15.3 版本号与更新

- 程序版本语义化（`MAJOR.MINOR.PATCH`）；`schema_version` 与程序版本**解耦**，只在配置结构真的变了时才 +1
- v1 不做自动更新（无签名分发渠道），只提供「检查更新」跳转 Releases 页
- 保留占位字段 `"channels": []`，避免日后加更新通道时又要做一次迁移

---

## 16. 测试策略与验收矩阵

### 16.1 单元测试（纯逻辑，CI 必过）

这些不碰系统，必须在 CI 里跑：

| 目标 | 用例要点 |
|---|---|
| 编码探测与解码（§2.3） | UTF-8 带/不带 BOM、UTF-16LE BOM、GBK 中文注释、混合乱码 → 断言"要么正确解码、要么明确拒绝" |
| 托管区块解析 / 生成（§2.4） | **幂等性**：解析 → 生成 → 再解析，结果逐字节相等；有 BEGIN 无 END → 报 `E2003` 而不是猜 |
| 条目校验（§2.5） | 非法 IPv4 / IPv6、超长域名、通配符、重复 hostname |
| 快照序列化与 diff（§6.2） | 往返序列化等价；`hosts_raw` 保留原始字节 |
| Profile 展开（§7.1） | `preset_id` 与展开值的一致性；`v6` 空数组 = DHCP 的语义 |
| 配置迁移（§15.2） | 每个 `v(n)→v(n+1)` 函数，旧样本进、新样本出 |
| 提权载荷编解码（§1.4.10） | 超长载荷走文件分支；损坏 JSON → `E1004` |
| 订阅内容校验（§10.1） | 恶意行（通配、非法 IP、控制字符、超量）逐条拒绝 |

### 16.2 破坏性测试（回滚是核心卖点，必须专门测）

§6 的事务式回滚如果没测过，等于没有。做法是在 `apply` 的每个步骤后**注入强制失败**：

| 注入点 | 期望结果 |
|---|---|
| hosts 写成功后、proxy 前失败 | 逆序回滚：hosts 还原为原始字节 |
| proxy 写成功后、dns 前失败 | 回滚：dns 未动、proxy 还原 |
| dns 写成功后、verify 失败 | 回滚：三层全部还原 |
| 回滚过程中再次注入失败 | 进入 Recovery，展示差异清单 + 备份目录入口，**不静默吞掉** |
| 提权子进程在写入中途被 `TerminateProcess` | 主进程超时分支触发；命名锁进入 `WAIT_ABANDONED`；下次启动崩溃恢复检测命中 |

### 16.3 实机验收矩阵

自动化测不到的部分，按下表人工过一遍，**每格至少一次**：

| 维度 | 取值 |
|---|---|
| 系统 | Windows 10 21H2、Windows 11 22H2、Windows 11 最新 |
| 账户 | 管理员账户、**标准用户账户**（验证 §1.4.6 的 HKCU 正确性） |
| 域 | 非域、加入域（验证组策略场景 `E4003`） |
| 安全软件 | 无、Windows Defender、第三方杀软（验证 §2.2 回退路径与 `E2001`） |
| 网络 | 单一物理网卡、多网卡（含 VMware / Hyper-V / WSL / TUN 虚拟网卡） |
| 地址族 | 纯 IPv4、双栈、纯 IPv6（验证 §4.3 的 `E4004` 提示） |
| 输入法 | 中文输入法下热键是否被吞（§5.5.9） |

**必测的五个交叉场景（最容易出问题的地方）：**

1. 标准用户 + 含 hosts/DNS 的方案 → 代理静默生效、hosts/DNS 提权一次、`HKCU` 归属正确
2. 企业 PAC 环境按 `Ctrl+Alt+S` → 彻底清零（含删 `AutoConfigURL`）→ 5 秒内撤销 → 逐项还原
3. 热键已被微信占用 → 注册失败必须报 `E5001`，且托盘菜单的「清除代理」仍可用
4. hosts 被杀软锁定 → 报 `E2001` 且**不留下半截文件**
5. 切换方案过程中拔网线 → 代理探测失败应触发回滚，而不是留下中间态

### 16.4 幂等性与稳定性

- **apply 幂等**：连续 apply 同一方案两次，第二次的 `steps` 应全部 `applied=true, verified=true`，且 hosts 文件内容不变
- **rollback 幂等**：连续 rollback 两次不报错，第二次为空操作
- **hosts 膨胀测试**：切换 100 次后文件行数与首次切换后一致（§11 M1 验收的自动化版本）


---

## 附录 A：参考来源

| 结论 | 来源 |
|---|---|
| SteamHostSync 功能与用法 | https://github.com/Clov614/SteamHostSync · https://blog.gitcode.com/16f1034ad7e7cbabc36c61abde532909.html |
| hosts 源与 jsDelivr/Statically 镜像 | https://gitee.com/jesse51076008/SteamHostSync · https://git.bytes.zone/Clov614/SteamHostSync |
| WinINET 注册表键值语义 | https://learn.microsoft.com/en-us/windows/win32/wininet/option-flags |
| `INTERNET_OPTION_SETTINGS_CHANGED`(39) / `REFRESH`(37) | https://learn.microsoft.com/de-DE/windows/win32/WinInet/option-flags · https://stackoverflow.com/questions/31348111 |
| `INTERNET_PER_CONN_OPTION_LIST` 完整写法 | https://stackoverflow.com/questions/31348111/setting-proxy-settings-in-windows-with-python |
| hosts 需管理员权限（KB 923947） | https://support.microsoft.com/kb/923947/zh-cn · https://learn.microsoft.com/en-ca/troubleshoot/windows-server/networking/cannot-modify-hosts-lmhosts-files |
| hosts 编码/换行/flushdns 要求 | https://eztoolset.com/?p=95 · https://www.renrendaima.com/article/16584.html · https://www.devcrea.com/edit-hosts-file-windows-10-11 |
| `nslookup` 不读 hosts | https://www.renrendaima.com/article/16584.html |
| DNS PowerShell cmdlet 对照 | https://www.cnblogs.com/suv789/p/18641235 · https://www.aiboce.com/ask/243755.html |
| 阿里 AliDNS 官方 v4/v6 地址 | http://www.alidns.com · https://www.alibabacloud.com/help/tc/dns/free-service-disclaimer · https://static-aliyun-doc.oss-cn-hangzhou.aliyuncs.com/download%2Fpdf/171501/产品简介_cn_zh-CN.pdf |
| AdGuard 官方 v4/v6 地址（三个版本） | https://adguard-dns.io/zh_cn/public-dns.html |
| 公共解析器 v4/v6 汇总（DNS Privacy Project） | https://dnsprivacy.org/public_resolvers/ |
| 公共 DNS 完整清单（含 IPv6） | https://testsdns.com/public-dns-servers · https://dnsspeedtester.com/best |
| 国内公共 DNS 汇总（OneDNS / 114 / 360 等） | https://www.31du.cn/blog/gaobiednsjiechiguobing.html · https://blog.lufei.de/p/公共dns汇总/ |
| 国内 + 国外 IPv6 地址专项汇总 | https://www.vvars.com/package-tool/free-Public-IPv6-DNS-Server-Addresses-in-China.html |
| 延迟实测对比（选 DNS 时的参考） | https://dnsgratis.com/en/comparison |
| Windows 11 DoH 配置 | https://kak.kornev-online.net/FILES/（DoH 部署文档） · https://www.bbccb.com/html/20260826/45586.html |
| Cisco NetBox（命名避让对象） | https://github.com/netbox-community/netbox |
| 任务栏深/浅主题检测（`AppsUseLightTheme`）| https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/apply-color-theme-to-your-app |
| `RegisterHotKey` 与 `ERROR_HOTKEY_ALREADY_REGISTERED`(1409) | https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey · https://ask.csdn.net/questions/8873110 |
| 全局热键冲突排查与常见占用进程 | https://ask.csdn.net/questions/9021562 · https://blog.csdn.net/weixin_32288959/article/details/165066562 |
| Windows 系统保留短捷键（Win 键类）| https://www.positioniseverything.net/how-to-set-hotkeys-in-windows-11 · https://pcnmobile.com/?p=129205/ |
| `tauri-plugin-global-shortcut` API | https://v2.tauri.app/plugin/global-shortcut/ · https://github.com/tauri-apps/plugins-workspace |
| 低级键盘钩子（`WH_KEYBOARD_LL`）的局限 | https://blog.csdn.net/weixin_32288959/article/details/165066562 |
| PowerToys Keyboard Manager 用于排查热键占用 | https://www.ufcn.cn/games/1863925 |
| Tauri 2 系统托盘 API | https://v2.tauri.app/learn/system-tray/ · https://tauri.app/zh-cn/learn/system-tray |
| 托盘图标需 `include_bytes!` 内嵌 | https://juejin.cn/post/7433967022188986380 |
| `tauri-plugin-autostart`（Run 键实现） | https://tauri.ubitools.com/plugin/autostart · https://lib.rs/crates/tauri-plugin-autostart |
| hosts 文件 ACL（Administrators F / Users RX）| https://blog.csdn.net/wenxuankeji/article/details/164064806 |
| HKCU 用户进程默认可写 | https://www.thewindowsclub.com/edit-registry-another-user-windows-10/ · https://www.advancedinstaller.com/application-packaging-training/msi/ebook/registries.html |
| 非提权改 DNS 返回 0x80070005 | https://ask.csdn.net/questions/9362323 |
| `CreateProcess` 不触发 UAC（740 错误）| https://learn.microsoft.com/en-nz/answers/questions/5829645/a-shortcut-run-from-tasksch-with-highest-privilege |
| 计划任务 `Run with highest privileges` 的 UAC 行为 | https://learn.microsoft.com/zh-cn/previous-versions/windows/it-pro/windows-7/cc722152(v=ws.10) · https://ss64.com/nt/schtasks.html |
| Session 0 隔离与交互式任务 | https://www.positioniseverything.net/fix-scheduled-task-does-not-start-at-logon-of-any-user-or-runs-in-background-in-windows-10-solved/ |
| 命名管道跨权限 DACL 设置 | https://learn.microsoft.com/zh-tw/windows/desktop/ipc/named-pipe-security-and-access-rights |
| hosts 永久放权（`icacls /grant Users:F`）的做法与问题 | https://www.ufcn.cn/article/2161453.html · https://www.php.cn/faq/2985195.html |
| UAC `requestedExecutionLevel` 语义与影响 | https://developer.cloud.tencent.com/article/2348800 · https://blog.walterlv.com/post/requested-execution-level-of-application-manifest.html |
| NSIS 安装模式与权限 | http://tauri.net.cn/254.html |
| `ShellExecuteEx` + `SEE_MASK_NOCLOSEPROCESS`（等待提权进程并取退出码）| https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecuteexw |
| UAC 被取消返回 `ERROR_CANCELLED`(1223) | https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--1200-1299- |
| 命名互斥体与 `WAIT_ABANDONED`（持有者崩溃后内核自动释放）| https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createmutexw · https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject |
| 通知区域 tooltip 长度上限（`szTip` 128 字节）| https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw |
| 专注助手对气泡通知的抑制 | https://support.microsoft.com/zh-cn/windows/ |

## 附录 B：实现前需实机验证的三点

以下三点本方案基于文档与社区实测推断，**不作为已确认结论**，需在 M0 阶段实机验证：

1. **`InternetSetOptionW` 的通知作用域** —— 通知是进程级的还是会话级的。模型 C（§1.4）下代理写入始终发生在**非提权的主进程**里，前一版担心的"提权后 SID 不一致导致 HKCU 归属错乱"场景已不存在；剩下要确认的是：主进程发出通知后，同一会话内已在运行的浏览器（其子进程）能否立即重读配置，还是各自需要重建网络栈。
2. **`ReplaceFileW` 在本机杀软环境下的成功率** —— 部分安全软件会持有 hosts 文件句柄，需确认回退路径（截断写入）是否可靠。
3. **`DnsFlushResolverCache` 对 hosts 变更的生效范围** —— 是否所有应用层缓存都被清掉，或仍有 Chromium 系应用需要额外清 `chrome://net-internals/#dns`。