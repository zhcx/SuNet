// Hosts 标签页（设计方案 §2）

import { api, errText, type HostsEntry, type HostsView, type Source, type StagedChange } from "../api";
import type { TabCtx } from "../main";
import * as ui from "../ui";

export async function renderHosts(root: HTMLElement, ctx: TabCtx): Promise<void> {
  const [view, sources] = await Promise.all([api.hostsList(), api.sourcesList()]);
  const readonly = !!ctx.state.read_only;

  root.innerHTML = `
    <div class="card">
      <div class="row-between">
        <div>
          <h2>托管区块条目</h2>
          <p class="hint">只操作 <code># &gt;&gt;&gt; SuNet BEGIN … # &lt;&lt;&lt; SuNet END</code> 之间的内容，区块外你手写的内容永远不会被改动。</p>
        </div>
        <div class="actions">
          <button id="hs-preview">预览写入内容</button>
          <button id="hs-import">导入现有区块</button>
          <button class="primary" id="hs-apply" ${readonly ? "disabled" : ""}>写入系统 hosts</button>
        </div>
      </div>

      <div class="kv"><span class="k">文件</span><span class="mono">${ui.esc(view.stats.path)}</span></div>
      <div class="kv"><span class="k">状态</span><span id="hs-status"></span></div>
      ${
        view.duplicates.length
          ? `<div class="warn-text" style="margin-top:6px">存在重复条目，首条生效：${ui.esc(
              view.duplicates.slice(0, 12).join("、"),
            )}${view.duplicates.length > 12 ? " …" : ""}</div>`
          : ""
      }

      <div class="section-title">新增 / 编辑</div>
      <div class="actions">
        <input type="text" id="hs-ip" placeholder="IP（1.1.1.1 或 2606:4700::1111）" style="width:220px" class="mono" />
        <input type="text" id="hs-host" placeholder="域名（多个用空格分隔）" style="width:260px" class="mono" />
        <input type="text" id="hs-comment" placeholder="注释（可选）" style="width:180px" />
        <button class="primary" id="hs-add" ${readonly ? "disabled" : ""}>添加</button>
        <button id="hs-batch" ${readonly ? "disabled" : ""}>批量屏蔽…</button>
        <button class="danger" id="hs-clear" ${readonly ? "disabled" : ""}>清空托管区块</button>
      </div>
      <div id="hs-err" class="error-text" style="margin-top:6px"></div>

      <div class="table-wrap" style="margin-top:12px">
        <table>
          <thead>
            <tr>
              <th style="width:64px">启用</th>
              <th style="width:200px">IP</th>
              <th>域名</th>
              <th style="width:180px">注释</th>
              <th style="width:110px">来源</th>
              <th style="width:110px"></th>
            </tr>
          </thead>
          <tbody id="hs-tbody"></tbody>
        </table>
      </div>
      <div id="hs-empty" class="empty" hidden>
        ${ui.iconEl("hosts", 22)}
        <div>还没有托管条目</div>
        <div class="faint">可以手工添加一条，或添加下方远程订阅后同步</div>
      </div>
    </div>

    <div class="card">
      <div class="row-between">
        <div>
          <h2>远程订阅</h2>
          <p class="hint">订阅内容是不可信输入：只接受 https，逐行校验，首次导入或变更超过 20% 时必须先看 diff 再写入。</p>
        </div>
        <div class="actions">
          <button id="src-sync-all">全部同步</button>
          <button id="src-add">新增订阅源</button>
        </div>
      </div>
      <div id="src-list"></div>
      <div id="src-diff"></div>
    </div>

    <div class="card">
      <h2>诊断</h2>
      <p class="hint">区分「命中 hosts」与「走 DNS」——改了没反应，多半是 hosts 里已经有这条。</p>
      <div class="actions">
        <input type="text" id="hs-resolve-host" placeholder="example.com" style="width:280px" />
        <button id="hs-resolve">解析诊断</button>
      </div>
      <div id="hs-resolve-out" style="margin-top:10px"></div>
    </div>
  `;

  const status = root.querySelector("#hs-status") as HTMLElement;
  status.innerHTML = `${view.stats.exists ? "文件存在" : "文件不存在"} · 启用 ${view.stats.enabled_entries} 条 / 共 ${view.stats.total_entries} 条 · ${
    view.pending
      ? `<span class="warn-text">草稿与系统文件不一致，需要点「写入系统 hosts」</span>`
      : `<span class="ok-text">已与系统文件同步</span>`
  }`;

  renderTable(root, view, ctx);

  const errBox = root.querySelector("#hs-err") as HTMLElement;
  const showErr = (e: unknown) => {
    errBox.textContent = errText(e);
  };
  const clearErr = () => (errBox.textContent = "");

  (root.querySelector("#hs-add") as HTMLButtonElement).addEventListener("click", async () => {
    clearErr();
    const ip = (root.querySelector("#hs-ip") as HTMLInputElement).value.trim();
    const hostname = (root.querySelector("#hs-host") as HTMLInputElement).value.trim();
    const comment = (root.querySelector("#hs-comment") as HTMLInputElement).value.trim();
    if (!ip || !hostname) {
      showErr("请填写 IP 与域名");
      return;
    }
    try {
      await api.hostsUpsert({
        id: "",
        ip,
        hostname,
        enabled: true,
        comment,
        origin: "manual",
      });
      await ctx.refresh(true);
      ui.toast("info", "已加入草稿", "点「写入系统 hosts」后生效");
    } catch (e) {
      showErr(e);
    }
  });

  (root.querySelector("#hs-batch") as HTMLButtonElement).addEventListener("click", async () => {
    clearErr();
    const raw = (root.querySelector("#hs-host") as HTMLInputElement).value.trim();
    if (!raw) {
      showErr("请先在域名输入框里填入要屏蔽的域名（空格分隔）");
      return;
    }
    const domains = raw.split(/\s+/).filter(Boolean);
    const yes = await ui.confirmModal({
      title: "批量屏蔽",
      body: `将把以下 ${domains.length} 个域名指向 0.0.0.0（屏蔽）：\n\n${domains.join("\n")}\n\n屏蔽是静默生效的，请确认清单无误。`,
      confirmText: "确认屏蔽",
      danger: true,
    });
    if (!yes) return;
    try {
      await api.hostsBatchBlock(domains);
      await ctx.refresh(true);
    } catch (e) {
      showErr(e);
    }
  });

  (root.querySelector("#hs-clear") as HTMLButtonElement).addEventListener("click", async () => {
    const yes = await ui.confirmModal({
      title: "清空托管区块？",
      body: "会删除所有托管条目（配置草稿也一并清空）。只有在你随后点击「写入系统 hosts」后才会真正从文件里移除。",
      confirmText: "清空",
      danger: true,
      typedWord: "清空",
    });
    if (!yes) return;
    try {
      await api.hostsClear();
      await ctx.refresh(true);
    } catch (e) {
      showErr(e);
    }
  });

  (root.querySelector("#hs-preview") as HTMLButtonElement).addEventListener("click", async () => {
    try {
      const text = await api.hostsPreview(view.entries);
      const { box, close } = ui.openModal("将要写入的托管区块");
      const pre = document.createElement("pre");
      pre.textContent = text || "（空区块：写入后托管区块内容为空）";
      box.appendChild(pre);
      const bar = document.createElement("div");
      bar.className = "modal-actions";
      const ok = document.createElement("button");
      ok.className = "primary";
      ok.textContent = "关闭";
      ok.addEventListener("click", close);
      bar.appendChild(ok);
      box.appendChild(bar);
    } catch (e) {
      ui.toast("error", "预览失败", errText(e));
    }
  });

  (root.querySelector("#hs-import") as HTMLButtonElement).addEventListener("click", async () => {
    try {
      const v = await api.hostsImport();
      await ctx.refresh(true);
      ui.toast("info", "已导入", `当前草稿 ${v.entries.length} 条`);
    } catch (e) {
      showErr(e);
    }
  });

  (root.querySelector("#hs-apply") as HTMLButtonElement).addEventListener("click", async () => {
    clearErr();
    const ok = await ui.elevationNotice("即将写入系统 hosts 文件", ctx.state.settings.confirm_elevation_notice);
    if (!ok) return;
    try {
      const r = await api.hostsApply();
      if (r.ok) {
        ui.toast("info", "hosts 已写入", r.message);
      } else {
        ui.showReport(r, "hosts 写入结果");
      }
      await ctx.refresh(true);
    } catch (e) {
      showErr(e);
      ui.toast("error", "hosts 写入失败", errText(e));
    }
  });

  (root.querySelector("#hs-resolve") as HTMLButtonElement).addEventListener("click", async () => {
    const host = (root.querySelector("#hs-resolve-host") as HTMLInputElement).value.trim();
    if (!host) return;
    const out = root.querySelector("#hs-resolve-out") as HTMLElement;
    out.innerHTML = `<span class="muted">解析中…</span>`;
    try {
      const r = await api.hostsResolve(host);
      out.innerHTML = `
        <div class="kv"><span class="k">来源</span><span>${
          r.source === "hits_hosts" ? "命中 hosts" : "走 DNS 服务器"
        }</span></div>
        <div class="kv"><span class="k">hosts 记录</span><span class="mono">${ui.esc(r.hosts_hit ?? "—")}</span></div>
        <div class="kv"><span class="k">解析结果</span><span class="mono">${ui.esc(r.addresses.join(", ") || "—")}</span></div>
        <div class="kv"><span class="k">443 连通</span><span>${r.ping_ok ? "可以连接" : "不可连接"}</span></div>
        ${r.message ? `<div class="muted">${ui.esc(r.message)}</div>` : ""}
        ${
          r.hosts_hit
            ? `<div class="hint" style="margin-top:6px">该域名已在 hosts 中，因此 DNS 设置对它无效 —— 这是 Windows 的固定优先级，不是 bug。</div>`
            : ""
        }`;
    } catch (e) {
      out.innerHTML = `<span class="error-text">${ui.esc(errText(e))}</span>`;
    }
  });

  void renderSources(root, sources, ctx);
}

