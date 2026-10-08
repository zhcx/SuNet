// 任务栏快捷小窗（托盘左键弹出，label = quick）
//
// 设计目标：**不打开主窗口就能完成日常操作**，所以它是一套独立的"瓦片"界面，
// 不是主界面的压缩版：
//   · 顶部只留一行状态（方案 / 权限 / 脏标记），不摆标签页
//   · 主体是 4 块可点瓦片：代理（大号开关）、hosts、DNS、清除代理
//   · 方案切换做成一行 chip，点一下即切
//   · 一切需要提权或破坏性的动作走**内联确认条**，不用居中弹层（小窗放不下）
//
// 交互原则：能在面板里一步做完的，绝不把用户丢回主窗口；
// 但需要"看到细节"的操作（编辑条目、选服务商、看快照）一律跳主窗口对应标签页。

import { listen } from "@tauri-apps/api/event";
import { api, errText, type AppStateView, type ProfileBrief } from "./api";
import { initTheme, onThemeChange, cycleTheme, currentMode } from "./theme";
import { icon, type IconName } from "./icons";
import * as ui from "./ui";
import * as os from "./platform";

const ic = (name: IconName, size = 16) => `<span class="i">${icon[name](size)}</span>`;
const esc = ui.esc;

let state: AppStateView | null = null;
let busy = false;

export async function bootQuick(): Promise<void> {
  os.markPlatform();
  document.documentElement.dataset.win = "quick";
  initTheme();
  onThemeChange(() => void refresh());

  await listen<{ level: string; title: string; body: string }>("sunet://notify", (ev) => {
    const p = ev.payload;
    if (p.level === "info") return; // 面板里只留需要用户知道的（§5.4.4 通知策略）
    ui.toast((p.level as "warn" | "error") ?? "warn", p.title, p.body);
  });
  await listen("sunet://report", () => void refresh());
  // 面板被反复唤出（失焦只隐藏），每次弹出都重新拉状态，
  // 否则设置改动、方案删除、主窗口里的操作都可能在面板上留下陈旧显示。
  await listen("sunet://refresh", () => void refresh());

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") void api.quickHide();
  });

  // 新开的面板不该继承上一次操作遗留的"钉住"状态；同时这也是面板就绪的信号
  await api.quickSetPinned(false).catch(() => undefined);
  await refresh();

  // 网卡 DNS 状态的首次采集要走 PowerShell（约 1 秒，主进程后台线程在做），
  // 面板可能先于它渲染完，所以补一次延迟刷新，避免 DNS 瓦片长期显示默认值
  window.setTimeout(() => void refresh(), 1600);
}

async function refresh(): Promise<void> {
  try {
    state = await api.getAppState();
  } catch (e) {
    renderFatal(errText(e));
    return;
  }
  ui.setElevated(state.prand.is_elevated);
  render();
}

