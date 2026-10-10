// 平台差异的唯一真源（前端）
//
// 界面上有十几处写着「UAC」「计划任务」「HKCU\...\Internet Settings」——
// 这些说法在 macOS 上全是错的。集中在这一处，改文案只改这一个文件。
//
// 判定优先读 <html data-os>：它由 public/os-mark.js（<head> 里的经典脚本，不是 module）
// 在**首次绘制之前**打好 —— 顶栏给交通灯让位、隐藏自绘窗口按钮都必须在首帧生效，
// 而本模块是 deferred 的，等到它执行时第一帧早就画完了。
// 那个脚本不在时（被删 / 页面被别的入口加载）回退 UA：WKWebView 在 macOS 上恒报
// "Macintosh; Intel Mac OS X ..."，WebView2 在 Windows 上报 "Windows NT ..."。
// 判定条件必须与 os-mark.js 一致 —— 改一处要改两处。

const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;

function detectMacos(): boolean {
  if (typeof document !== "undefined") {
    const marked = document.documentElement.dataset.os;
    if (marked) return marked === "macos";
  }
  return /Macintosh|Mac OS X/.test(ua);
}

export const IS_MACOS = detectMacos();

/** 平台名（日志 / 关于页） */
export const OS_NAME = IS_MACOS ? "macOS" : "Windows";

/** 授权框叫什么 */
export const AUTH_NAME = IS_MACOS ? "系统管理员授权框" : "UAC 授权框";

/** 一句话说明「要弹一次授权」 */
export const AUTH_HINT = IS_MACOS ? "会弹一次系统授权框" : "会弹一次 UAC";

/** 自绘说明对话框的正文（真正的授权框只显示程序名，用户会认不出） */
export const AUTH_BODY = IS_MACOS
  ? "接下来会弹出 macOS 的系统授权框（需要输入登录密码），请点击「好」继续。\n本次操作只需要一次授权，不会常驻管理员进程。"
  : "接下来会弹出 Windows 的 UAC 授权框（程序名显示为 SuNet.exe），请点击「是」继续。\n本次操作只需要一次授权，不会常驻管理员进程。";

export const AUTH_CONTINUE = IS_MACOS ? "继续（将弹出授权框）" : "继续（将弹出 UAC）";

/** 权限说明整句（顶栏权限卡片 / 设置页） */
export const PERM_HINT = IS_MACOS
  ? "代理开关需要写入系统网络设置（弹一次授权框）；hosts / DNS 同样只在<b>你主动点击</b>后弹一次授权框。"
  : "代理开关完全静默，无需管理员权限；只有修改 hosts / DNS 时才会在<b>你主动点击</b>后弹一次 UAC。";

/** 已是管理员时的徽章提示 */
export const ELEVATED_TITLE = IS_MACOS
  ? "当前进程具备管理员权限，hosts / DNS 修改无需再弹授权框"
  : "当前进程具备管理员权限，hosts / DNS 修改无需再弹 UAC";

export const NORMAL_TITLE = IS_MACOS
  ? "仅在你主动点击后弹一次系统授权框；不会常驻管理员进程"
  : "代理开关无需权限；仅在修改 hosts / DNS 时弹一次 UAC";

export const SWITCH_TOAST_HINT = IS_MACOS
  ? "含 hosts / DNS 的方案会弹一次授权框"
  : "含 hosts / DNS 的方案会弹出一次 UAC";

/** 开机自启的方式 */
export const AUTOSTART_LABEL = IS_MACOS ? "登录项" : "计划任务";
export const AUTOSTART_HINT = IS_MACOS
  ? "通过登录项（LaunchAgent）在登录后静默启动，不弹授权框"
  : "通过计划任务在登录后静默启动（/RL LIMITED，不弹 UAC）";

/** 代理一层的落点（macOS 写系统代理要 root，与 Windows 的 HKCU 完全不同） */
export const PROXY_NEEDS_ADMIN = IS_MACOS;
export const PROXY_TITLE = IS_MACOS ? "系统代理（网络设置里的 Web 代理）" : "系统代理（WinINET 全局代理）";
export const PROXY_HINT = IS_MACOS
  ? "对所有活动网络服务写入 <b>HTTP 代理</b>（networksetup）；只写这一项 —— HTTPS 请求由系统复用 HTTP 代理，多写 SOCKS / 安全 Web 反而会让只支持 HTTP 的代理端口收到错协议的请求。"
  : "写入 HKCU\\...\\Internet Settings 并调用 InternetSetOptionW 通知系统，<b>全程不需要管理员权限</b>。";
export const PROXY_SWITCH_NOTICE = IS_MACOS ? "即将开关系统代理（需要一次管理员授权）" : "即将开关系统代理（无需管理员权限）";

/** 代理页「为什么有的程序不跟随」的正文（各平台作用域完全不同） */
export const PROXY_SCOPE_WHY = IS_MACOS
  ? `系统代理写在各网络服务的 Web 代理（HTTP）设置里（由 configd 管理），
         Safari / 系统 WebKit 与大多数原生应用都会读它（HTTPS 请求也复用这一项）；
          而 <code>curl</code> / <code>git</code> / <code>npm</code> / Homebrew 等命令行工具默认不读系统代理
          （需要自己设 <code>http_proxy</code> 环境变量），App Store 与部分软件更新走独立通道。`
  : `系统代理位于 WinINET 层，Chrome / Edge / Electron 应用 / 大部分办公软件都会读它；
          而 Windows Update、部分系统服务走 WinHTTP（机器级，需 <code>netsh winhttp</code>），
          <code>curl</code> / <code>git</code> / <code>npm</code> 默认不读系统代理（Git 可配 <code>http.proxy</code>）。`;

/** 快照说明里的代理层（Windows 是注册表值，macOS 是各服务的代理设置） */
export const PROXY_SNAPSHOT_NAME = IS_MACOS ? "各网络服务的代理设置" : "代理注册表值";

/** 修饰键的书写名（录制提示用） */
export const MOD_NAMES = IS_MACOS ? "⌃ Control / ⌥ Option / ⇧ Shift / ⌘ Command" : "Ctrl / Alt / Shift / Win";

/**
 * 修饰键的显示符号。配置里永远存规范名（Ctrl / Alt / Shift / Super），
 * 只有界面显示成符号：macOS 上 Super 就是 Command（⌘）。
 */
export const MOD_GLYPH: Record<string, string> = IS_MACOS
  ? { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Super: "⌘" }
  : {};

/**
 * 在 <html> 上标记平台，供 CSS 做差异（隐藏自绘窗口按钮、给交通灯让位）。
 *
 * 正常情况下 public/os-mark.js 已经打过了，这里是兜底重写（幂等）：那个脚本被删、
 * 页面被别的入口加载时，界面也不会整块错位。
 */
export function markPlatform(): void {
  if (typeof document !== "undefined") {
    document.documentElement.dataset.os = IS_MACOS ? "macos" : "windows";
  }
}