function renderTable(root: HTMLElement, view: HostsView, ctx: TabCtx): void {
  const tbody = root.querySelector("#hs-tbody") as HTMLElement;
  const empty = root.querySelector("#hs-empty") as HTMLElement;
  tbody.innerHTML = "";
  empty.hidden = view.entries.length > 0;

  const dupSet = new Set(view.duplicates);
  for (const e of view.entries) {
    const tr = document.createElement("tr");
    const isDup = e.hostname
      .split(/\s+/)
      .some((h) => dupSet.has(h.toLowerCase()));
    if (isDup) tr.className = "dup";
    if (!e.enabled) tr.className = "disabled";
    tr.innerHTML = `
      <td><input type="checkbox" ${e.enabled ? "checked" : ""} /></td>
      <td class="ip">${ui.esc(e.ip)}</td>
      <td class="ip">${ui.esc(e.hostname)}</td>
      <td>${ui.esc(e.comment)}</td>
      <td><span class="pill">${ui.esc(
        e.origin === "manual" || e.origin === "file" ? "手工" : "订阅",
      )}</span></td>
      <td class="actions">
        <button class="link" data-act="edit">编辑</button>
        <button class="link danger" data-act="del">删除</button>
      </td>`;

    (tr.querySelector("input") as HTMLInputElement).addEventListener("change", async (ev) => {
      try {
        await api.hostsUpsert({ ...e, enabled: (ev.target as HTMLInputElement).checked });
        await ctx.refresh(true);
      } catch (err) {
        ui.toast("error", "修改失败", errText(err));
      }
    });
    (tr.querySelector('[data-act="edit"]') as HTMLButtonElement).addEventListener("click", () => {
      void editEntry(e, ctx);
    });
    (tr.querySelector('[data-act="del"]') as HTMLButtonElement).addEventListener("click", async () => {
      try {
        await api.hostsDelete(e.id);
        await ctx.refresh(true);
      } catch (err) {
        ui.toast("error", "删除失败", errText(err));
      }
    });
    tbody.appendChild(tr);
  }
}