function renderFatal(msg: string): void {
  const app = document.getElementById("app")!;
  app.innerHTML = `
    <div class="flyout">
      <div class="flyout-head" data-tauri-drag-region="deep">
        <span class="flyout-title">${ic("zap", 15)}速网</span>
        <span class="spacer"></span>
        <button class="icon-btn sm" id="q-close" title="收起">${ic("close", 14)}</button>
      </div>
      <div class="flyout-body">
        <div class="flyout-note danger">
          <span class="grow">${esc(msg)}</span>
        </div>
      </div>
    </div>`;
  document.getElementById("q-close")?.addEventListener("click", () => void api.quickHide());
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

interface View {
  ro: boolean;
  proxyOn: boolean;
  hostsOn: boolean;
  hostsNeedsWrite: boolean;
  dnsManual: boolean;
  active?: ProfileBrief;
}

function view(): View {
  const s = state!;
  const active = s.profiles.find((p) => p.id === s.active_profile_id);
  return {
    ro: !!s.read_only,
    proxyOn: s.proxy.enable && !s.proxy.pac_present,
    hostsOn: s.hosts.block_present && s.hosts.enabled_entries > 0,
    hostsNeedsWrite: !s.hosts.block_present || s.hosts_pending,
    dnsManual: !s.tray.dns_v4.includes("自动") || !s.tray.dns_v6.includes("自动"),
    active,
  };
}

/** 生效状态标题：口径与主界面 / 托盘一致 —— 没有活动方案时按真实生效状态显示 */
function effectiveHeadline(v: View): string {
  if (v.active) return v.active.name;
  return v.proxyOn || v.hostsOn || v.dnsManual ? "自定义设置" : "默认 · 直连";
}

function render(): void {
  if (!state) return;
  const v = view();
  const s = state;

  document.getElementById("app")!.innerHTML = `
  <div class="flyout">
    <div class="flyout-head" data-tauri-drag-region="deep">
      <span class="flyout-title">
        <span class="dot ${v.proxyOn || v.hostsOn || v.dnsManual ? "on" : ""}"></span>速网
      </span>
      <span class="spacer"></span>
      <button class="icon-btn sm" id="q-theme" title="主题：${themeLabel()}">${themeIcon()}</button>
      <button class="icon-btn sm" id="q-main" title="打开主窗口">${ic("external", 14)}</button>
      <button class="icon-btn sm" id="q-close" title="收起（Esc）">${ic("close", 14)}</button>
    </div>

    <div class="flyout-status">
      <span class="badge plain">${esc(effectiveHeadline(v))}</span>
      ${s.prand.is_elevated ? `<span class="badge ok plain">已提权</span>` : ""}
      ${s.runtime.dirty ? `<span class="badge warn plain">已改系统配置</span>` : ""}
      ${s.runtime.lock_contended ? `<span class="badge warn plain">另一进程写入中</span>` : ""}
      ${v.ro ? `<span class="badge err plain">只读</span>` : ""}
    </div>

    <div class="flyout-body">
      ${notes()}
      ${tileProxy(v)}
      <div class="tiles">${tileHosts(v)}${tileDns(v)}</div>
      ${tileClear(v)}
      ${undoStrip()}
      <div class="flyout-section">${ic("profile", 13)}快速切换方案<span class="spacer"></span>${
        v.ro ? "" : `<button class="link sm" id="q-new">新建</button>`
      }</div>
      <div class="chip-row">${chips(v)}</div>
    </div>

    <div class="flyout-foot">
      <button class="sm" id="q-settings">${ic("gear", 14)}设置</button>
      <span class="spacer"></span>
      <button class="sm primary" id="q-open">主窗口</button>
    </div>
  </div>`;

  bind();
  fitHeight();
}

/** 上一次已生效的面板高度：用来避免高度没变时反复发窗口 resize IPC */
let lastFitHeight = 0;

/** 飞行面板贴内容：量出自然高度交给主进程调整窗口（底边不动） */
function fitHeight(): void {
  const head = document.querySelector(".flyout-head") as HTMLElement | null;
  const status = document.querySelector(".flyout-status") as HTMLElement | null;
  const foot = document.querySelector(".flyout-foot") as HTMLElement | null;
  const body = document.querySelector(".flyout-body") as HTMLElement | null;
  if (!head || !status || !foot || !body) return;

  // 不能用 body.scrollHeight：body 是 flex:1，内容不足时它会被拉到容器高度
  const kids = Array.from(body.children) as HTMLElement[];
  const cs = getComputedStyle(body);
  const gap = parseFloat(cs.rowGap || "0") || 0;
  const content =
    kids.reduce((sum, el) => sum + el.offsetHeight, 0) +
    gap * Math.max(0, kids.length - 1) +
    (parseFloat(cs.paddingTop) || 0) +
    (parseFloat(cs.paddingBottom) || 0);

  const natural = Math.ceil(
    head.offsetHeight + status.offsetHeight + foot.offsetHeight + content + 2,
  );
  // 高度没变就不再发 IPC（每次操作结束都会走到这里）；只有真正生效才记下来，
  // 失败时保留旧值以便下次重试。
  if (natural === lastFitHeight) return;
  void api
    .quickFit(natural)
    .then(() => {
      lastFitHeight = natural;
    })
    .catch(() => undefined);
}

function themeLabel(): string {
  const m = currentMode();
  return m === "light" ? "浅色" : m === "dark" ? "深色" : "跟随系统";
}

function themeIcon(): string {
  const m = currentMode();
  return m === "light" ? ic("sun", 14) : m === "dark" ? ic("moon", 14) : ic("auto", 14);
}

function notes(): string {
  const s = state!;
  const out: string[] = [];
  if (s.read_only) {
    out.push(`<div class="flyout-note danger">
      <span class="grow">只读模式 ${esc(s.read_only.code)}：${esc(s.read_only.message)}</span>
    </div>`);
  }
  if (s.runtime.crash_recovery) {
    out.push(`<div class="flyout-note warn">
      <span class="grow">上次异常退出，有未回滚的变更</span>
      <button class="sm" id="q-crash">处理</button>
    </div>`);
  }
  if (s.proxy.pac_present) {
    out.push(`<div class="flyout-note warn">
      <span class="grow">系统存在 PAC（AutoConfigURL），手动代理不生效</span>
      <button class="sm" id="q-pac">详情</button>
    </div>`);
  }
  return out.join("");
}

/** 代理瓦片：面板里的头号操作，用大号开关 + 一个通栏 */
function tileProxy(v: View): string {
  const addr =
    state!.proxy.server ||
    (v.active ? `${v.active.proxy_host}:${v.active.proxy_port}` : "");
  const stateText = v.proxyOn
    ? "所有遵循系统代理的应用都会走它"
    : state!.proxy.pac_present
      ? "当前由 PAC 接管"
      : "应用直连，不经过代理";
  return `<div class="tile wide ${v.proxyOn ? "on" : ""}">
    <div class="tile-top">
      <span class="tile-icon">${ic("proxy", 15)}</span>
      <span class="tile-name">系统代理</span>
      <span class="spacer"></span>
      <label class="switch switch-lg" title="一键开关（不需要管理员权限）">
        <input type="checkbox" id="q-proxy" ${v.proxyOn ? "checked" : ""} ${
          v.ro ? "disabled" : ""
        } />
        <span class="track"></span>
      </label>
    </div>
    ${
      v.proxyOn && addr
        ? `<div class="tile-addr">${esc(addr)}</div>`
        : `<div class="tile-state">${stateText}</div>`
    }
  </div>`;
}

function tileHosts(v: View): string {
  const h = state!.hosts;
  const text = !h.block_present
    ? "托管区块未写入"
    : `已写入 ${h.enabled_entries} 条${state!.hosts_pending ? "（有草稿未写入）" : ""}`;
  return `<div class="tile ${v.hostsOn ? "on" : ""}">
    <div class="tile-top">
      <span class="tile-icon">${ic("hosts", 15)}</span>
      <span class="tile-name">hosts</span>
    </div>
    <div class="tile-state">${text}</div>
    <div class="tile-actions">
      ${
        v.hostsNeedsWrite
          ? `<button class="sm primary" id="q-hosts-apply" ${v.ro ? "disabled" : ""}>写入</button>`
          : `<button class="sm" id="q-hosts-go">查看</button>`
      }
      ${state!.hosts_pending && h.block_present ? `<button class="sm" id="q-hosts-go2">对比</button>` : ""}
    </div>
  </div>`;
}

function tileDns(v: View): string {
  const short = (x: string) => x.replace(/^DNS\s*/, "");
  return `<div class="tile ${v.dnsManual ? "on" : ""}">
    <div class="tile-top">
      <span class="tile-icon">${ic("dns", 15)}</span>
      <span class="tile-name">DNS</span>
    </div>
    <div class="tile-state">
      <div>${esc(short(state!.tray.dns_v4))}</div>
      <div>${esc(short(state!.tray.dns_v6))}</div>
    </div>
    <div class="tile-actions">
      ${
        v.dnsManual && !v.ro
          ? `<button class="sm" id="q-dns-reset" title="把 IPv4/IPv6 都交回自动获取">还原自动</button>`
          : ""
      }
      <button class="sm" id="q-dns-go">${v.dnsManual ? "调整" : "配置"}</button>
    </div>
  </div>`;
}

function tileClear(v: View): string {
  return `<div class="tile wide">
    <div class="tile-top">
      <span class="tile-icon">${ic("power", 15)}</span>
      <span class="tile-name">清除代理设置</span>
      <span class="spacer"></span>
      <button class="sm danger" id="q-clear" ${v.ro ? "disabled" : ""}>清除</button>
    </div>
    <div class="tile-state">
      ${ic("warn", 12)} 同时删除 PAC，不区分代理来源；5 秒内可撤销
    </div>
  </div>`;
}

function undoStrip(): string {
  if (!state?.can_undo_clear) return "";
  return `<div class="flyout-note warn">
    <span class="grow">刚刚清除了系统代理</span>
    <button class="sm" id="q-undo">${ic("undo", 13)}撤销</button>
  </div>`;
}

function chips(v: View): string {
  const s = state!;
  const chips = s.profiles
    .map(
      (p) => `<button class="chip ${p.id === s.active_profile_id ? "active" : ""}"
        data-profile="${esc(p.id)}" title="${esc(p.name)}" ${v.ro ? "disabled" : ""}>${esc(
          p.name,
        )}</button>`,
    )
    .join("");
  return `${chips}<button class="chip add" id="q-new2" title="在主窗口新建方案" ${
    v.ro ? "disabled" : ""
  }>＋</button>`;
}

// ---------------------------------------------------------------------------
// 交互
// ---------------------------------------------------------------------------

/** 内联确认条：贴在 body 顶部（小窗里居中弹层会很挤） */
function inlineConfirm(
  text: string,
  confirmLabel: string,
  onOk: () => void | Promise<void>,
  danger = false,
): void {
  const holder = document.querySelector(".flyout-body") as HTMLElement;
  holder.querySelectorAll(".inline-confirm").forEach((n) => n.remove());
  const strip = document.createElement("div");
  strip.className = `inline-confirm ${danger ? "danger" : ""}`;
  strip.innerHTML = `
    <span class="grow">${esc(text)}</span>
    <button class="sm" id="ic-cancel">取消</button>
    <button class="sm ${danger ? "danger" : "primary"}" id="ic-ok">${esc(
      confirmLabel,
    )}</button>`;
  holder.prepend(strip);
  strip.querySelector("#ic-cancel")?.addEventListener("click", () => strip.remove());
  strip.querySelector("#ic-ok")?.addEventListener("click", async () => {
    strip.remove();
    await onOk();
  });
}

/** 操作期间的临时提示（例如探测失败），贴在 body 顶部 */
function flash(text: string, danger = false, retry?: () => void): void {
  const holder = document.querySelector(".flyout-body") as HTMLElement;
  holder.querySelectorAll(".inline-confirm").forEach((n) => n.remove());
  const strip = document.createElement("div");
  strip.className = `inline-confirm ${danger ? "danger" : "warn"}`;
  strip.innerHTML = `<span class="grow">${esc(text)}</span>
    ${retry ? `<button class="sm danger" id="fc-retry">仍然开启</button>` : ""}
    <button class="sm" id="fc-close">知道了</button>`;
  holder.prepend(strip);
  strip.querySelector("#fc-close")?.addEventListener("click", () => strip.remove());
  strip.querySelector("#fc-retry")?.addEventListener("click", () => {
    strip.remove();
    retry?.();
  });
}

function bind(): void {
  // 注意：这里的调用点传的是 "#q-close" 这类选择器，
  // 之前写成 getElementById（只接受裸 id）会永远拿到 null —— 整个面板的按钮
  // 看起来"点了没反应"，实际上是一个监听器都没挂上（实测踩过，别改回去）
  const $ = (sel: string) => document.querySelector(sel) as HTMLButtonElement | null;

  $("#q-close")?.addEventListener("click", () => void api.quickHide());
  $("#q-main")?.addEventListener("click", () => void api.quickOpenMain(null));
  $("#q-open")?.addEventListener("click", () => void api.quickOpenMain(null));
  $("#q-settings")?.addEventListener("click", () => void api.quickOpenMain("settings"));
  $("#q-theme")?.addEventListener("click", () => {
    cycleTheme();
    render();
  });

  // 细节类操作一律跳主窗口对应页
  $("#q-dns-go")?.addEventListener("click", () => void api.quickOpenMain("dns"));
  $("#q-hosts-go")?.addEventListener("click", () => void api.quickOpenMain("hosts"));
  $("#q-hosts-go2")?.addEventListener("click", () => void api.quickOpenMain("hosts"));
  $("#q-crash")?.addEventListener("click", () => void api.quickOpenMain("profiles"));
  $("#q-pac")?.addEventListener("click", () => void api.quickOpenMain("proxy"));
  $("#q-new")?.addEventListener("click", () => void api.quickOpenMain("profiles"));
  $("#q-new2")?.addEventListener("click", () => void api.quickOpenMain("profiles"));

  // ---- 代理开关：一步到位，不需要提权，因此不预先确认 ----
  $("#q-proxy")?.addEventListener("change", (e) => {
    const box = e.target as HTMLInputElement;
    const wantOn = box.checked;
    if (!wantOn) {
      void run(box, async () => {
        const r = await api.proxySet(
          { enable: false, host: "", port: 0, bypass: "", force: false },
          false,
        );
        if (r.ok) ui.toast("info", "系统代理已关闭", r.message);
        else flash(r.message, true);
      });
      return;
    }
    const cfg = activeProxy();
    if (!cfg) {
      box.checked = false;
      flash("当前方案里没有代理地址，请先到「代理」页填写", true);
      void api.quickOpenMain("proxy");
      return;
    }
    const probe = state!.settings.proxy_probe_before_enable;
    void run(box, async () => {
      const r = await api.proxySet({ ...cfg, enable: true, force: false }, probe);
      if (r.ok) {
        ui.toast("info", "系统代理已开启", "Chromium 系应用通常无需重启");
      } else {
        // 探测失败时不闷掉：给"仍然开启"的明确出口（高级用户，明确知情）
        flash(r.message, true, () => {
          void run(null, async () => {
            const r2 = await api.proxySet({ ...cfg, enable: true, force: true }, false);
            if (!r2.ok) flash(r2.message, true);
          });
        });
      }
    });
  });

  // ---- hosts 写入：需要一次管理员授权 ----
  $("#q-hosts-apply")?.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    inlineConfirm(`写入系统 hosts 需要一次管理员授权（${os.AUTH_HINT}）`, "继续", () =>
      run(btn, async () => {
        const r = await api.hostsApply();
        if (r.ok) ui.toast("info", "hosts 已写入", r.message);
        else ui.showReport(r, "hosts 写入失败");
      }),
    );
  });

  // ---- DNS 还原为自动：需要一次管理员授权 ----
  $("#q-dns-reset")?.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    inlineConfirm("把 IPv4 / IPv6 DNS 都交回自动获取？需要一次管理员授权", "还原", () =>
      run(btn, async () => {
        const alias = activeAlias();
        if (!alias) {
          flash("没有可操作的网卡，请到 DNS 页选择", true);
          return;
        }
        const r = await api.dnsReset(alias, null);
        if (r.ok) ui.toast("info", "DNS 已还原为自动获取", r.message);
        else ui.showReport(r, "DNS 还原失败");
      }),
    );
  });

  // ---- 彻底清除：破坏性，必须确认 ----
  $("#q-clear")?.addEventListener("click", () => {
    inlineConfirm(
      "彻底清除系统代理设置（含 PAC），不区分来源，5 秒内可撤销",
      "清除",
      () =>
        run(null, async () => {
          await api.proxyClear();
        }),
      true,
    );
  });

  $("#q-undo")?.addEventListener("click", (e) => {
    void run(e.currentTarget as HTMLButtonElement, async () => {
      await api.proxyUndoClear();
      ui.toast("info", "已撤销", "系统代理配置已原样写回");
    });
  });

  // ---- 方案切换：点一下即切，需提权时先确认 ----
  document.querySelectorAll<HTMLButtonElement>("[data-profile]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.dataset.profile!;
      const p = state?.profiles.find((x) => x.id === id);
      if (!p) return;
      if (id === state?.active_profile_id) return;
      inlineConfirm(`切换到「${p.name}」？hosts / DNS 变更需要一次管理员授权`, "切换", () =>
        run(btn, async () => {
          const r = await api.profileApply(id);
          if (r.ok) ui.toast("info", "切换完成", r.message);
          else ui.showReport(r, "切换失败");
        }),
      );
    });
  });
}

