// 深浅色三态（设计方案 §9.1）
// 主题三态：follow_system（默认）/ light / dark；选择写入 localStorage；
// 选 follow_system 时监听 matchMedia('(prefers-color-scheme: dark)') 的 change 事件

export type ThemeMode = "follow_system" | "light" | "dark";

const KEY = "sunet.theme";
const media = window.matchMedia("(prefers-color-scheme: dark)");
let current: ThemeMode = "follow_system";
let listener: ((mode: ThemeMode) => void) | null = null;

export function loadMode(): ThemeMode {
  const v = localStorage.getItem(KEY);
  if (v === "light" || v === "dark" || v === "follow_system") return v;
  return "follow_system";
}

function resolved(mode: ThemeMode): "light" | "dark" {
  if (mode === "follow_system") return media.matches ? "dark" : "light";
  return mode;
}

export function applyTheme(mode: ThemeMode): void {
  current = mode;
  localStorage.setItem(KEY, mode);
  document.documentElement.setAttribute("data-theme", resolved(mode));
  // 让滚动条、表单控件、日期选择器等原生元素一起切换
  document.documentElement.style.colorScheme = resolved(mode);
  updateButton();
}

export function cycleTheme(): ThemeMode {
  const order: ThemeMode[] = ["follow_system", "light", "dark"];
  const next = order[(order.indexOf(current) + 1) % order.length];
  applyTheme(next);
  return next;
}

export function currentMode(): ThemeMode {
  return current;
}

const ICON_SUN = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"/></svg>`;
const ICON_MOON = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>`;
const ICON_AUTO = `<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><path d="M12 3v18M3 12h18"/></svg>`;

function iconFor(mode: ThemeMode): string {
  if (mode === "light") return ICON_SUN;
  if (mode === "dark") return ICON_MOON;
  return ICON_AUTO;
}

function updateButton(): void {
  const btn = document.getElementById("btn-theme");
  if (!btn) return;
  btn.innerHTML = iconFor(current);
  btn.title =
    current === "follow_system"
      ? "主题：跟随系统（点击切换为浅色）"
      : current === "light"
        ? "主题：浅色（点击切换为深色）"
        : "主题：深色（点击切换为跟随系统）";
}

export function initTheme(): void {
  applyTheme(loadMode());
  media.addEventListener("change", () => {
    if (current === "follow_system") {
      applyTheme("follow_system");
      listener?.(current);
    }
  });
  document.getElementById("btn-theme")?.addEventListener("click", () => {
    const m = cycleTheme();
    listener?.(m);
  });
}

export function onThemeChange(fn: (mode: ThemeMode) => void): void {
  listener = fn;
}