async function editEntry(e: HostsEntry, ctx: TabCtx): Promise<void> {
  const { box, close } = ui.openModal("编辑条目");
  box.innerHTML += `
    <div class="field-grid">
      <span>IP</span><input type="text" id="ed-ip" class="mono" value="${ui.esc(e.ip)}" />
      <span>域名</span><input type="text" id="ed-host" class="mono" value="${ui.esc(e.hostname)}" />
      <span>注释</span><input type="text" id="ed-comment" value="${ui.esc(e.comment)}" />
      <span>启用</span><label class="row"><input type="checkbox" id="ed-enabled" ${e.enabled ? "checked" : ""} /></label>
    </div>
    <div id="ed-err" class="error-text" style="margin-top:8px"></div>
    <div class="modal-actions"><button id="ed-cancel">取消</button><button class="primary" id="ed-save">保存</button></div>`;

  box.querySelector("#ed-cancel")?.addEventListener("click", close);
  box.querySelector("#ed-save")?.addEventListener("click", async () => {
    const next: HostsEntry = {
      ...e,
      ip: (box.querySelector("#ed-ip") as HTMLInputElement).value.trim(),
      hostname: (box.querySelector("#ed-host") as HTMLInputElement).value.trim(),
      comment: (box.querySelector("#ed-comment") as HTMLInputElement).value.trim(),
      enabled: (box.querySelector("#ed-enabled") as HTMLInputElement).checked,
    };
    try {
      await api.hostsUpsert(next);
      close();
      await ctx.refresh(true);
    } catch (err) {
      (box.querySelector("#ed-err") as HTMLElement).textContent = errText(err);
    }
  });
}

