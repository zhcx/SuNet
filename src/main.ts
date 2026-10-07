// 速网 SuNet 前端入口：外壳、路由、状态栏、事件桥

import { listen } from "@tauri-apps/api/event";
import { api, errText, type AppStateView, type Settings } from "./api";
import * as ui from "./ui";
import { initTheme, onThemeChange } from "./theme";
import { icon } from "./icons";
import { renderHosts } from "./tabs/hosts";
import { renderProxy } from "./tabs/proxy";
import { renderDns } from "./tabs/dns";
import { renderProfiles } from "./tabs/profiles";
import { renderLogs } from "./tabs/logs";
import { openSettingsModal } from "./settings";

// 主窗口入口；快捷面板走 src/quick-main.ts（独立 HTML 入口）

export interface TabCtx {
  state: AppStateView;
  /** 重新拉取全局状态；deep=true 时同时重绘当前标签页 */
  refresh: (deep?: boolean) => Promise<void>;
  openSettings: () => void;
  navigate: (tab: string) => void;
}

export type TabRender = (root: HTMLElement, ctx: TabCtx) => Promise<void> | void;

type TabIcon = keyof typeof icon;

const TABS: { id: string; label: string; icon: TabIcon; render: TabRender }[] = [
  { id: "profiles", label: "方案", icon: "profile", render: renderProfiles },
  { id: "hosts", label: "Hosts", icon: "hosts", render: renderHosts },
  { id: "proxy", label: "代理", icon: "proxy", render: renderProxy },
  { id: "dns", label: "DNS", icon: "dns", render: renderDns },
  { id: "logs", label: "日志", icon: "logs", render: renderLogs },
];

const ic = (name: TabIcon, size = 15) => `<span class="i">${icon[name](size)}</span>`;

let state: AppStateView | null = null;
let activeTab = "profiles";

function h(id: string): HTMLElement {
  return document.getElementById(id) as HTMLElement;
}

async function refreshState(deep = false): Promise<void> {
  try {
    state = await api.getAppState();
    ui.setElevated(state.prand.is_elevated);
    renderHeader();
    renderFooter();
    if (deep) await renderActiveTab();
  } catch (e) {
    ui.toast("error", "无法读取程序状态", errText(e));
  }
}

function renderHeader(): void {
  if (!state) return;
  const badge = h("priv-badge");
  if (state.read_only) {
    badge.className = "badge err";
    badge.textContent = `只读模式 ${state.read_only.code}`;
    badge.title = state.read_only.message;
  } else if (state.prand.is_elevated) {
    badge.className = "badge ok";
    badge.textContent = "已提权";
    badge.title = "当前进程具备管理员权限，hosts / DNS 修改无需再弹 UAC";
  } else {
    badge.className = "badge plain";
    badge.textContent = "普通权限";
    badge.title = "代理开关无需权限；仅在修改 hosts / DNS 时弹一次 UAC";
  }

  const themeBtn = h("btn-theme");
  const settingsBtn = h("btn-settings");
  const minBtn = h("btn-min");
  const hideBtn = h("btn-hide");
  settingsBtn.innerHTML = ic("gear");
  settingsBtn.setAttribute("aria-label", "设置");
  // 主窗口是无边框窗口（tauri.conf.json `decorations: false`），系统标题栏的
  // 最小化/关闭按钮也随之消失，所以这两个按钮由界面自己承担：
  //   − 最小化（真正 minimize）  × 关闭窗口 = 隐藏到托盘（保留原有语义）
  minBtn.innerHTML = ic("minus");
  minBtn.setAttribute("aria-label", "最小化");
  hideBtn.innerHTML = ic("close");
  hideBtn.setAttribute("aria-label", "关闭窗口（隐藏到托盘）");
  themeBtn.setAttribute("aria-label", "切换主题");

  const select = h("active-profile") as HTMLSelectElement;
  const prev = select.value;
  select.innerHTML = "";
  // 不再放「未启用任何方案」这种空选项：没有活动方案时直接显示内置直连（它才是真实状态），
  // 而且空选项会让 select.value="" 把下面的「切换」按钮永久置灰。
  for (const p of state.profiles) {
    const o = document.createElement("option");
    o.value = p.id;
    o.textContent = p.is_builtin ? `${p.name}（内置）` : p.name;
    select.appendChild(o);
  }
  const wanted = state.active_profile_id ?? prev;
  const fallback = state.profiles.find((p) => p.is_builtin)?.id ?? state.profiles[0]?.id ?? "";
  select.value = state.profiles.some((p) => p.id === wanted) ? wanted : fallback;
  h("btn-apply-profile").toggleAttribute("disabled", !select.value || !!state.read_only);
}

