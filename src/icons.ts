// 内联 SVG 图标集：统一 16×16 线框风格，stroke 用 currentColor
// 设计文档 §4.5.3 要求图标必须承载语义（三层各一个形状），这里保持一致

const wrap = (path: string, size = 16): string =>
  `<svg width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${path}</svg>`;

export const icon = {
  /** hosts：文档叠层 */
  hosts: (s?: number) =>
    wrap(
      '<path d="M5 4h11a2 2 0 0 1 2 2v13H7a2 2 0 0 1-2-2z"/><path d="M8.5 9.5h6M8.5 13h6"/>',
      s,
    ),
  /** 代理：双向箭头（流量改道） */
  proxy: (s?: number) =>
    wrap('<path d="M4 8h13l-3-3M20 16H7l3 3"/>', s),
  /** DNS：地球 */
  dns: (s?: number) =>
    wrap(
      '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.2 2.4 3.3 5.3 3.3 8.5S14.2 18.1 12 20.5c-2.2-2.4-3.3-5.3-3.3-8.5S9.8 5.9 12 3.5z"/>',
      s,
    ),
  /** 方案：书签 */
  profile: (s?: number) =>
    wrap('<path d="M7 4h10a1 1 0 0 1 1 1v15l-6-4-6 4V5a1 1 0 0 1 1-1z"/>', s),
  /** 日志：列表 */
  logs: (s?: number) =>
    wrap('<path d="M5 6.5h14M5 12h14M5 17.5h9"/>', s),
  shield: (s?: number) =>
    wrap('<path d="M12 3.5l7 3v6c0 4.2-2.9 7.2-7 8.5-4.1-1.3-7-4.3-7-8.5v-6z"/>', s),
  warn: (s?: number) =>
    wrap('<path d="M12 4.5l8.5 15h-17z"/><path d="M12 10v4M12 17h.01"/>', s),
  check: (s?: number) => wrap('<path d="M5 12.5l4.5 4.5L19 7.5"/>', s),
  close: (s?: number) => wrap('<path d="M6 6l12 12M18 6L6 18"/>', s),
  gear: (s?: number) =>
    wrap(
      '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1V21a2 2 0 1 1-4 0v-.1A1.6 1.6 0 0 0 7.9 19.4l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.6 1.6 0 0 0 3 13.9H3a2 2 0 1 1 0-4h.1A1.6 1.6 0 0 0 4.6 7.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.8.3H9a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.6 1.6 0 0 0 2.7 1.1l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7H21a2 2 0 1 1 0 4h-.1a1.6 1.6 0 0 0-1.5 1z"/>',
      s,
    ),
  refresh: (s?: number) =>
    wrap('<path d="M20 11a8 8 0 1 0-1.6 6.3"/><path d="M20 5v6h-6"/>', s),
  undo: (s?: number) =>
    wrap('<path d="M9 8H5V4"/><path d="M5 8a8 8 0 1 1 1.6 8.3"/>', s),
  power: (s?: number) =>
    wrap('<path d="M12 4v8"/><path d="M7.5 7A7 7 0 1 0 16.5 7"/>', s),
  sun: (s?: number) =>
    wrap(
      '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"/>',
      s,
    ),
  moon: (s?: number) =>
    wrap('<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>', s),
  auto: (s?: number) =>
    wrap('<circle cx="12" cy="12" r="9"/><path d="M12 3v18M3 12h18"/>', s),
  chevron: (s?: number) => wrap('<path d="M9 6l6 6-6 6"/>', s),
  external: (s?: number) =>
    wrap('<path d="M14 4h6v6"/><path d="M20 4l-8 8"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>', s),
  minus: (s?: number) => wrap('<path d="M6 12h12"/>', s),
  download: (s?: number) =>
    wrap('<path d="M12 4v11"/><path d="M7.5 11L12 15.5 16.5 11"/><path d="M5 19h14"/>', s),
  folder: (s?: number) =>
    wrap('<path d="M4 7a1 1 0 0 1 1-1h4l2 2h8a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1z"/>', s),
  zap: (s?: number) => wrap('<path d="M13 3L5 13h6l-1 8 8-10h-6z"/>', s),
} as const;

export type IconName = keyof typeof icon;

/** 取单个图标的 SVG 字符串 */
export function svg(name: IconName, size = 16): string {
  return icon[name](size);
}

/** 生成 <span class="i">图标</span>，用于安全插入 DOM（内容是常量，不含用户输入） */
export function iconHtml(name: IconName, size?: number): string {
  return `<span class="i">${svg(name, size)}</span>`;
}
