// 网络优化（内嵌 netopt）：巡检结果的界面翻译 + 主窗口弹层
//
// 后端只给结构化数据（`sections[].status` / `findings[]` / `plan[]`），文案统一在
// 这里收口：快捷面板与主窗口共用同一套口径，两边各写一份迟早分叉。
//
// 交互遵守 netopt 自身的约定 —— **先把计划摆给用户看，再由用户点确认**，
// 不在后台"顺手"改配置。

import { api, errText, type NetoptSection, type NetoptView } from "./api";
import * as ui from "./ui";

const STATUS_TEXT: Record<string, string> = {
  ok: "正常",
  warn: "注意",
  fail: "故障",
  unsupported: "不支持",
};

/** `info` 级别的发现不影响状态，也不该占用用户的注意力（见 netopt README） */
const LEVEL_TEXT: Record<string, string> = { warn: "注意", fail: "故障" };

function statusText(status: string): string {
  return STATUS_TEXT[status] ?? status;
}

export interface Issue {
  section: string;
  level: "warn" | "fail";
  message: string;
}

export interface OptimizeSummary {
  /** 一行总览：`1 项注意 · 1 项故障`，全好时是"全部正常" */
  headline: string;
  /** 需要用户知道的问题（warn / fail） */
  issues: Issue[];
  /** 将要执行的变更（netopt 计划里的 desc） */
  plans: string[];
  /** 有可执行的变更 */
  actionable: boolean;
}

export function summarize(v: NetoptView): OptimizeSummary {
  const issues: Issue[] = [];
  const plans: string[] = [];
  for (const sec of v.sections) {
    for (const f of sec.findings) {
      if (f.level === "warn" || f.level === "fail") {
        issues.push({ section: sec.title, level: f.level, message: f.message });
      }
    }
    for (const p of sec.plan) plans.push(p.desc);
  }
  const parts: string[] = [];
  if (v.summary.warn) parts.push(`${v.summary.warn} 项注意`);
  if (v.summary.fail) parts.push(`${v.summary.fail} 项故障`);
  if (v.summary.unsupported) parts.push(`${v.summary.unsupported} 项不支持`);
  return {
    headline: parts.length ? parts.join(" · ") : "全部正常",
    issues,
    plans,
    actionable: plans.length > 0,
  };
}

/** 一句话说清"这次会改什么"，给确认条 / 弹层当引导文案 */
export function planPreview(s: OptimizeSummary, limit = 3): string {
  if (!s.plans.length) return "没有需要改动的项";
  const head = s.plans.slice(0, limit).join("；");
  const more = s.plans.length > limit ? `等 ${s.plans.length} 项` : "";
  return `${head}${more}`;
}

/** 逐子系统的状态行（主窗口弹层用） */
function sectionRows(v: NetoptView): HTMLElement {
  const ul = document.createElement("ul");
  ul.className = "steps";
  for (const sec of v.sections) {
    ul.appendChild(sectionRow(sec));
  }
  return ul;
}

function sectionRow(sec: NetoptSection): HTMLLIElement {
  const li = document.createElement("li");
  const kind = document.createElement("span");
  kind.className = "kind";
  kind.textContent = sec.id;
  const text = document.createElement("span");
  const suffix = sec.supported
    ? sec.status === "ok"
      ? ""
      : `：${sec.findings
          .filter((f) => LEVEL_TEXT[f.level])
          .map((f) => f.message)
          .join("；")}`
    : `：${sec.unsupported_reason ?? "当前平台未实现"}`;
  text.textContent = `${sec.title} ${statusText(sec.status)}${suffix}`;
  li.append(kind, text);
  return li;
}

/**
 * 主窗口的「优化网络」弹层：先巡检，把结果与计划摆出来，再让用户点确认。
 *
 * @param refresh 优化结束后重拉全局状态（DNS 摘要等会变）
 */
export async function openOptimizeModal(refresh: () => Promise<void>): Promise<void> {
  const { box, close } = ui.openModal("网络优化", "zap");

  const lead = document.createElement("div");
  lead.className = "muted";
  lead.style.whiteSpace = "pre-wrap";
  lead.textContent = "正在巡检 DNS、IPv6、代理与防火墙…";
  box.appendChild(lead);

  const detail = document.createElement("div");
  detail.style.marginTop = "12px";
  box.appendChild(detail);

  const bar = document.createElement("div");
  bar.className = "modal-actions";
  const cancel = document.createElement("button");
  cancel.textContent = "关闭";
  cancel.addEventListener("click", close);
  const go = document.createElement("button");
  go.className = "primary";
  go.textContent = "一键优化";
  go.disabled = true;
  bar.append(cancel, go);
  box.appendChild(bar);

  let view: NetoptView;
  try {
    view = await api.netoptAudit();
  } catch (e) {
    lead.className = "error-text";
    lead.textContent = errText(e);
    return;
  }
  if (!view.available) {
    // 不可用的原因必须原样给出（多半是"没装 Python 3"），而不是让按钮静静地灰着
    lead.className = "error-text";
    lead.textContent = view.error ?? "网络优化组件不可用";
    return;
  }

  const s = summarize(view);
  lead.textContent = [
    `巡检结果：${s.headline}`,
    s.actionable
      ? `发现 ${s.plans.length} 项可优化，点「一键优化」后需要一次管理员授权。`
      : "当前配置已符合建议，无需改动。",
  ].join("\n");
  detail.replaceChildren(sectionRows(view));

  if (s.actionable) {
    const plan = document.createElement("div");
    plan.className = "muted";
    plan.style.marginTop = "12px";
    plan.style.whiteSpace = "pre-wrap";
    plan.textContent = `将执行：\n${s.plans.map((p) => `· ${p}`).join("\n")}`;
    detail.appendChild(plan);
  }

  go.disabled = !s.actionable;
  go.addEventListener("click", async () => {
    go.disabled = true;
    go.setAttribute("aria-busy", "true");
    try {
      const r = await api.netoptOptimize();
      close();
      ui.showReport(r, r.ok ? "网络优化完成" : "网络优化结果");
      await refresh();
    } catch (e) {
      ui.toast("error", "网络优化失败", errText(e));
    } finally {
      go.disabled = false;
      go.removeAttribute("aria-busy");
    }
  });
}