function renderFooter(): void {
  if (!state) return;
  const t = state.tray;
  const hostsOn = state.hosts.block_present && state.hosts.enabled_entries > 0;
  const proxyOn = state.proxy.enable && !state.proxy.pac_present;

  const hosts = h("status-hosts");
  hosts.innerHTML = `${ic("hosts", 14)}hosts ${state.hosts.enabled_entries}${
    state.hosts.block_present ? "" : "（未写入）"
  }`;
  hosts.className = `status-item ${hostsOn ? "on" : "faint"}`;
  hosts.onclick = () => void navigate("hosts");

  const proxy = h("status-proxy");
  proxy.innerHTML = `${ic("proxy", 14)}${
    proxyOn
      ? `代理 ${state.proxy.server || "已开启"}`
      : state.proxy.pac_present
        ? "代理 PAC 接管"
        : "代理 关闭"
  }`;
  proxy.className = `status-item ${proxyOn ? "on" : state.proxy.pac_present ? "warn" : "faint"}`;
  proxy.onclick = () => void navigate("proxy");

  const dns = h("status-dns");
  dns.innerHTML = `${ic("dns", 14)}${t.dns_v4} · ${t.dns_v6}`;
  dns.className = "status-item";
  dns.onclick = () => void navigate("dns");

  const lock = h("status-lock");
  lock.hidden = !state.runtime.lock_contended;
  lock.innerHTML = `${ic("warn", 14)}另一个 SuNet 进程正在写入 hosts/DNS`;

  const restore = h("btn-global-restore") as HTMLButtonElement;
  restore.disabled = !!state.read_only;
}

async function renderActiveTab(): Promise<void> {
  // 启动初期的竞态：状态还没回来时点标签，不能再静默吞掉（否则表现为"点了没反应"），
  // 等一次状态刷新后继续渲染
  if (!state) {
    await refreshState();
    if (!state) return;
  }
  const view = h("view");
  const tab = TABS.find((t) => t.id === activeTab) ?? TABS[0];
  view.innerHTML = `<div class="empty">加载中…</div>`;
  const ctx: TabCtx = {
    state,
    refresh: refreshState,
    openSettings: openSettingsModal,
    navigate: (t) => navigate(t),
  };
  try {
    await tab.render(view, ctx);
  } catch (e) {
    view.innerHTML = `<div class="card"><h2>加载失败</h2><div class="error-text">${ui.esc(
      errText(e),
    )}</div></div>`;
  }
  renderTabs();
}

function renderTabs(): void {
  const nav = h("tabs");
  ui.clear(nav);
  for (const t of TABS) {
    const btn = document.createElement("button");
    btn.innerHTML = `${ic(t.icon)}<span>${t.label}</span>`;
    btn.className = t.id === activeTab ? "active" : "";
    btn.setAttribute("role", "tab");
    btn.setAttribute("aria-selected", String(t.id === activeTab));
    btn.addEventListener("click", () => navigate(t.id));
    nav.appendChild(btn);
  }
}

async function navigate(tab: string): Promise<void> {
  if (!TABS.some((t) => t.id === tab)) return;
  activeTab = tab;
  await renderActiveTab();
}

// ---------------- 首次运行（§15.1） ----------------

