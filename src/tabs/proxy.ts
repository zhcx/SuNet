// 代理标签页（设计方案 §3）

import { api, errText, paintProxyEndpoints, proxyEndpoints, type ProxyView } from "../api";
import type { TabCtx } from "../main";
import * as ui from "../ui";
import * as os from "../platform";

export async function renderProxy(root: HTMLElement, ctx: TabCtx): Promise<void> {
  const view: ProxyView = await api.proxyGet();
  const st = view.state;
  const last = readLastForm();
  // 逐协议落点：三个协议都**可选**（留空 = 关闭该协议，两平台三个开关互相独立），至少要填一个
  const splitEp = (v?: string | null) => {
    if (!v) return { host: "", port: "" };
    const i = v.lastIndexOf(":");
    return i <= 0 ? { host: v, port: "" } : { host: v.slice(0, i), port: v.slice(i + 1) };
  };
  // 老状态（≤ 0.0.2）只记了一个主落点，那时它就是 HTTP；新状态逐协议记录 ——
  // HTTP 留空就是"没开 HTTP"，**不能**拿 `server` 顶上（那时它可能是 HTTPS / SOCKS 的落点）
  const legacy = !st.http_proxy && !st.https_proxy && !st.socks_proxy;
  const http0 = splitEp(st.http_proxy ?? (legacy ? st.server : ""));
  const https0 = splitEp(st.https_proxy);
  const socks0 = splitEp(st.socks_proxy);
  const host = last.host ?? http0.host;
  const port = last.port ?? http0.port;
  const httpsHost = last.https_host ?? https0.host;
  const httpsPort = last.https_port ?? https0.port;
  const socksHost = last.socks_host ?? socks0.host;
  const socksPort = last.socks_port ?? socks0.port;
  const bypass = last.bypass ?? st.bypass;
  const readonly = !!ctx.state.read_only;

  // pi-lens-ignore: no-inner-html-js -- 动态内容均经 esc/ui.esc 转义后拼接（项目统一模板写法）
  root.innerHTML = `
    <div class="card">
      <h2>${os.PROXY_TITLE}</h2>
      <p class="hint">${os.PROXY_HINT}</p>

      <div class="row-between" style="margin-bottom:12px">
        <div>
          <div class="kv"><span class="k">当前状态</span><span id="px-state"></span></div>
          <div class="kv"><span class="k">落点</span><span id="px-eps"></span></div>
          <div class="kv"><span class="k">PAC</span><span id="px-pac"></span></div>
        </div>
        <div class="actions">
          <button id="px-undo" class="ghost" ${view.can_undo ? "" : "disabled"}>撤销上次清除</button>
        </div>
      </div>

      ${true ? `<div class="field-grid">
        <span>HTTP 代理</span>
        <div style="display:flex;gap:8px">
          <input type="text" id="px-host" value="${ui.esc(host)}" placeholder="${os.IS_MACOS ? "留空 = 不启用" : "127.0.0.1 或 proxy.local"}" />
          <input type="number" id="px-port" min="1" max="65535" value="${ui.esc(port)}" placeholder="7890" style="width:120px" />
        </div>
        <span>HTTPS 代理</span>
        <div style="display:flex;gap:8px">
          <input type="text" id="px-https-host" value="${ui.esc(httpsHost)}" placeholder="留空 = 不启用" />
          <input type="number" id="px-https-port" min="1" max="65535" value="${ui.esc(httpsPort)}" placeholder="7891" style="width:120px" />
        </div>
        <span>SOCKS 代理</span>
        <div style="display:flex;gap:8px">
          <input type="text" id="px-socks-host" value="${ui.esc(socksHost)}" placeholder="留空 = 不启用" />
          <input type="number" id="px-socks-port" min="1" max="65535" value="${ui.esc(socksPort)}" placeholder="1080" style="width:120px" />
        </div>
        <span>绕过列表</span>
        <input type="text" id="px-bypass" value="${ui.esc(bypass)}" />
      </div>` : ""}

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
          ${os.PROXY_SCOPE_WHY}
        </div>
      </details>
    </div>
  `;

  const stateEl = root.querySelector("#px-state")!;
  const epsEl = root.querySelector("#px-eps")!;
  const pacEl = root.querySelector("#px-pac")!;
  const renderState = (s: ProxyView) => {
    stateEl.textContent = s.state.enable
      ? `已开启（${proxyEndpoints(s.state).length} 个协议）`
      : "已关闭";
    (stateEl as HTMLElement).className = s.state.enable ? "ok-text" : "muted";
    // 落点逐协议列出：macOS 三个开关互相独立，只报一个地址会让人以为只走了一个代理
    paintProxyEndpoints(epsEl, s.state);
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
  // pi-lens-ignore: no-inner-html-js -- 动态内容均经 esc/ui.esc 转义后拼接（项目统一模板写法）
    body.innerHTML = `<div class="muted" style="margin-bottom:8px">${ui.esc(
      report.message,
    )}</div>${ui.stepsHtml(report.steps)}`;
  };

  // 地址与端口都为空 = 该协议不启用（读不到元素时兜底为 undefined）
  const ep = (hSel: string, pSel: string) => {
    const h = (root.querySelector(hSel) as HTMLInputElement | null)?.value.trim() ?? "";
    const p = Number((root.querySelector(pSel) as HTMLInputElement | null)?.value || 0);
    return h || p ? { host: h, port: p } : undefined;
  };
  const val = (sel: string) => (root.querySelector(sel) as HTMLInputElement | null)?.value ?? "";

  const config = () => {
    // HTTP 也是可选的：留空 = 关闭该协议（macOS 三个开关互相独立）
    const http = ep("#px-host", "#px-port");
    return {
      enable: true,
      host: http?.host ?? "",
      port: http?.port ?? 0,
      https: ep("#px-https-host", "#px-https-port"),
      socks: ep("#px-socks-host", "#px-socks-port"),
      bypass: val("#px-bypass").trim(),
      force: false,
    };
  };

  const saveForm = () => {
    writeLastForm({
      host: val("#px-host"),
      port: val("#px-port"),
      https_host: val("#px-https-host"),
      https_port: val("#px-https-port"),
      socks_host: val("#px-socks-host"),
      socks_port: val("#px-socks-port"),
      bypass: val("#px-bypass"),
    });
  };

  root.querySelector("#px-enable")?.addEventListener("click", async () => {
    const cfg = config();
    saveForm();
    // HTTP 也是可选的：留空 = 关闭该协议（两平台的三个协议互相独立）
    const http = cfg.host || cfg.port ? { host: cfg.host, port: cfg.port } : undefined;
    const pairs: Array<[string, { host: string; port: number } | undefined]> = [
      ["HTTP", http],
      ["HTTPS", cfg.https],
      ["SOCKS", cfg.socks],
    ];
    // 逐协议成对校验：只填一半会让"这个协议到底开不开"变得含糊
    const half = pairs.find(([, e]) => e && (!e.host || !e.port));
    if (half) {
      ui.toast("error", `${half[0]} 代理的地址与端口要成对填写`, "整套留空表示关闭该协议");
      return;
    }
    if (!pairs.some(([, e]) => e)) {
      ui.toast(
        "error",
        "至少填写一个代理协议",
        "HTTP / HTTPS / SOCKS 三个都留空 = 不开代理",
      );
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
      body: os.IS_MACOS
        ? "将执行：关闭 HTTP / HTTPS / SOCKS 三个代理开关，并关闭自动代理（PAC）。\n\n这个操作不区分代理来源，公司 VPN / 企业网关的代理配置也会被一起抹掉。清除后 5 秒内可撤销。"
        : "将执行：ProxyEnable=0、删除 AutoConfigURL、清空 ProxyServer，保留绕过列表。\n\n这个操作不区分代理来源，公司 VPN / 企业网关的代理配置也会被一起抹掉。清除后 5 秒内可撤销。",
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
let draft: {
  host?: string;
  port?: string;
  https_host?: string;
  https_port?: string;
  socks_host?: string;
  socks_port?: string;
  bypass?: string;
} = {};

function readLastForm() {
  return draft;
}

function writeLastForm(v: {
  host: string;
  port: string;
  https_host: string;
  https_port: string;
  socks_host: string;
  socks_port: string;
  bypass: string;
}) {
  draft = v;
}

function patchSettings(ctx: TabCtx, patch: Record<string, unknown>): void {
  void api
    .settingsSave({ ...ctx.state.settings, ...patch } as never)
    .catch(() => undefined);
}
