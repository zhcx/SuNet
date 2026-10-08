// DNS 标签页（设计方案 §4.3：v4 / v6 独立可控 + 写前预览 + 写后回读）

import {
  api,
  errText,
  type DnsInterfaceView,
  type DnsPreset,
  type DnsState,
  type DohReference,
} from "../api";
import type { TabCtx } from "../main";
import * as ui from "../ui";

interface Draft {
  alias: string;
  presetId: string;
  enableV4: boolean;
  enableV6: boolean;
  v4: string;
  v6: string;
}

let draft: Draft = {
  alias: "",
  presetId: "ali",
  enableV4: true,
  enableV6: false,
  v4: "",
  v6: "",
};

export async function renderDns(root: HTMLElement, ctx: TabCtx): Promise<void> {
  const readonly = !!ctx.state.read_only;
  root.innerHTML = `<div class="card"><h2>正在读取网卡…</h2></div>`;

  let interfaces: DnsInterfaceView[] = [];
  let presets: DnsPreset[] = [];
  let doh: DohReference[] = [];
  try {
    [interfaces, presets, doh] = await Promise.all([
      api.dnsInterfaces(),
      api.dnsPresets(""),
      api.dnsDohReference(),
    ]);
  } catch (e) {
    root.innerHTML = `<div class="card"><h2>读取网卡失败</h2><div class="error-text">${ui.esc(
      errText(e),
    )}</div></div>`;
    return;
  }

  if (!draft.alias || !interfaces.some((i) => i.alias === draft.alias)) {
    // 默认落点：当前正在使用的网卡（物理 + 已连接 + 有默认网关）
    const preferred =
      interfaces.find((i) => i.is_in_use) ??
      interfaces.find((i) => i.is_physical && i.status.toLowerCase() === "up") ??
      interfaces[0];
    draft.alias = preferred?.alias ?? "";
  }

  root.innerHTML = `
    <div class="card">
      <div class="row-between">
        <div>
          <h2>系统 DNS</h2>
          <p class="hint">IPv4 与 IPv6 是两套独立解析通道，本工具让你分别控制。<b>取消勾选某一族 = 该族还原为自动获取</b>，不会留下隐性残留。</p>
        </div>
        <div class="actions">
          <button id="dn-flush">刷新 DNS 缓存</button>
        </div>
      </div>

      <div class="field-grid">
        <span>网卡</span>
        <div class="actions">
          <select id="dn-alias" style="min-width:260px"></select>
          <span id="dn-current" class="muted"></span>
        </div>
        <span>服务商</span>
        <input type="text" id="dn-search" placeholder="搜索名称 / 地址 / 地区…" />
      </div>

      <div class="grid-2" style="margin-top:12px">
        <div>
          <div class="section-title">选择解析服务</div>
          <div class="preset-list" id="dn-presets"></div>
        </div>
        <div>
          <div class="section-title">地址族与预览</div>
          <label class="row"><input type="checkbox" id="dn-v4-en" /> IPv4（默认勾选）</label>
          <input type="text" id="dn-v4" class="mono" style="width:100%;margin:4px 0 10px" />
          <label class="row"><input type="checkbox" id="dn-v6-en" /> IPv6（默认不勾选）</label>
          <input type="text" id="dn-v6" class="mono" style="width:100%;margin:4px 0 10px" />
          <div id="dn-v6-warn"></div>
          <div class="muted" style="font-size:13px">预览：将在 <b id="dn-preview-alias"></b> 上设置</div>
          <pre id="dn-preview"></pre>
          <div class="actions" style="margin-top:10px">
            <button class="primary" id="dn-apply" ${readonly ? "disabled" : ""}>应用</button>
            <button id="dn-test">测速（写入前先确认地址会响应）</button>
            <button id="dn-reset-all" ${readonly ? "disabled" : ""}>还原为自动</button>
            <button id="dn-reset-v4" ${readonly ? "disabled" : ""}>仅还原 IPv4</button>
            <button id="dn-reset-v6" ${readonly ? "disabled" : ""}>仅还原 IPv6</button>
          </div>
          <div id="dn-test-out" style="margin-top:8px"></div>
        </div>
      </div>
      <div id="dn-report" style="margin-top:12px"></div>
    </div>

    <div class="card">
      <h2>我的 DNS</h2>
      <p class="hint">局域网自建解析（AdGuard Home / Pi-hole 等）请填这里。切换 Wi-Fi 后 IP 变了会导致解析失败 —— 那是地址失效，不是工具的问题；本工具只提示，不会自动回滚。</p>
      <div class="actions">
        <input type="text" id="my-name" placeholder="名称，如 家里 AdGuard Home" style="width:200px" />
        <input type="text" id="my-v4" placeholder="IPv4，多个用逗号分隔" style="width:220px" class="mono" />
        <input type="text" id="my-v6" placeholder="IPv6（可留空）" style="width:220px" class="mono" />
        <button id="my-save" ${readonly ? "disabled" : ""}>保存为自定义项</button>
      </div>
      <div id="my-list" style="margin-top:10px"></div>
    </div>

    <div class="card">
      <h2>加密 DNS（DoH / DoT）端点参考</h2>
      <p class="hint">
        v1 不写系统 DoH 配置（涉及 HKLM 与 Windows 11 特性，且 Chrome/Edge 自带的「安全 DNS」会在应用层覆盖系统设置）。
        需要时把下面的地址复制到对应软件里；如果启用了浏览器的安全 DNS，系统 DNS 设置对该浏览器无效 —— 这和 hosts 被绕过是同一类坑。
      </p>
      <table>
        <thead><tr><th>服务商</th><th>DoH</th><th>DoT</th></tr></thead>
        <tbody>
          ${doh
            .map(
              (d) =>
                `<tr><td>${ui.esc(d.name)}</td><td class="mono" style="user-select:text">${ui.esc(
                  d.doh,
                )}</td><td class="mono" style="user-select:text">${ui.esc(d.dot || "—")}</td></tr>`,
            )
            .join("")}
        </tbody>
      </table>
    </div>
  `;

  const $ = <T extends HTMLElement>(sel: string) => root.querySelector(sel) as T;
  const aliasSel = $<HTMLSelectElement>("#dn-alias");
  for (const i of interfaces) {
    const o = document.createElement("option");
    o.value = i.alias;
    o.textContent = `${i.alias}${i.is_physical ? "" : "（虚拟）"}${
      i.is_in_use ? "（当前使用）" : ""
    } · ${i.description}`;
    aliasSel.appendChild(o);
  }
  aliasSel.value = draft.alias;

  let current: DnsState | null = null;
  const loadCurrent = async () => {
    try {
      current = await api.dnsGet(aliasSel.value);
      const v4 = current.v4.is_dhcp
        ? `自动获取${current.v4.servers.length ? `（DHCP: ${current.v4.servers.join("/")}）` : ""}`
        : `手动 ${current.v4.servers.join("/")}`;
      const v6 = current.v6.is_dhcp
        ? `自动获取${current.v6.servers.length ? `（DHCP: ${current.v6.servers.join("/")}）` : ""}`
        : `手动 ${current.v6.servers.join("/")}`;
      $<HTMLElement>("#dn-current").textContent = `当前 IPv4 ${v4} · IPv6 ${v6}`;
    } catch (e) {
      $<HTMLElement>("#dn-current").textContent = errText(e);
    }
    const iface = interfaces.find((i) => i.alias === aliasSel.value);
    const warn = $<HTMLElement>("#dn-v6-warn");
    if (iface && !iface.has_v6_global && $<HTMLInputElement>("#dn-v6-en").checked) {
      warn.innerHTML = `<div class="warn-text" style="font-size:13px">当前网卡无 IPv6 通路（[E4004]），写入 IPv6 DNS 只会让部分域名解析变慢。</div>`;
    } else {
      warn.innerHTML = "";
    }
  };

  const renderPresets = (list: DnsPreset[]) => {
    const box = $<HTMLElement>("#dn-presets");
    box.innerHTML = "";
    if (!list.length) {
      box.innerHTML = `<div class="empty">${ui.iconEl("dns", 22)}<div>没有匹配的服务商</div>
        <div class="faint">换个关键字，或在下方「我的 DNS」里自建</div></div>`;
      return;
    }
    for (const p of list) {
      const div = document.createElement("div");
      div.className = `preset ${p.id === draft.presetId ? "active" : ""}`;
      const region =
        p.region === "domestic"
          ? `<span class="pill domestic">国内</span>`
          : p.region === "global"
            ? `<span class="pill global">国外</span>`
            : `<span class="pill">系统</span>`;
      div.innerHTML = `
        <div><input type="radio" name="dn-preset" ${p.id === draft.presetId ? "checked" : ""} /></div>
        <div>
          <div class="title">${ui.esc(p.name)} ${region}${
            p.is_custom ? `<span class="pill">自定义</span>` : ""
          }</div>
          <div class="addr">${ui.esc(p.v4.join(" / ") || "无 IPv4")}　|　${ui.esc(
            p.v6.join(" / ") || "无 IPv6",
          )}</div>
          <div class="muted" style="font-size:12px">${ui.esc(p.note)}</div>
        </div>`;
      div.addEventListener("click", () => {
        draft.presetId = p.id;
        draft.v4 = p.v4.join(", ");
        draft.v6 = p.v6.join(", ");
        if (p.v4.length === 0) draft.enableV4 = false;
        if (p.v6.length === 0) draft.enableV6 = false;
        renderPresets(currentList);
        syncInputs();
        updatePreview();
      });
      box.appendChild(div);
    }
  };

  let currentList = presets;
  renderPresets(presets);

  const syncInputs = () => {
    $<HTMLInputElement>("#dn-v4").value = draft.v4;
    $<HTMLInputElement>("#dn-v6").value = draft.v6;
    $<HTMLInputElement>("#dn-v4-en").checked = draft.enableV4;
    $<HTMLInputElement>("#dn-v6-en").checked = draft.enableV6;
  };

  const updatePreview = () => {
    $<HTMLElement>("#dn-preview-alias").textContent = aliasSel.value;
    const v4 = draft.enableV4 ? splitAddr(draft.v4) : [];
    const v6 = draft.enableV6 ? splitAddr(draft.v6) : [];
    const lines: string[] = [];
    lines.push(`  IPv4 → ${v4.length ? v4.join(", ") : "还原为自动获取"}`);
    lines.push(`  IPv6 → ${v6.length ? v6.join(", ") : "还原为自动获取"}`);
    $<HTMLElement>("#dn-preview").textContent = lines.join("\n");
  };

  const applyPresetDefaults = () => {
    const p = presets.find((x) => x.id === draft.presetId);
    if (p) {
      draft.v4 = p.v4.join(", ");
      draft.v6 = p.v6.join(", ");
      draft.enableV4 = p.v4.length > 0;
      draft.enableV6 = p.v6.length > 0;
    }
  };
  applyPresetDefaults();
  syncInputs();
  updatePreview();
  await loadCurrent();

  aliasSel.addEventListener("change", async () => {
    draft.alias = aliasSel.value;
    await loadCurrent();
    updatePreview();
  });
  // 搜索输入防抖：预设表可能很长，逐键重建整份列表既费 DOM 也费布局
  let searchTimer: number | undefined;
  $<HTMLInputElement>("#dn-search").addEventListener("input", (ev) => {
    const q = (ev.target as HTMLInputElement).value.toLowerCase();
    if (searchTimer !== undefined) window.clearTimeout(searchTimer);
    searchTimer = window.setTimeout(() => {
      currentList = presets.filter(
        (p) =>
          p.name.toLowerCase().includes(q) ||
          p.note.toLowerCase().includes(q) ||
          p.v4.some((a) => a.includes(q)) ||
          p.v6.some((a) => a.includes(q)),
      );
      renderPresets(currentList);
    }, 120);
  });
  $<HTMLInputElement>("#dn-v4").addEventListener("input", (ev) => {
    draft.v4 = (ev.target as HTMLInputElement).value;
    updatePreview();
  });
  $<HTMLInputElement>("#dn-v6").addEventListener("input", (ev) => {
    draft.v6 = (ev.target as HTMLInputElement).value;
    updatePreview();
  });
  $<HTMLInputElement>("#dn-v4-en").addEventListener("change", (ev) => {
    draft.enableV4 = (ev.target as HTMLInputElement).checked;
    updatePreview();
  });
  $<HTMLInputElement>("#dn-v6-en").addEventListener("change", async (ev) => {
    draft.enableV6 = (ev.target as HTMLInputElement).checked;
    updatePreview();
    if (draft.enableV6) {
      const ready = await api.dnsV6Ready(aliasSel.value);
      if (!ready) {
        $<HTMLElement>("#dn-v6-warn").innerHTML = `<div class="warn-text" style="font-size:13px">当前网卡无 IPv6 通路（[E4004]），写入 IPv6 DNS 只会让部分域名解析变慢。</div>`;
      }
    }
  });

  const showReport = (r: import("../api").ApplyReport) => {
    $<HTMLElement>("#dn-report").innerHTML = `<div class="muted">${ui.esc(
      r.message,
    )}</div>${ui.stepsHtml(r.steps)}`;
  };

  $<HTMLButtonElement>("#dn-apply").addEventListener("click", async () => {
    const v4 = draft.enableV4 ? splitAddr(draft.v4) : [];
    const v6 = draft.enableV6 ? splitAddr(draft.v6) : [];
    if (!v4.length && !v6.length) {
      ui.toast("error", "没有要写入的地址", "两族都为空等同于「还原为自动获取」");
      return;
    }
    const ok = await ui.elevationNotice(
      `即将在网卡「${aliasSel.value}」上设置 DNS（IPv4 ${v4.length || 0} 个 / IPv6 ${v6.length || 0} 个）`,
      ctx.state.settings.confirm_elevation_notice,
    );
    if (!ok) return;
    try {
      const r = await api.dnsSet({ alias: aliasSel.value, v4, v6 });
      showReport(r);
      if (r.ok) ui.toast("info", "DNS 已设置", "写入后已回读校验");
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "DNS 设置失败", errText(e));
      $<HTMLElement>("#dn-report").innerHTML = `<div class="error-text">${ui.esc(errText(e))}</div>`;
    }
  });

  const doReset = async (family: string | null) => {
    const label = family === "v4" ? "IPv4" : family === "v6" ? "IPv6" : "IPv4 与 IPv6";
    const yes = await ui.confirmModal({
      title: `还原 ${label} 为自动获取？`,
      body: "将清除该地址族的手动 DNS 设置，交回 DHCP / 路由器下发。",
      confirmText: "还原",
    });
    if (!yes) return;
    const ok = await ui.elevationNotice(`即将还原 ${label} DNS`, ctx.state.settings.confirm_elevation_notice);
    if (!ok) return;
    try {
      const r = await api.dnsReset(aliasSel.value, family);
      showReport(r);
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "还原失败", errText(e));
    }
  };
  $<HTMLButtonElement>("#dn-reset-all").addEventListener("click", () => void doReset(null));
  $<HTMLButtonElement>("#dn-reset-v4").addEventListener("click", () => void doReset("v4"));
  $<HTMLButtonElement>("#dn-reset-v6").addEventListener("click", () => void doReset("v6"));

  $<HTMLButtonElement>("#dn-flush").addEventListener("click", async () => {
    try {
      await api.dnsFlush();
      ui.toast("info", "DNS 缓存已刷新");
    } catch (e) {
      ui.toast("error", "刷新失败", errText(e));
    }
  });

  $<HTMLButtonElement>("#dn-test").addEventListener("click", async () => {
    const servers = [
      ...(draft.enableV4 ? splitAddr(draft.v4) : []),
      ...(draft.enableV6 ? splitAddr(draft.v6) : []),
    ];
    const out = $<HTMLElement>("#dn-test-out");
    if (!servers.length) {
      out.innerHTML = `<span class="warn-text">当前没有勾选任何地址族</span>`;
      return;
    }
    out.innerHTML = `<span class="muted">测速中（IPv4 走 A 记录、IPv6 走 AAAA 记录，单次超时 1.5s）…</span>`;
    try {
      const results = await api.dnsTest(servers);
      out.innerHTML = `<table><thead><tr><th>地址</th><th>族</th><th>结果</th></tr></thead><tbody>${results
        .map(
          (r) =>
            `<tr><td class="mono">${ui.esc(r.server)}</td><td>${ui.esc(r.family)}</td><td class="${
              r.ok ? "ok-text" : "error-text"
            }">${ui.esc(r.ok ? `${r.latency_ms} ms` : r.message)}</td></tr>`,
        )
        .join("")}</tbody></table>
        ${
          results.some((r) => r.family === "v6" && !r.ok)
            ? `<div class="warn-text" style="margin-top:6px">有 IPv6 地址无响应：可能是本机没有可用 IPv6 通路（[E4004]）。这种情况下写入 IPv6 DNS 只会让部分域名解析变慢，建议取消该族勾选。</div>`
            : ""
        }`;
    } catch (e) {
      out.innerHTML = `<span class="error-text">${ui.esc(errText(e))}</span>`;
    }
  });

  // 我的 DNS
  const renderCustom = (list: DnsPreset[]) => {
    const box = $<HTMLElement>("#my-list");
    const customs = list.filter((p) => p.is_custom);
    box.innerHTML = customs.length
      ? ""
      : `<div class="muted">还没有自定义项。</div>`;
    for (const c of customs) {
      const div = document.createElement("div");
      div.className = "row-between";
      div.style.borderBottom = "1px solid var(--border)";
      div.style.padding = "6px 0";
      div.innerHTML = `<div><b>${ui.esc(c.name)}</b> <span class="mono muted">${ui.esc(
        [...c.v4, ...c.v6].join(" / "),
      )}</span></div>`;
      const use = document.createElement("button");
      use.textContent = "填入";
      use.addEventListener("click", () => {
        draft.presetId = c.id;
        draft.v4 = c.v4.join(", ");
        draft.v6 = c.v6.join(", ");
        draft.enableV4 = c.v4.length > 0;
        draft.enableV6 = c.v6.length > 0;
        syncInputs();
        updatePreview();
      });
      const del = document.createElement("button");
      del.className = "link danger";
      del.textContent = "删除";
      del.addEventListener("click", async () => {
        const next = await api.dnsCustomDelete(c.id);
        presets = next;
        currentList = next.filter((p) => !p.is_custom || true);
        renderPresets(currentList);
        renderCustom(next);
      });
      const actions = document.createElement("div");
      actions.className = "actions";
      actions.append(use, del);
      div.appendChild(actions);
      box.appendChild(div);
    }
  };
  renderCustom(presets);

  $<HTMLButtonElement>("#my-save").addEventListener("click", async () => {
    const name = $<HTMLInputElement>("#my-name").value.trim();
    const v4 = splitAddr($<HTMLInputElement>("#my-v4").value);
    const v6 = splitAddr($<HTMLInputElement>("#my-v6").value);
    if (!name || (!v4.length && !v6.length)) {
      ui.toast("error", "请填写名称与至少一个地址族");
      return;
    }
    try {
      const list = await api.dnsCustomSave({
        id: "",
        name,
        region: "domestic",
        features: [],
        v4,
        v6,
        doh: null,
        dot: null,
        note: "自定义",
        is_custom: true,
        is_favorite: false,
      });
      presets = list;
      renderPresets(list);
      renderCustom(list);
      ui.toast("info", "已保存自定义 DNS");
    } catch (e) {
      ui.toast("error", "保存失败", errText(e));
    }
  });
}

function splitAddr(s: string): string[] {
  return s
    .split(/[,，\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);
}