async function maybeFirstRun(): Promise<void> {
  // 已初始化就完全不发这个请求：first_run_report 内部要枚举网卡（带子进程开销），
  // 主窗口每次打开都为它阻塞数秒是启动慢的主要来源之一
  if (state?.initialized) return;
  const report = await api.firstRunReport();
  if (report.initialized) return;
  const view = h("view");
  activeTab = "profiles";
  renderTabs();
  view.innerHTML = `
    <div class="card">
      <h2>欢迎使用速网 SuNet</h2>
      <p class="hint">检测到你的系统当前已有配置。速网不会在你主动操作前修改任何一项。</p>
      <div class="kv"><span class="k">hosts</span><span>${
        report.hosts_custom_lines > 0
          ? `已有 ${report.hosts_custom_lines} 行自定义内容（不在本工具管理范围内，不会被改动）`
          : "无自定义内容"
      }${report.hosts_block_lines > 0 ? `，托管区块已有 ${report.hosts_block_lines} 条` : ""}</span></div>
      <div class="kv"><span class="k">系统代理</span><span>${
        report.proxy.pac_present
          ? "已存在 PAC 配置（AutoConfigURL），速网不会在你动手前覆盖它"
          : report.proxy.enable
            ? `已开启（${ui.esc(report.proxy.server || "未知地址")}，来源可能是其他软件）`
            : "未开启"
      }</span></div>
      <div class="kv"><span class="k">系统 DNS</span><span>${
        report.dns.length
          ? report.dns
              .slice(0, 3)
              .map(([alias, s]) => `${ui.esc(alias)}：${ui.esc(s)}`)
              .join("；")
          : "未检测到已连接网卡"
      }</span></div>
      <div class="section-title">接下来</div>
      <p class="hint">把当前状态存成一个方案，你就有了一个「回到原点」的落点。</p>
      <div class="actions">
        <input type="text" id="first-run-name" placeholder="方案名称" value="当前配置（${new Date()
          .toLocaleDateString("zh-CN")
          .replace(/\//g, "-")}）" style="width:240px" />
        <button class="primary" id="first-run-go">保存并开始使用</button>
        <button id="first-run-skip">跳过</button>
      </div>
      <p class="hint" style="margin-top:8px">权限说明：代理开关完全静默，无需管理员权限；只有修改 hosts / DNS 时才会在<b>你主动点击</b>后弹一次 UAC。</p>
    </div>`;

  document.getElementById("first-run-skip")?.addEventListener("click", async () => {
    await api.firstRunFinish("当前配置（未命名）");
    await refreshState(true);
  });
  document.getElementById("first-run-go")?.addEventListener("click", async () => {
    const name = (document.getElementById("first-run-name") as HTMLInputElement).value.trim();
    try {
      await api.firstRunFinish(name || null);
      ui.toast("info", "已保存为方案", "可用它在任何时候回到当前状态");
      await refreshState(true);
    } catch (e) {
      ui.toast("error", "保存失败", errText(e));
    }
  });
}

// ---------------- 崩溃恢复提示（§6.3） ----------------

async function maybeCrashRecovery(): Promise<void> {
  if (!state?.crash_recovery) return;
  const c = state.crash_recovery;
  ui.banner(
    "warn",
    `上次异常退出（${c.marker}），已检测到未保存的网络配置变更。速网不会自动改动你的系统状态，请选择处理方式。`,
    [
      {
        label: "查看差异",
        run: async () => {
          if (!c.snapshot_id) {
            ui.toast("warn", "没有可用快照");
            return;
          }
          try {
            const d = await api.snapshotsDiff(c.snapshot_id);
            const lines = d.identical
              ? ["当前状态与快照一致，无需处理。"]
              : d.items.map((i) => `${i.kind} · ${i.field}：${i.before} → ${i.after}`);
            ui.banner("info", lines.join("\n"), [], true);
          } catch (e) {
            ui.toast("error", "读取差异失败", errText(e));
          }
        },
      },
      {
        label: "还原到快照",
        run: async () => {
          const yes = await ui.confirmModal({
            title: "还原到崩溃前快照？",
            body: "将按快照逐字节还原 hosts，并把代理与 DNS 恢复到该时刻的状态。",
            confirmText: "还原",
            danger: true,
          });
          if (!yes) return;
          try {
            const r = await api.rollback(c.snapshot_id ?? null);
            ui.showReport(r, "还原结果");
            await refreshState(true);
          } catch (e) {
            ui.toast("error", "还原失败", errText(e));
          }
        },
      },
      { label: "忽略", run: () => void api.markNotificationsRead() },
    ],
    true,
  );
}

// ---------------- 启动 ----------------

