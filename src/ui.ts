// DOM 工具、弹层、提示（界面交互约束见设计方案 §9）

import { api, type ApplyReport, type ApplyStep } from "./api";
import { icon, type IconName } from "./icons";
import * as os from "./platform";

export function esc(s: unknown): string {
  return String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

export function clear(node: HTMLElement): void {
  while (node.firstChild) node.removeChild(node.firstChild);
}

export function iconEl(name: IconName, size?: number): string {
  return `<span class="i">${icon[name](size)}</span>`;
}

// ---------------- 提示 ----------------

export interface ToastAction {
  label: string;
  run: () => void;
}

const LEVEL_ICON: Record<string, string> = {
  info: "check",
  warn: "warn",
  error: "warn",
};

export function toast(
  level: "info" | "warn" | "error",
  title: string,
  body = "",
  actions: ToastAction[] = [],
  timeoutMs = 0,
): void {
  const root = document.getElementById("toasts");
  if (!root) return;
  const box = document.createElement("div");
  box.className = `toast ${level}`;
  box.setAttribute("role", level === "error" ? "alert" : "status");

  const main = document.createElement("div");
  main.innerHTML = `
    <div class="toast-title">${iconEl(LEVEL_ICON[level] as "check")}${esc(title)}</div>
    ${body ? `<div class="toast-body">${esc(body)}</div>` : ""}`;
  box.appendChild(main);

  const close = document.createElement("button");
  close.className = "icon-btn sm";
  close.title = "关闭";
  close.innerHTML = iconEl("close", 13);
  close.style.height = "20px";
  close.style.width = "20px";
  box.appendChild(close);

  const leave = () => {
    box.classList.add("leaving");
    window.setTimeout(() => box.remove(), 200);
  };
  close.addEventListener("click", leave);

  if (actions.length) {
    const bar = document.createElement("div");
    bar.className = "actions";
    bar.style.marginTop = "8px";
    bar.style.gridColumn = "1 / -1";
    for (const a of actions) {
      const btn = document.createElement("button");
      btn.className = "sm";
      btn.textContent = a.label;
      btn.addEventListener("click", () => {
        leave();
        a.run();
      });
      bar.appendChild(btn);
    }
    box.appendChild(bar);
  }

  root.appendChild(box);
  const ttl = timeoutMs || (level === "error" ? 9000 : level === "warn" ? 7000 : 4000);
  window.setTimeout(leave, ttl);
}

// ---------------- 顶部横幅（气泡不可用时的降级承载） ----------------

export function banner(
  level: "info" | "warn" | "error",
  text: string,
  actions: ToastAction[] = [],
  sticky = false,
): HTMLElement {
  const root = document.getElementById("banners")!;
  const box = document.createElement("div");
  box.className = `banner ${level}`;
  box.innerHTML = `<span class="i">${
    icon[LEVEL_ICON[level] as "warn"](16)
  }</span>`;
  const grow = document.createElement("span");
  grow.className = "grow";
  grow.textContent = text;
  box.appendChild(grow);
  for (const a of actions) {
    const btn = document.createElement("button");
    btn.className = "sm";
    btn.textContent = a.label;
    btn.addEventListener("click", () => {
      box.remove();
      a.run();
    });
    box.appendChild(btn);
  }
  const close = document.createElement("button");
  close.className = "ghost sm";
  close.innerHTML = iconEl("close", 13);
  close.title = "关闭";
  close.addEventListener("click", () => box.remove());
  box.appendChild(close);
  root.appendChild(box);
  if (!sticky) window.setTimeout(() => box.remove(), 12000);
  return box;
}

// ---------------- 弹层 ----------------

export interface ConfirmOptions {
  title: string;
  body: string;
  confirmText?: string;
  cancelText?: string;
  danger?: boolean;
  /** 危险操作的二次确认：可要求输入指定文本来启用确认按钮 */
  typedWord?: string;
}

export function confirmModal(opts: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    const root = document.getElementById("modal-root")!;
    const back = document.createElement("div");
    back.className = "backdrop";
    const danger = !!opts.danger;
    back.innerHTML = `
      <div class="modal" role="dialog" aria-modal="true">
        <h3>${opts.danger ? iconEl("warn", 17) : iconEl("shield", 17)}${esc(opts.title)}</h3>
        <div class="muted" style="white-space:pre-wrap">${esc(opts.body)}</div>
        ${
          opts.typedWord
            ? `<div style="margin-top:12px"><label class="row">请输入 <code>${esc(
                opts.typedWord,
              )}</code> 以确认：<input type="text" id="typed-confirm" style="flex:1" /></label></div>`
            : ""
        }
        <div class="modal-actions">
          <button id="m-cancel">${esc(opts.cancelText ?? "取消")}</button>
          <button id="m-ok" class="${danger ? "danger" : "primary"}" ${
            opts.typedWord ? "disabled" : ""
          }>${esc(opts.confirmText ?? "确定")}</button>
        </div>
      </div>`;
    root.appendChild(back);

    const ok = back.querySelector<HTMLButtonElement>("#m-ok")!;
    const cancel = back.querySelector<HTMLButtonElement>("#m-cancel")!;
    const typed = back.querySelector<HTMLInputElement>("#typed-confirm");

    // 默认焦点**不落在**危险按钮上（§9 交互约束）
    if (typed) {
      setTimeout(() => typed.focus(), 0);
      typed.addEventListener("input", () => {
        ok.disabled = typed.value.trim() !== opts.typedWord;
      });
    } else {
      setTimeout(() => cancel.focus(), 0);
    }

    const done = (v: boolean) => {
      back.remove();
      document.removeEventListener("keydown", onKey);
      resolve(v);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") done(false);
      if (e.key === "Enter" && !ok.disabled) done(true);
    };
    document.addEventListener("keydown", onKey);
    ok.addEventListener("click", () => done(true));
    cancel.addEventListener("click", () => done(false));
    back.addEventListener("click", (e) => {
      if (e.target === back) done(false);
    });
  });
}

