// 日志标签页（设计方案 §14）

import { api, errText, type LogRecord } from "../api";
import type { TabCtx } from "../main";
import * as ui from "../ui";

let levelFilter = "info";
let query = "";
let timer: number | null = null;

export async function renderLogs(root: HTMLElement, ctx: TabCtx): Promise<void> {
  root.innerHTML = `
    <div class="card">
      <div class="row-between">
        <div>
          <h2>运行日志</h2>
          <p class="hint">按天切分，保留最近 ${ctx.state.settings.log_keep_days} 天；默认脱敏（代理主机、内网地址、用户目录）。</p>
        </div>
        <div class="actions">
          <select id="lg-level">
            <option value="error">error</option>
            <option value="warn">warn</option>
            <option value="info">info</option>
            <option value="debug">debug</option>
          </select>
          <input type="text" id="lg-query" placeholder="过滤关键字…" style="width:180px" />
          <label class="row"><input type="checkbox" id="lg-auto" /> 自动刷新</label>
          <button id="lg-refresh">刷新</button>
          <button id="lg-open">打开日志目录</button>
          <button id="lg-export">导出诊断包</button>
        </div>
      </div>
      <div class="log-view" id="lg-view"></div>
      <div class="muted" id="lg-stat" style="margin-top:6px"></div>
    </div>

    <div class="card">
      <h2>必记事件说明</h2>
      <p class="hint">这些事件被强制记录，用于事后回答"到底是谁改了什么"。</p>
      <table>
        <thead><tr><th style="width:180px">事件</th><th>记录内容</th></tr></thead>
        <tbody>
          <tr><td>hosts 写入</td><td>条目数、写入前/后文件哈希、是否走了回退路径、耗时</td></tr>
          <tr><td>代理变更</td><td>变更前后 ProxyEnable / ProxyServer / AutoConfigURL（脱敏）、是否探测、探测结果</td></tr>
          <tr><td>代理彻底清零</td><td>清除前的完整值（脱敏）、是否被撤销、撤销时间</td></tr>
          <tr><td>DNS 变更</td><td>网卡别名、地址族、写入地址（脱敏）、回读结果</td></tr>
          <tr><td>提权任务</td><td>task 名、退出码、耗时；只记载荷/结果文件路径，不记内容</td></tr>
          <tr><td>热键注册 / 释放</td><td>组合、成功或失败、错误码</td></tr>
          <tr><td>订阅同步</td><td>源、HTTP 状态、条目数变化、是否采用</td></tr>
          <tr><td>回滚 / Recovery</td><td>失败步骤、原始错误码、回滚结果</td></tr>
        </tbody>
      </table>
    </div>
  `;

  const view = root.querySelector("#lg-view") as HTMLElement;
  const stat = root.querySelector("#lg-stat") as HTMLElement;
  const levelSel = root.querySelector("#lg-level") as HTMLSelectElement;
  const queryInput = root.querySelector("#lg-query") as HTMLInputElement;
  const autoBox = root.querySelector("#lg-auto") as HTMLInputElement;
  levelSel.value = levelFilter;
  queryInput.value = query;

  const order = ["error", "warn", "info", "debug"];
  const refresh = async () => {
    try {
      const all: LogRecord[] = await api.logsRecent(600);
      const min = order.indexOf(levelFilter);
      const filtered = all.filter(
        (r) =>
          order.indexOf(r.level.toLowerCase()) <= min &&
          (!query ||
            r.message.toLowerCase().includes(query.toLowerCase()) ||
            r.target.toLowerCase().includes(query.toLowerCase())),
      );
      view.innerHTML =
        filtered
          .map(
            (r) =>
              `<div class="log-line"><span>${ui.esc(r.ts)}</span><span class="lv-${ui.esc(
                r.level.toLowerCase(),
              )}">${ui.esc(r.level)}</span><span>${ui.esc(r.message)}</span></div>`,
          )
          .join("") ||
        `<div class="empty">${ui.iconEl("logs", 22)}<div>没有匹配的日志</div>
         <div class="faint">换个关键字，或把级别调到 debug</div></div>`;
      view.scrollTop = view.scrollHeight;
      stat.textContent = `共 ${all.length} 条，显示 ${filtered.length} 条（点"打开日志目录"可看完整文件）`;
    } catch (e) {
      view.innerHTML = `<div class="error-text">${ui.esc(errText(e))}</div>`;
    }
  };

  await refresh();

  levelSel.addEventListener("change", () => {
    levelFilter = levelSel.value;
    void refresh();
  });
  queryInput.addEventListener("input", () => {
    query = queryInput.value;
    void refresh();
  });
  root.querySelector("#lg-refresh")?.addEventListener("click", () => void refresh());
  root.querySelector("#lg-open")?.addEventListener("click", () => void api.systemOpenDir("logs"));
  root.querySelector("#lg-export")?.addEventListener("click", async () => {
    const yes = await ui.confirmModal({
      title: "导出诊断包",
      body: "包含最近 3 天日志、当前三层状态、环境信息；代理主机名与内网地址会脱敏。",
      confirmText: "导出",
    });
    if (!yes) return;
    try {
      const p = await api.logsExport();
      ui.toast("info", "诊断包已生成", p);
    } catch (e) {
      ui.toast("error", "导出失败", errText(e));
    }
  });

  autoBox.addEventListener("change", () => {
    if (timer) {
      window.clearInterval(timer);
      timer = null;
    }
    if (autoBox.checked) {
      timer = window.setInterval(() => void refresh(), 2000);
    }
  });

  // 离开标签页时停掉定时器
  const observer = new MutationObserver(() => {
    if (!document.body.contains(root)) {
      if (timer) {
        window.clearInterval(timer);
        timer = null;
      }
      observer.disconnect();
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
}
