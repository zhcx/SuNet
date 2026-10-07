// 方案标签页：方案列表 + 编辑器 + 快照/回滚（设计方案 §6 / §7.1）

import {
  api,
  errText,
  type DnsPreset,
  type HostsView,
  type Profile,
  type Source,
} from "../api";
import type { TabCtx } from "../main";
import { svg } from "../icons";
import * as ui from "../ui";
import * as os from "../platform";

export async function renderProfiles(root: HTMLElement, ctx: TabCtx): Promise<void> {
  const [profiles, snapshots, hosts, sources, proxyView] = await Promise.all([
    api.profileList(),
    api.snapshotsList(),
    api.hostsList(),
    api.sourcesList(),
    api.proxyGet(),
  ]);
  const readonly = !!ctx.state.read_only;
  const s = ctx.state;
  const proxyOn = proxyView.state.enable && !proxyView.state.pac_present;
  const hostsOn = s.hosts.block_present && s.hosts.enabled_entries > 0;
  const dnsManual =
    !s.tray.dns_v4.includes("自动") || !s.tray.dns_v6.includes("自动");
  // 没有活动方案时不再写「未启用任何方案」：回落显示内置直连（它本身就是生效状态），
  // 只有配置里连内置方案都找不到这种异常情况才给一个中性说法
  const active =
    s.profiles.find((p) => p.id === s.active_profile_id) ??
    s.profiles.find((p) => p.is_builtin);

  root.innerHTML = `
    <div class="card">
      <div class="row-between">
        <div>
          <h2>${svg("shield", 17)}三层总览</h2>
          <p class="hint">
            当前生效：<b>${active ? ui.esc(active.name) : "未修改系统设置"}</b>　·　
            托盘图标<b>左键</b>可随时唤出快捷面板（${ui.esc(s.settings.shortcuts.clear_proxy.binding)} 一键清除代理）
          </p>
        </div>
        <div class="actions">
          <button id="pf-refresh">${svg("refresh", 14)}刷新</button>
          <button id="pf-panel">${svg("zap", 14)}快捷面板</button>
        </div>
      </div>

      <div class="layer-grid">
        <div class="layer ${hostsOn ? "on" : ""}">
          <div class="layer-head"><span class="i">${svg("hosts", 15)}</span>hosts 托管</div>
          <div class="layer-state">${
            s.hosts.block_present
              ? `${s.hosts.enabled_entries} 条生效${s.hosts.total_entries !== s.hosts.enabled_entries ? ` / 共 ${s.hosts.total_entries} 条` : ""}`
              : "未写入托管区块"
          }</div>
          <div class="layer-foot">
            <span class="sub">${
              hosts.pending ? "草稿待写入" : s.hosts.exists ? "区块已同步" : "文件不存在"
            }</span>
            <div class="actions">
              <button class="sm" id="pf-hosts-go">${svg("logs", 13)}编辑</button>
              ${
                hosts.pending || !s.hosts.block_present
                  ? `<button class="sm primary" id="pf-hosts-apply" ${readonly ? "disabled" : ""}>写入</button>`
                  : ""
              }
            </div>
          </div>
        </div>

        <div class="layer ${proxyOn ? "on" : ""}">
          <div class="layer-head"><span class="i">${svg("proxy", 15)}</span>系统代理</div>
          <div class="layer-state">${
            proxyView.state.pac_present
              ? "PAC 接管中（手动代理不生效）"
              : proxyOn
                ? ui.esc(proxyView.state.server || "已开启，未填地址")
                : "已关闭"
          }</div>
          <div class="layer-foot">
            <label class="switch">
              <input type="checkbox" id="pf-proxy-switch" ${proxyOn ? "checked" : ""} ${
                readonly ? "disabled" : ""
              } />
              <span class="track"></span>
              <span class="sub">${proxyOn ? "跟随系统" : "不接管"}</span>
            </label>
            <div class="actions"><button class="sm" id="pf-proxy-go">${svg("gear", 13)}配置</button></div>
          </div>
        </div>

        <div class="layer ${dnsManual ? "on" : ""}">
          <div class="layer-head"><span class="i">${svg("dns", 15)}</span>系统 DNS</div>
          <div class="layer-state">${ui.esc(s.tray.dns_v4)}<br />${ui.esc(s.tray.dns_v6)}</div>
          <div class="layer-foot">
            <span class="sub">${dnsManual ? "手动指定" : "自动获取"}</span>
            <div class="actions"><button class="sm" id="pf-dns-go">${svg("gear", 13)}配置</button></div>
          </div>
        </div>
      </div>
      <div id="pf-hero-msg" style="margin-top:10px"></div>
    </div>

    <div class="card">
      <div class="row-between">
        <div>
          <h2>方案（Profile）</h2>
          <p class="hint">一个方案 = hosts 条目 + 系统代理 + 系统 DNS 的原子组合。一键切换，失败自动逆序回滚。</p>
        </div>
        <div class="actions">
          <button id="pf-new" ${readonly ? "disabled" : ""}>新建方案</button>
        </div>
      </div>
      <div id="pf-list"></div>
    </div>

    <div class="card">
      <h2>快照与回滚</h2>
      <p class="hint">每次切换前都会采集 hosts 原始字节、${os.PROXY_SNAPSHOT_NAME}与各网卡 DNS 状态。保留最近 ${ctx.state.settings.hosts_backup_keep} 份。</p>
      <div class="table-wrap">
        <table>
          <thead>
            <tr><th>时间</th><th>原因</th><th>代理</th><th>DNS 网卡</th><th>hosts</th><th></th></tr>
          </thead>
          <tbody id="sn-tbody"></tbody>
        </table>
      </div>
      <div id="sn-empty" class="empty" hidden>还没有快照。</div>
      <div class="actions" style="margin-top:10px">
        <button id="sn-open">打开快照目录</button>
        <button id="sn-hostsbak">查看 hosts 备份目录</button>
      </div>
      <div id="sn-diff"></div>
    </div>
  `;

  const list = root.querySelector("#pf-list") as HTMLElement;
  for (const p of profiles) {
    const card = document.createElement("div");
    card.style.borderBottom = "1px solid var(--border)";
    card.style.padding = "10px 0";
    const active = ctx.state.active_profile_id === p.id;
    const hostsCount = p.hosts.enabled ? p.hosts.entry_ids.length : 0;
    const subs = p.hosts.source_ids.length;
    const proxyText = p.proxy.enabled
      ? `代理 ${p.proxy.host || "未填地址"}:${p.proxy.port || 0}`
      : "代理 关闭";
    const dnsText = p.dns.enabled
      ? `DNS ${
          p.dns.mode === "dhcp"
            ? "还原为自动"
            : `${p.dns.preset_id ? presetName(p.dns.preset_id) : "自定义"} ${
                p.dns.enable_v4 ? "v4" : ""
              }${p.dns.enable_v6 ? "+v6" : ""}`
        }（${p.dns.interface_alias || "未选网卡"}）`
      : "DNS 不管理";
    card.innerHTML = `
      <div class="row-between">
        <div>
          <div style="font-weight:600">
            ${ui.esc(p.name)}
            ${p.is_builtin ? `<span class="pill">内置</span>` : ""}
            ${active ? `<span class="pill domestic">当前生效</span>` : ""}
          </div>
          <div class="muted" style="font-size:13px">
            hosts ${hostsCount} 条${subs ? ` + ${subs} 个订阅源` : ""} · ${ui.esc(proxyText)} · ${ui.esc(dnsText)}
          </div>
          ${p.note ? `<div class="faint" style="font-size:12px">${ui.esc(p.note)}</div>` : ""}
        </div>
        <div class="actions">
          <button data-act="apply" class="primary" ${readonly ? "disabled" : ""}>切换到此方案</button>
          <button data-act="edit" ${readonly || p.is_builtin ? "disabled" : ""}>编辑</button>
          <button data-act="del" class="link danger" ${
            readonly || p.is_builtin ? "disabled" : ""
          }>删除</button>
        </div>
      </div>`;
    (card.querySelector('[data-act="apply"]') as HTMLButtonElement).addEventListener("click", async () => {
      const ok = await ui.elevationNotice(
        `即将切换方案「${p.name}」（可能包含 hosts 与 DNS 变更）`,
        !ctx.state.prand.is_elevated && ctx.state.settings.confirm_elevation_notice,
      );
      if (!ok) return;
      try {
        const r = await api.profileApply(p.id);
        if (r.ok) ui.toast("info", "切换完成", r.message);
        else ui.showReport(r, "切换失败");
        await ctx.refresh(true);
      } catch (e) {
        ui.toast("error", "切换失败", errText(e));
      }
    });
    (card.querySelector('[data-act="edit"]') as HTMLButtonElement).addEventListener("click", () =>
      void profileEditor(p, hosts, sources, ctx),
    );
    (card.querySelector('[data-act="del"]') as HTMLButtonElement).addEventListener("click", async () => {
      const yes = await ui.confirmModal({
        title: "删除方案",
        body: `删除「${p.name}」不会改动系统状态，只移除这份配置。`,
        confirmText: "删除",
        danger: true,
        typedWord: p.name,
      });
      if (!yes) return;
      await api.profileDelete(p.id);
      await ctx.refresh(true);
    });
    list.appendChild(card);
  }

  // ---- 三层总览的快捷动作 ----
  (root.querySelector("#pf-refresh") as HTMLButtonElement).addEventListener("click", (e) =>
    void ui.withBusy(e.currentTarget as HTMLButtonElement, () => ctx.refresh(true)),
  );
  (root.querySelector("#pf-panel") as HTMLButtonElement).addEventListener("click", async () => {
    try {
      await api.quickShow();
    } catch (e) {
      ui.toast("error", "无法唤出快捷面板", errText(e));
    }
  });
  (root.querySelector("#pf-hosts-go") as HTMLButtonElement).addEventListener("click", () =>
    ctx.navigate("hosts"),
  );
  (root.querySelector("#pf-dns-go") as HTMLButtonElement).addEventListener("click", () =>
    ctx.navigate("dns"),
  );
  (root.querySelector("#pf-proxy-go") as HTMLButtonElement).addEventListener("click", () =>
    ctx.navigate("proxy"),
  );

  const heroMsg = root.querySelector("#pf-hero-msg") as HTMLElement;
  const applyHosts = async (btn: HTMLButtonElement) => {
    const ok = await ui.elevationNotice(
      "即将把托管区块写入系统 hosts 文件",
      ctx.state.settings.confirm_elevation_notice,
    );
    if (!ok) return;
    await ui.withBusy(btn, async () => {
      try {
        const r = await api.hostsApply();
        if (r.ok) ui.toast("info", "hosts 已写入", r.message);
        else ui.showReport(r, "hosts 写入结果");
      } catch (e) {
        ui.toast("error", "hosts 写入失败", errText(e));
      }
      await ctx.refresh(true);
    });
  };
  root.querySelector("#pf-hosts-apply")?.addEventListener("click", (e) =>
    void applyHosts(e.currentTarget as HTMLButtonElement),
  );

  root.querySelector("#pf-proxy-switch")?.addEventListener("change", async (e) => {
    const box = e.target as HTMLInputElement;
    const wantOn = box.checked;
    if (wantOn && (!active || !active.proxy_host.trim() || !active.proxy_port)) {
      box.checked = false;
      ui.toast("warn", "当前方案里没有代理地址", "请到「代理」页填写地址与端口后保存方案");
      ctx.navigate("proxy");
      return;
    }
    const ok = await ui.elevationNotice(os.PROXY_SWITCH_NOTICE, os.PROXY_NEEDS_ADMIN);
    if (!ok) return;
    try {
      const r = await api.proxySet(
        {
          enable: wantOn,
          host: active?.proxy_host ?? "",
          port: active?.proxy_port ?? 0,
          bypass: active?.proxy_bypass ?? "",
          force: false,
        },
        ctx.state.settings.proxy_probe_before_enable,
      );
      if (!r.ok) {
        box.checked = !wantOn;
        heroMsg.innerHTML = `<div class="error-text">${ui.esc(r.message)}</div>${
          ui.stepsHtml(r.steps) as string
        }`;
      } else {
        ui.toast("info", wantOn ? "系统代理已开启" : "系统代理已关闭", r.message);
      }
      await ctx.refresh(true);
    } catch (err) {
      box.checked = !wantOn;
      ui.toast("error", "代理操作失败", errText(err));
    }
  });

  (root.querySelector("#pf-new") as HTMLButtonElement).addEventListener("click", () =>
    void profileEditor(null, hosts, sources, ctx),
  );

  // ---- 快照 ----
  const tbody = root.querySelector("#sn-tbody") as HTMLElement;
  (root.querySelector("#sn-empty") as HTMLElement).hidden = snapshots.length > 0;
  for (const s of snapshots) {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td class="mono" style="font-size:12px">${ui.esc(ui.fmtTime(s.created_at))}</td>
      <td>${ui.esc(s.reason)}${s.restored ? ` <span class="pill">已还原过</span>` : ""}</td>
      <td>${ui.esc(s.proxy_state)}</td>
      <td>${s.dns_count}</td>
      <td class="mono">${s.hosts_bytes} B</td>
      <td class="actions">
        <button class="link" data-act="diff">差异</button>
        <button class="link" data-act="restore">还原</button>
      </td>`;
    (tr.querySelector('[data-act="diff"]') as HTMLButtonElement).addEventListener("click", async () => {
      try {
        const d = await api.snapshotsDiff(s.id);
        const box = root.querySelector("#sn-diff") as HTMLElement;
        box.innerHTML = d.identical
          ? `<div class="card"><h2>差异（${ui.esc(ui.fmtTime(d.created_at))}）</h2><div class="ok-text">当前系统状态与该快照一致，无需还原。</div></div>`
          : `<div class="card"><h2>差异（${ui.esc(ui.fmtTime(d.created_at))}）</h2>
              <table><thead><tr><th>层</th><th>字段</th><th>快照</th><th>当前</th></tr></thead>
              <tbody>${d.items
                .map(
                  (i) =>
                    `<tr><td>${ui.esc(i.kind)}</td><td>${ui.esc(i.field)}</td><td class="mono">${ui.esc(
                      i.before,
                    )}</td><td class="mono">${ui.esc(i.after)}</td></tr>`,
                )
                .join("")}</tbody></table></div>`;
      } catch (e) {
        ui.toast("error", "读取差异失败", errText(e));
      }
    });
    (tr.querySelector('[data-act="restore"]') as HTMLButtonElement).addEventListener("click", async () => {
      const yes = await ui.confirmModal({
        title: "还原到该快照？",
        body: "hosts 会被逐字节还原，代理与 DNS 也会恢复到该时刻的状态。\n（hosts / DNS 的还原需要一次管理员授权）",
        confirmText: "还原",
        danger: true,
      });
      if (!yes) return;
      try {
        const r = await api.rollback(s.id);
        ui.showReport(r, "还原结果");
        await ctx.refresh(true);
      } catch (e) {
        ui.toast("error", "还原失败", errText(e));
      }
    });
    tbody.appendChild(tr);
  }

  (root.querySelector("#sn-open") as HTMLButtonElement).addEventListener("click", () =>
    void api.systemOpenDir("snapshots"),
  );
  (root.querySelector("#sn-hostsbak") as HTMLButtonElement).addEventListener("click", () =>
    void api.systemOpenDir("hosts"),
  );
  void snapshots;
}

function presetName(id: string): string {
  const map: Record<string, string> = {
    ali: "阿里 AliDNS",
    dnspod: "腾讯 DNSPod",
    baidu: "百度 DNS",
    cloudflare: "Cloudflare",
    google: "Google",
    quad9: "Quad9",
    adguard: "AdGuard",
    opendns: "OpenDNS",
  };
  return map[id] ?? id;
}

// ---------------- 方案编辑器 ----------------

async function profileEditor(
  p: Profile | null,
  hosts: HostsView,
  sources: Source[],
  ctx: TabCtx,
): Promise<void> {
  let presets: DnsPreset[] = [];
  let interfaces: import("../api").DnsInterfaceView[] = [];
  try {
    [presets, interfaces] = await Promise.all([api.dnsPresets(""), api.dnsInterfaces()]);
  } catch {
    /* 网卡读取失败时仍允许编辑其他项 */
  }

  const draft: Profile =
    p ??
    {
      id: "",
      name: "",
      is_builtin: false,
      note: "",
      hosts: { enabled: true, entry_ids: [], source_ids: [] },
      proxy: { enabled: false, host: "127.0.0.1", port: 7890, bypass: "localhost;127.*;<local>" },
      dns: {
        enabled: false,
        mode: "dhcp",
        // 新建方案默认落点 = 当前正在使用的网卡（物理 + 已连接 + 有默认网关）
        interface_alias: interfaces.find((i) => i.is_in_use)?.alias ?? interfaces[0]?.alias ?? "",
        preset_id: null,
        v4: [],
        v6: [],
        enable_v4: true,
        enable_v6: false,
      },
    };

  const { box, close } = ui.openModal(p ? `编辑方案：${p.name}` : "新建方案");
  box.innerHTML += `
    <div class="field-grid">
      <span>名称</span><input type="text" id="pe-name" value="${ui.esc(draft.name)}" />
      <span>备注</span><input type="text" id="pe-note" value="${ui.esc(draft.note)}" />
    </div>

    <div class="section-title">Hosts（托管区块）</div>
    <div class="actions">
      <label class="row"><input type="checkbox" id="pe-hosts-en" ${
        draft.hosts.enabled ? "checked" : ""
      } /> 该方案管理 hosts 托管区块</label>
      <span class="muted">共 ${hosts.entries.length} 条手工条目可选</span>
      <button id="pe-hosts-all">全选</button>
      <button id="pe-hosts-none">全不选</button>
    </div>
    <div class="table-wrap">
      <table><tbody id="pe-hosts-body"></tbody></table>
    </div>
    ${sources.length ? `<div style="margin-top:8px"><span class="muted">订阅源：</span></div>` : ""}
    <div id="pe-sources"></div>

    <div class="section-title">系统代理</div>
    <div class="actions">
      <label class="row"><input type="checkbox" id="pe-proxy-en" ${
        draft.proxy.enabled ? "checked" : ""
      } /> 切换时开启系统代理</label>
    </div>
    <div class="field-grid">
      <span>地址</span><input type="text" id="pe-proxy-host" value="${ui.esc(draft.proxy.host)}" />
      <span>端口</span><input type="number" id="pe-proxy-port" value="${draft.proxy.port}" min="1" max="65535" style="width:120px" />
      <span>绕过列表</span><input type="text" id="pe-proxy-bypass" value="${ui.esc(draft.proxy.bypass)}" />
    </div>

    <div class="section-title">系统 DNS</div>
    <div class="actions">
      <label class="row"><input type="checkbox" id="pe-dns-en" ${
        draft.dns.enabled ? "checked" : ""
      } /> 该方案管理 DNS</label>
      <label class="row">模式
        <select id="pe-dns-mode">
          <option value="dhcp" ${draft.dns.mode === "dhcp" ? "selected" : ""}>还原为自动获取</option>
          <option value="manual" ${draft.dns.mode === "manual" ? "selected" : ""}>手动指定</option>
        </select>
      </label>
    </div>
    <div class="field-grid">
      <span>网卡</span>
      <select id="pe-dns-alias">
        ${interfaces
          .map(
            (i) =>
              `<option value="${ui.esc(i.alias)}" ${
                i.alias === draft.dns.interface_alias ? "selected" : ""
              }>${ui.esc(i.alias)}${
                i.is_in_use ? "（当前使用）" : i.is_physical ? "" : "（虚拟）"
              }</option>`,
          )
          .join("")}
      </select>
      <span>服务商</span>
      <select id="pe-dns-preset">
        <option value="">（自定义）</option>
        ${presets
          .map(
            (x) =>
              `<option value="${ui.esc(x.id)}" ${
                x.id === draft.dns.preset_id ? "selected" : ""
              }>${ui.esc(x.name)}</option>`,
          )
          .join("")}
      </select>
      <span>IPv4</span>
      <label class="row"><input type="checkbox" id="pe-dns-v4-en" ${
        draft.dns.enable_v4 ? "checked" : ""
      } /> <input type="text" id="pe-dns-v4" class="mono" style="flex:1" value="${ui.esc(
        draft.dns.v4.join(", "),
      )}" /></label>
      <span>IPv6</span>
      <label class="row"><input type="checkbox" id="pe-dns-v6-en" ${
        draft.dns.enable_v6 ? "checked" : ""
      } /> <input type="text" id="pe-dns-v6" class="mono" style="flex:1" value="${ui.esc(
        draft.dns.v6.join(", "),
      )}" /></label>
    </div>
    <p class="hint">取消勾选某地址族 = 切换时该族还原为自动获取（不留隐性残留）。</p>
    <div id="pe-err" class="error-text"></div>
    <div class="modal-actions"><button id="pe-cancel">取消</button><button class="primary" id="pe-save">保存</button></div>
  `;

  const q = <T extends HTMLElement>(sel: string) => box.querySelector(sel) as T;
  const selectedHosts = new Set(draft.hosts.entry_ids);
  const selectedSources = new Set(draft.hosts.source_ids);

  const body = q<HTMLElement>("#pe-hosts-body");
  for (const e of hosts.entries) {
    const tr = document.createElement("tr");
    tr.innerHTML = `<td style="width:32px"><input type="checkbox" ${
      selectedHosts.has(e.id) ? "checked" : ""
    } /></td><td class="mono" style="width:170px">${ui.esc(e.ip)}</td><td class="mono">${ui.esc(
      e.hostname,
    )}</td><td class="faint">${ui.esc(e.comment)}</td>`;
    (tr.querySelector("input") as HTMLInputElement).addEventListener("change", (ev) => {
      if ((ev.target as HTMLInputElement).checked) selectedHosts.add(e.id);
      else selectedHosts.delete(e.id);
    });
    body.appendChild(tr);
  }
  q<HTMLButtonElement>("#pe-hosts-all").addEventListener("click", () => {
    hosts.entries.forEach((e) => selectedHosts.add(e.id));
    box.querySelectorAll<HTMLInputElement>("#pe-hosts-body input").forEach((i) => {
      i.checked = true;
    });
  });
  q<HTMLButtonElement>("#pe-hosts-none").addEventListener("click", () => {
    selectedHosts.clear();
    box.querySelectorAll<HTMLInputElement>("#pe-hosts-body input").forEach((i) => {
      i.checked = false;
    });
  });

  const srcBox = q<HTMLElement>("#pe-sources");
  for (const s of sources) {
    const label = document.createElement("label");
    label.className = "row";
    label.innerHTML = `<input type="checkbox" ${
      selectedSources.has(s.id) ? "checked" : ""
    } /> <span>${ui.esc(s.name)}</span> <span class="faint">（间隔 ${s.interval_hours}h）</span>`;
    (label.querySelector("input") as HTMLInputElement).addEventListener("change", (ev) => {
      if ((ev.target as HTMLInputElement).checked) selectedSources.add(s.id);
      else selectedSources.delete(s.id);
    });
    srcBox.appendChild(label);
  }

  const presetSel = q<HTMLSelectElement>("#pe-dns-preset");
  presetSel.addEventListener("change", () => {
    const found = presets.find((x) => x.id === presetSel.value);
    if (found) {
      q<HTMLInputElement>("#pe-dns-v4").value = found.v4.join(", ");
      q<HTMLInputElement>("#pe-dns-v6").value = found.v6.join(", ");
      q<HTMLInputElement>("#pe-dns-v4-en").checked = found.v4.length > 0;
      q<HTMLInputElement>("#pe-dns-v6-en").checked = found.v6.length > 0;
      q<HTMLSelectElement>("#pe-dns-mode").value = "manual";
    }
  });

  q<HTMLButtonElement>("#pe-cancel").addEventListener("click", close);
  q<HTMLButtonElement>("#pe-save").addEventListener("click", async () => {
    const split = (s: string) =>
      s
        .split(/[,，\s]+/)
        .map((x) => x.trim())
        .filter(Boolean);
    const next: Profile = {
      ...draft,
      name: q<HTMLInputElement>("#pe-name").value.trim(),
      note: q<HTMLInputElement>("#pe-note").value.trim(),
      hosts: {
        enabled: q<HTMLInputElement>("#pe-hosts-en").checked,
        entry_ids: [...selectedHosts],
        source_ids: [...selectedSources],
      },
      proxy: {
        enabled: q<HTMLInputElement>("#pe-proxy-en").checked,
        host: q<HTMLInputElement>("#pe-proxy-host").value.trim(),
        port: Number(q<HTMLInputElement>("#pe-proxy-port").value || 0),
        bypass: q<HTMLInputElement>("#pe-proxy-bypass").value.trim(),
      },
      dns: {
        enabled: q<HTMLInputElement>("#pe-dns-en").checked,
        mode: q<HTMLSelectElement>("#pe-dns-mode").value,
        interface_alias: q<HTMLSelectElement>("#pe-dns-alias").value,
        preset_id: presetSel.value || null,
        v4: split(q<HTMLInputElement>("#pe-dns-v4").value),
        v6: split(q<HTMLInputElement>("#pe-dns-v6").value),
        enable_v4: q<HTMLInputElement>("#pe-dns-v4-en").checked,
        enable_v6: q<HTMLInputElement>("#pe-dns-v6-en").checked,
      },
    };
    if (!next.name) {
      q<HTMLElement>("#pe-err").textContent = "方案名称不能为空";
      return;
    }
    try {
      await api.profileSave(next);
      close();
      await ctx.refresh(true);
      ui.toast("info", "方案已保存");
    } catch (e) {
      q<HTMLElement>("#pe-err").textContent = errText(e);
    }
  });
  void ctx.state;
}