function activeProfile(): ProfileBrief | undefined {
  return state?.profiles.find((x) => x.id === state?.active_profile_id);
}

function activeProxy(): { host: string; port: number; bypass: string } | null {
  const p = activeProfile();
  if (!p || !p.proxy_host.trim() || !p.proxy_port) return null;
  return { host: p.proxy_host, port: p.proxy_port, bypass: p.proxy_bypass };
}

/** 还原 DNS 需要一个网卡；面板不摆选择器，用当前方案里记的那个 */
function activeAlias(): string | null {
  const p = activeProfile();
  return p && p.dns_alias.trim() ? p.dns_alias : null;
}

async function run<T>(el: HTMLElement | null, fn: () => Promise<T>): Promise<void> {
  // 静默吞掉点击 = 用户以为按钮坏了（实测反馈"所有按钮无反应"）；
  // 操作确实要串行，但必须给出可见反馈
  if (busy) {
    flash("上一步操作还在执行，请稍候…");
    return;
  }
  busy = true;
  // 执行期间钉住面板：授权框会抢焦点，不钉住的话面板会在用户点「是 / 好」的瞬间消失
  await api.quickSetPinned(true).catch(() => undefined);
  try {
    if (el instanceof HTMLButtonElement) await ui.withBusy(el, fn);
    else await fn();
  } catch (e) {
    flash(errText(e), true);
  } finally {
    busy = false;
    await api.quickSetPinned(false).catch(() => undefined);
    await refresh();
  }
}