async function boot(): Promise<void> {
  initTheme();
  onThemeChange(() => void api.markNotificationsRead());

  h("btn-apply-profile").addEventListener("click", async () => {
    const select = h("active-profile") as HTMLSelectElement;
    if (!select.value) return;
    await applyProfile(select.value);
  });
  h("btn-min").addEventListener("click", () => void api.windowMinimize());
  h("btn-hide").addEventListener("click", () => void api.windowHide());
  h("btn-settings").addEventListener("click", () => openSettingsModal());
  h("btn-global-restore").addEventListener("click", () => void globalRestore());

  await listen<{ level: string; title: string; body: string; actions: { label: string; action: string }[] }>(
    "sunet://notify",
    (ev) => {
      const p = ev.payload;
      const actions = (p.actions ?? []).map((a) => ({
        label: a.label,
        run: () => runNotifyAction(a.action),
      }));
      ui.toast((p.level as "info" | "warn" | "error") ?? "info", p.title, p.body, actions);
    },
  );
  await listen<import("./api").ApplyReport>("sunet://report", () => {
    void refreshState(true);
  });
  // 快捷面板用 sunet://navigate 把用户带到对应位置；settings 是唯一的非标签页目标
  await listen<string>("sunet://navigate", (ev) => {
    if (ev.payload === "settings") {
      openSettingsModal();
      return;
    }
    void navigate(ev.payload);
  });

  renderTabs();
  await refreshState();
  // 先渲染当前标签页再做首启检测：后者在"未初始化"时才需要跑，
  // 若先跑它，主窗口首屏会在"加载中…"上卡住数秒（含网卡枚举开销）
  if (state?.initialized) {
    await renderActiveTab();
  } else {
    await maybeFirstRun();
  }
  await maybeCrashRecovery();
  // 订阅到期的同步与热键状态在启动后 2 秒内自查一次
  setTimeout(async () => {
    try {
      const sc = await api.shortcutStatus();
      if (sc.enabled && !sc.registered) {
        ui.banner(
          "error",
          `全局热键 ${sc.binding} 未生效：${sc.error ? `${sc.error.code} ${sc.error.message}` : "注册失败"}。功能仍可通过托盘菜单与主界面使用。`,
          [{ label: "去设置", run: () => openSettingsModal() }],
        );
      }
    } catch {
      /* 忽略：状态查询失败不影响主流程 */
    }
  }, 2000);
}

export async function applyProfile(id: string): Promise<void> {
  const s = state;
  const name = s?.profiles.find((p) => p.id === id)?.name ?? id;
  // 提权说明只在"确实需要提权"时出现，且用户可关闭（§1.4.4 ③）
  const ok = await ui.elevationNotice(
    `即将切换方案「${name}」（可能包含 hosts 与 DNS 变更）`,
    !!s && !s.prand.is_elevated && s.settings.confirm_elevation_notice,
  );
  if (!ok) return;
  ui.toast("info", "正在切换…", "含 hosts / DNS 的方案会弹出一次 UAC");
  try {
    const r = await api.profileApply(id);
    if (r.ok) {
      ui.toast("info", "切换完成", r.message);
    } else {
      ui.showReport(r, "切换失败");
    }
    await refreshState(true);
  } catch (e) {
    ui.toast("error", "切换失败", errText(e));
    await refreshState(true);
  }
}

async function globalRestore(): Promise<void> {
  const yes = await ui.confirmModal({
    title: "全局还原？",
    body: "将清除 SuNet 的托管 hosts 区块、关闭系统代理、把当前方案的 DNS 切回自动获取。\n你自己的 hosts 内容（区块外）不会被改动。",
    confirmText: "执行全局还原",
    danger: true,
  });
  if (!yes) return;
  try {
    const r = await api.globalRestore();
    ui.showReport(r, "全局还原结果");
    await refreshState(true);
  } catch (e) {
    ui.toast("error", "全局还原失败", errText(e));
  }
}

function runNotifyAction(action: string): void {
  const [cmd, arg] = action.split(":");
  switch (cmd) {
    case "open":
      if (arg === "settings") openSettingsModal();
      else void navigate(arg);
      break;
    case "proxy":
      if (arg === "undo_clear") {
        void api
          .proxyUndoClear()
          .then(() => refreshState(true))
          .catch((e) => ui.toast("error", "撤销失败", errText(e)));
      }
      break;
    case "system":
      if (arg === "open_backup") void api.systemOpenDir("backup");
      break;
    case "sources":
      // 订阅同步的「查看变更」：确认清单在 Hosts 页的订阅卡片下方，
      // 打开页面时会自动列出所有待确认的暂存变更
      if (arg === "show_diff") void navigate("hosts");
      break;
    default:
      void navigate("profiles");
  }
}

window.addEventListener("unhandledrejection", (e) => {
  ui.toast("error", "未处理的错误", String((e as PromiseRejectionEvent).reason));
});

// 供设置弹层读取当前 settings
export function currentSettings(): Settings | null {
  return state?.settings ?? null;
}

// 主窗口入口。快捷面板是独立入口 quick.html（见 src/quick-main.ts）
void boot();
