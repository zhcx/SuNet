// 代理标签页（设计方案 §3）

import { api, errText, type ProxyView } from "../api";
import type { TabCtx } from "../main";
import * as ui from "../ui";

export async function renderProxy(root: HTMLElement, ctx: TabCtx): Promise<void> {
  const view: ProxyView = await api.proxyGet();
  const st = view.state;
  const last = readLastForm();
  const host = last.host ?? (st.server ? st.server.split(":").slice(0, -1).join(":") : "");
  const port = last.port ?? (st.server ? st.server.split(":").pop() ?? "" : "");
  const bypass = last.bypass ?? st.bypass;
  const readonly = !!ctx.state.read_only;

  root.innerHTML = `
    <div class="card">
      <h2>系统代理（WinINET 全局代理）</h2>
      <p class="hint">写入 HKCU\\...\\Internet Settings 并调用 InternetSetOptionW 通知系统，<b>全程不需要管理员权限</b>。</p>

      <div class="row-between" style="margin-bottom:12px">
        <div>
          <div class="kv"><span class="k">当前状态</span><span id="px-state"></span></div>
          <div class="kv"><span class="k">PAC</span><span id="px-pac"></span></div>
        </div>
        <div class="actions">
          <button id="px-undo" class="ghost" ${view.can_undo ? "" : "disabled"}>撤销上次清除</button>
        </div>
      </div>

      <div class="field-grid">
        <span>地址</span>
        <input type="text" id="px-host" value="${ui.esc(host)}" placeholder="127.0.0.1 或 proxy.local" />
        <span>端口</span>
        <input type="number" id="px-port" min="1" max="65535" value="${ui.esc(port)}" placeholder="7890" style="width:120px" />
        <span>绕过列表</span>
        <input type="text" id="px-bypass" value="${ui.esc(bypass)}" />
      </div>

      <div style="margin:12px 0">
        <label class="row"><input type="checkbox" id="px-probe" ${view.probe_before_enable ? "checked" : ""} />
          开启前做连通性自检（800ms TCP 探测；探测失败会拒绝开启并说明原因）</label>
      </div>

      <div class="actions">
        <button class="primary" id="px-enable" ${readonly ? "disabled" : ""}>开启代理</button>
        <button id="px-disable" ${readonly ? "disabled" : ""}>关闭代理（按接管前快照还原）</button>
        <button class="danger" id="px-clear" ${readonly ? "disabled" : ""}>清除代理设置（彻底清零）</button>
      </div>
      <p class="hint" style="margin-top:10px">
        「清除代理设置」会同时删除 <code>AutoConfigURL</code>（PAC），
        不区分代理来源 —— 公司 VPN / 企业网关写入的代理配置也会一起被抹掉。
        因此它自带 5 秒撤销窗口，主界面也保留常驻的「撤销上次清除」入口。
      </p>
    </div>

    <div class="card" id="px-report" hidden>
      <h2>上次操作结果</h2>
      <div id="px-report-body"></div>
    </div>

    <div class="card">
      <h2>生效范围与盲区</h2>
      <p class="hint">这张表决定了"开了代理为什么有些程序没走"。工具只承诺它承诺的事。</p>
      <table>
        <thead><tr><th style="width:60%">应用</th><th>是否跟随系统代理</th></tr></thead>
        <tbody>
          ${view.scope
            .map(
              ([app, follows]) =>
                `<tr><td>${ui.esc(app)}</td><td class="${follows.startsWith("不跟随") ? "faint" : ""}">${ui.esc(follows)}</td></tr>`,
            )
            .join("")}
        </tbody>
      </table>
      <details class="help">
        <summary>为什么有的程序不跟随？</summary>
        <div style="margin-top:6px">
          系统代理位于 WinINET 层，Chrome / Edge / Electron 应用 / 大部分办公软件都会读它；
          而 Windows Update、部分系统服务走 WinHTTP（机器级，需 <code>netsh winhttp</code>），
          <code>curl</code> / <code>git</code> / <code>npm</code> 默认不读系统代理（Git 可配 <code>http.proxy</code>）。
        </div>
      </details>
    </div>
  `;

  const stateEl = root.querySelector("#px-state")!;
  const pacEl = root.querySelector("#px-pac")!;
  const renderState = (s: ProxyView) => {
    stateEl.textContent = s.state.enable
      ? `已开启（${s.state.server || "未设置地址"}）`
      : "已关闭";
    (stateEl as HTMLElement).className = s.state.enable ? "ok-text" : "muted";
    pacEl.textContent = s.state.pac_present
      ? "存在（PAC 优先级高于手动代理，会使「关闭代理」失效）"
      : "无";
    (pacEl as HTMLElement).className = s.state.pac_present ? "warn-text" : "muted";
  };
  renderState(view);

  const showReport = (report: import("../api").ApplyReport) => {
    const box = root.querySelector("#px-report") as HTMLElement;
    const body = root.querySelector("#px-report-body") as HTMLElement;
    box.hidden = false;
    body.innerHTML = `<div class="muted" style="margin-bottom:8px">${ui.esc(
      report.message,
    )}</div>${ui.stepsHtml(report.steps)}`;
  };

  const config = () => ({
    enable: true,
    host: (root.querySelector("#px-host") as HTMLInputElement).value.trim(),
    port: Number((root.querySelector("#px-port") as HTMLInputElement).value || 0),
    bypass: (root.querySelector("#px-bypass") as HTMLInputElement).value.trim(),
    force: false,
  });

  const saveForm = () => {
    writeLastForm({
      host: (root.querySelector("#px-host") as HTMLInputElement).value,
      port: (root.querySelector("#px-port") as HTMLInputElement).value,
      bypass: (root.querySelector("#px-bypass") as HTMLInputElement).value,
    });
  };

  root.querySelector("#px-enable")?.addEventListener("click", async () => {
    const cfg = config();
    saveForm();
    if (!cfg.host || !cfg.port) {
      ui.toast("error", "请先填写地址与端口", "端口范围 1–65535");
      return;
    }
    const probe = (root.querySelector("#px-probe") as HTMLInputElement).checked;
    const okElev = await ui.elevationNotice("即将开启系统代理", false);
    if (!okElev) return;
    patchSettings(ctx, { proxy_probe_before_enable: probe });
    try {
      const r = await api.proxySet(cfg, probe);
      showReport(r);
      if (!r.ok) {
        // 探测失败：给出"仍然开启"的明确出口（高级用户，明确知情）
        const reason = r.steps.map((s) => s.message).join("\n");
        const force = await ui.confirmModal({
          title: "开启失败",
          body: `${reason}\n\n仍然开启的话，所有遵循系统代理的应用都会无法联网。`,
          confirmText: "仍然开启",
          danger: true,
        });
        if (force) {
          const r2 = await api.proxySet({ ...cfg, force: true }, false);
          showReport(r2);
        }
      } else {
        ui.toast("info", "代理已开启", "Chromium 系应用通常无需重启即生效");
      }
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "开启失败", errText(e));
    }
  });

  root.querySelector("#px-disable")?.addEventListener("click", async () => {
    try {
      const r = await api.proxySet(
        { enable: false, host: "", port: 0, bypass: "", force: false },
        false,
      );
      showReport(r);
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "关闭失败", errText(e));
    }
  });

  root.querySelector("#px-clear")?.addEventListener("click", async () => {
    const yes = await ui.confirmModal({
      title: "彻底清除系统代理设置？",
      body: "将执行：ProxyEnable=0、删除 AutoConfigURL、清空 ProxyServer，保留绕过列表。\n\n这个操作不区分代理来源，公司 VPN / 企业网关的代理配置也会被一起抹掉。清除后 5 秒内可撤销。",
      confirmText: "清除",
      danger: true,
    });
    if (!yes) return;
    try {
      await api.proxyClear();
      ui.toast(
        "warn",
        "已清除系统代理设置",
        "5 秒内可撤销",
        [{ label: "撤销", run: () => void undo() }],
        6000,
      );
      await ctx.refresh(true);
      setTimeout(() => void ctx.refresh(false), 5500);
    } catch (e) {
      ui.toast("error", "清除失败", errText(e));
    }
  });

  const undo = async () => {
    try {
      await api.proxyUndoClear();
      ui.toast("info", "已撤销", "系统代理配置已原样写回");
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "撤销失败", errText(e));
    }
  };
  root.querySelector("#px-undo")?.addEventListener("click", () => void undo());
}

// 表单在标签页切换后保留草稿（避免反复输入）
let draft: { host?: string; port?: string; bypass?: string } = {};

function readLastForm() {
  return draft;
}

function writeLastForm(v: { host: string; port: string; bypass: string }) {
  draft = v;
}

function patchSettings(ctx: TabCtx, patch: Record<string, unknown>): void {
  void api
    .settingsSave({ ...ctx.state.settings, ...patch } as never)
    .catch(() => undefined);
}