// ---------------- 订阅 ----------------

async function renderSources(root: HTMLElement, sources: Source[], ctx: TabCtx): Promise<void> {
  const list = root.querySelector("#src-list") as HTMLElement;
  list.innerHTML = sources.length
    ? ""
    : `<div class="empty">${ui.iconEl("download", 22)}<div>还没有订阅源</div>
        <div class="faint">默认已内置 SteamHostSync 公开源的话可直接同步；也可以自己添加</div></div>`;

  for (const s of sources) {
    const box = document.createElement("div");
    box.style.borderBottom = "1px solid var(--border)";
    box.style.padding = "8px 0";
    const statusText =
      s.last_status === "ok"
        ? `已同步（${ui.fmtTime(s.last_sync)}）`
        : s.last_status === "staged"
          ? "有待确认的变更（清单已在下方列出）"
          : s.last_status === "failed"
            ? "同步失败（保留本地内容）"
            : "从未同步";
    box.innerHTML = `
      <div class="row-between">
        <div>
          <div><b>${ui.esc(s.name)}</b> <span class="pill">${
            s.enabled ? "启用" : "停用"
          }</span> <span class="faint mono" style="font-size:12px">${ui.esc(
            s.url.replace(/\?.*$/, "?…"),
          )}</span></div>
          <div class="muted" style="font-size:13px">间隔 ${s.interval_hours} 小时 · ${ui.esc(
            statusText,
          )}${s.mirror ? " · 已配置镜像源" : ""}</div>
        </div>
        <div class="actions">
          <button data-act="sync">同步</button>
          <button data-act="edit">编辑</button>
          <button class="link danger" data-act="del">删除</button>
        </div>
      </div>`;
    (box.querySelector('[data-act="sync"]') as HTMLButtonElement).addEventListener("click", async () => {
      try {
        const r = await api.sourcesSync(s.id);
        ui.toast(
          r.ok ? "info" : "warn",
          `${s.name}：${r.message}`,
          r.rejected_lines
            ? `已拒绝 ${r.rejected_lines} 行非法内容：${r.rejected_sample.slice(0, 2).join("；")}`
            : "",
        );
        await ctx.refresh(true);
      } catch (e) {
        ui.toast("error", "同步失败", errText(e));
      }
    });
    (box.querySelector('[data-act="edit"]') as HTMLButtonElement).addEventListener("click", () =>
      void sourceEditor(s, ctx),
    );
    (box.querySelector('[data-act="del"]') as HTMLButtonElement).addEventListener("click", async () => {
      const yes = await ui.confirmModal({
        title: "删除订阅源",
        body: `将删除「${s.name}」及其已拉取的条目（下次写入 hosts 时生效）。`,
        confirmText: "删除",
        danger: true,
      });
      if (!yes) return;
      await api.sourcesDelete(s.id);
      await ctx.refresh(true);
    });
    list.appendChild(box);
  }

  (root.querySelector("#src-add") as HTMLButtonElement).addEventListener("click", () =>
    void sourceEditor(null, ctx),
  );
  (root.querySelector("#src-sync-all") as HTMLButtonElement).addEventListener("click", async () => {
    ui.toast("info", "正在同步全部订阅源…");
    try {
      await api.sourcesSyncAll();
      // 变更清单由下方统一的"待确认渲染"展示（refresh 后重走 renderSources）
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "同步失败", errText(e));
    }
  });

  // 无论同步发生在哪里（本页按钮 / 全部同步 / 通知跳转 / 启动时到期自动同步），
  // 只要存在待确认的暂存变更，打开本页就必须能看到确认清单 ——
  // 之前只在"刚点完同步"的那一次渲染里出现，刷新或重启后就再也找不到（2026-10-07 实测反馈）
  try {
    const staged = await api.sourcesStaged();
    for (const sv of staged) {
      if (!sv.staged) continue;
      const src = sources.find((s) => s.id === sv.source_id);
      showDiff(root, sv.source_id, src?.name ?? sv.source_id, sv.staged, ctx);
    }
  } catch {
    /* 读取待确认状态失败不打断页面，同步按钮仍可用 */
  }
}