/** 自绘弹窗容器：返回 { box, close } */
export function openModal(title: string, iconName: IconName = "gear"): { box: HTMLElement; close: () => void } {
  const root = document.getElementById("modal-root")!;
  const back = document.createElement("div");
  back.className = "backdrop";
  const box = document.createElement("div");
  box.className = "modal";
  box.setAttribute("role", "dialog");
  box.setAttribute("aria-modal", "true");
  const h = document.createElement("h3");
  h.innerHTML = `${iconEl(iconName, 17)}${esc(title)}`;
  box.appendChild(h);
  back.appendChild(box);
  root.appendChild(back);
  const close = () => back.remove();
  back.addEventListener("click", (e) => {
    if (e.target === back) close();
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      close();
      document.removeEventListener("keydown", onKey);
    }
  };
  document.addEventListener("keydown", onKey);
  return { box, close };
}

// ---------------- 事务报告 ----------------

export function stepsHtml(steps: ApplyStep[]): string {
  if (!steps.length) return `<div class="muted">没有步骤</div>`;
  return `<ul class="steps">${steps
    .map((s) => {
      const mark = s.applied ? (s.verified ? "✅" : "⚠️") : "❌";
      return `<li><span class="kind">${esc(s.kind)}</span><span>${mark} ${esc(
        s.message,
      )}</span></li>`;
    })
    .join("")}</ul>`;
}

export function reportBody(report: ApplyReport): string {
  const lines: string[] = [];
  lines.push(`结果：${report.ok ? "成功" : "失败"}`);
  if (report.rolled_back) lines.push("已回滚到操作前状态");
  if (report.recovery_required)
    lines.push("回滚未完全成功 —— 需要人工介入，请查看差异清单与备份目录");
  lines.push("");
  return lines.join("\n");
}

/** 统一展示 ApplyReport：步骤逐项如实展示，不吞错误 */
export function showReport(report: ApplyReport, title = "操作结果"): void {
  const { box, close } = openModal(title, report.ok ? "check" : "warn");
  const pre = document.createElement("div");
  pre.className = "muted";
  pre.style.whiteSpace = "pre-wrap";
  pre.textContent = reportBody(report);
  box.appendChild(pre);
  const list = document.createElement("div");
  list.style.marginTop = "12px";
  list.innerHTML = stepsHtml(report.steps);
  box.appendChild(list);
  const bar = document.createElement("div");
  bar.className = "modal-actions";
  if (report.recovery_required) {
    const open = document.createElement("button");
    open.innerHTML = `${iconEl("folder")}打开备份目录`;
    open.addEventListener("click", () => void api.systemOpenDir("backup"));
    bar.appendChild(open);
  }
  const ok = document.createElement("button");
  ok.className = "primary";
  ok.textContent = "关闭";
  ok.addEventListener("click", close);
  bar.appendChild(ok);
  box.appendChild(bar);
  setTimeout(() => ok.focus(), 0);
}

// ---------------- 提权前置说明（§1.4.4 ③） ----------------

let elevatedCache: boolean | null = null;

export function setElevated(v: boolean): void {
  elevatedCache = v;
}

export function isElevated(): boolean | null {
  return elevatedCache;
}

/**
 * 授权提示前先自绘说明对话框：
 * Windows 的 UAC 框只显示程序名 SuNet.exe、macOS 的授权框只显示程序名，
 * 用户可能不知道这弹窗是自己刚才点的操作触发的。
 */
export async function elevationNotice(action: string, enabled: boolean): Promise<boolean> {
  if (!enabled || elevatedCache === true) return true;
  return confirmModal({
    title: "需要管理员权限",
    body: `${action}

${os.AUTH_BODY}`,
    confirmText: os.AUTH_CONTINUE,
  });
}

export function fmtTime(iso?: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("zh-CN", { hour12: false });
}

/** 按钮忙碌态包装：防重复点击 + 视觉反馈 */
export async function withBusy<T>(
  btn: HTMLButtonElement | null,
  fn: () => Promise<T>,
): Promise<T> {
  if (btn) {
    btn.setAttribute("aria-busy", "true");
    btn.disabled = true;
  }
  try {
    return await fn();
  } finally {
    if (btn) {
      btn.removeAttribute("aria-busy");
      btn.disabled = false;
    }
  }
}