function sourceEditor(s: Source | null, ctx: TabCtx): void {
  const { box, close } = ui.openModal(s ? "编辑订阅源" : "新增订阅源");
  const src: Source =
    s ??
    {
      id: "",
      name: "",
      url: "https://cdn.jsdelivr.net/gh/Clov614/SteamHostSync@main/Hosts_github",
      mirror: "https://cdn.statically.io/gh/Clov614/SteamHostSync@main/Hosts_github",
      interval_hours: 24,
      enabled: true,
      last_sync: null,
      last_status: "never",
    };
  box.innerHTML += `
    <div class="field-grid">
      <span>名称</span><input type="text" id="sr-name" value="${ui.esc(src.name)}" />
      <span>主源</span><input type="text" id="sr-url" value="${ui.esc(src.url)}" />
      <span>镜像</span><input type="text" id="sr-mirror" value="${ui.esc(src.mirror)}" />
      <span>间隔</span><label class="row"><input type="number" id="sr-interval" value="${src.interval_hours}" min="1" max="720" style="width:80px" /> 小时</label>
      <span>启用</span><label class="row"><input type="checkbox" id="sr-enabled" ${src.enabled ? "checked" : ""} /></label>
    </div>
    <p class="hint">只接受 https 地址；主源不可达时自动切镜像，两源都失败会保留上一次内容而不是清空。</p>
    <div id="sr-err" class="error-text"></div>
    <div class="modal-actions"><button id="sr-cancel">取消</button><button class="primary" id="sr-save">保存</button></div>`;

  box.querySelector("#sr-cancel")?.addEventListener("click", close);
  box.querySelector("#sr-save")?.addEventListener("click", async () => {
    const next: Source = {
      ...src,
      name: (box.querySelector("#sr-name") as HTMLInputElement).value.trim(),
      url: (box.querySelector("#sr-url") as HTMLInputElement).value.trim(),
      mirror: (box.querySelector("#sr-mirror") as HTMLInputElement).value.trim(),
      interval_hours: Number((box.querySelector("#sr-interval") as HTMLInputElement).value) || 24,
      enabled: (box.querySelector("#sr-enabled") as HTMLInputElement).checked,
    };
    try {
      await api.sourcesSave(next);
      close();
      await ctx.refresh(true);
    } catch (e) {
      (box.querySelector("#sr-err") as HTMLElement).textContent = errText(e);
    }
  });
}

function showDiff(
  root: HTMLElement,
  sourceId: string,
  name: string,
  pending: StagedChange,
  ctx: TabCtx,
): void {
  const holder = root.querySelector("#src-diff") as HTMLElement;
  // 同一源的清单只保留一张卡：同步路径与"打开页面自动渲染"可能先后触发
  holder.querySelector(`[data-diff-source="${CSS.escape(sourceId)}"]`)?.remove();
  const box = document.createElement("div");
  box.className = "card";
  box.dataset.diffSource = sourceId;
  box.style.marginTop = "12px";
  const risky = pending.risky_count;
  box.innerHTML = `
    <h2>${ui.esc(name)} 的变更清单</h2>
    <p class="hint">${
      pending.first_import
        ? "首次导入：必须先看清楚要写入什么。"
        : "本次变更超过总量 20%，需要你确认后才写入。"
    }${risky ? ` 其中 <b class="warn-text">${risky}</b> 条命中敏感域名（银行 / 支付 / 政务 / 邮箱 / CA），默认不勾选。` : ""}</p>
    <div class="grid-2">
      <div>
        <div class="section-title">新增 ${pending.added.length}</div>
        <pre>${ui.esc(
          pending.added
            .slice(0, 200)
            .map(
              (e) =>
                `${e.ip} ${e.hostname}${
                  e.hostname
                    .split(/\\s+/)
                    .some((h) => pending.sensitive_added.includes(h.toLowerCase()))
                    ? "  ⚠敏感"
                    : ""
                }`,
            )
            .join("\n") || "（无）",
        )}</pre>
      </div>
      <div>
        <div class="section-title">修改 ${pending.changed.length}</div>
        <pre>${ui.esc(
          pending.changed
            .slice(0, 200)
            .map(
              (c) =>
                `${c.after.hostname}\n  ${c.before.ip} → ${c.after.ip}${c.sensitive ? "  ⚠敏感" : ""}`,
            )
            .join("\n") || "（无）",
        )}</pre>
      </div>
    </div>
    <div class="section-title">删除 ${pending.removed.length}</div>
    <pre>${ui.esc(pending.removed.slice(0, 100).map((e) => e.hostname).join("\n") || "（无）")}</pre>
    <div class="actions" style="margin-top:12px">
      <label class="row"><input type="checkbox" id="diff-risky" /> 同时接受敏感域名的变更（危险）</label>
      <button class="primary" id="diff-commit">确认写入</button>
      <button id="diff-discard">放弃本次变更</button>
    </div>`;
  holder.appendChild(box);

  (box.querySelector("#diff-commit") as HTMLButtonElement).addEventListener("click", async () => {
    const includeSensitive = (box.querySelector("#diff-risky") as HTMLInputElement).checked;
    if (risky && !includeSensitive) {
      const yes = await ui.confirmModal({
        title: "确认继续？",
        body: "有敏感域名的变更未被接受，这些域名将保持原值。确定按当前选择写入吗？",
        confirmText: "写入",
      });
      if (!yes) return;
    }
    try {
      const total = await api.sourcesCommit(sourceId, includeSensitive);
      box.remove();
      ui.toast("info", "已写入订阅条目库", `共 ${total} 条；切方案或点「写入系统 hosts」后生效`);
      await ctx.refresh(true);
    } catch (e) {
      ui.toast("error", "写入失败", errText(e));
    }
  });
  (box.querySelector("#diff-discard") as HTMLButtonElement).addEventListener("click", async () => {
    await api.sourcesDiscard(sourceId);
    box.remove();
    await ctx.refresh(true);
  });
}
