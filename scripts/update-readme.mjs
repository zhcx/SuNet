// 重新生成 README.md 里的两个「自动区」：当前版本行 与 平台对照表。
//
// 数据来自仓库里的真实文件（package.json、CHANGELOG.md、src-tauri/tauri.conf.json），
// 所以 README 里的版本号不会随发布而失效。本地可以直接跑：
//   node scripts/update-readme.mjs
//
// 由 .github/workflows/release.yml 的 readme job 在打 tag 之后调用；
// 无变化则不写文件，工作流据此跳过提交。

import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = process.env.GITHUB_REPOSITORY || 'zhcx/SuNet';
const REPO_URL = `https://github.com/${repo}`;
const README_PATH = join(root, 'README.md');

const readText = (rel) => readFileSync(join(root, rel), 'utf8');
const norm = (t) => t.replace(/\r\n/g, '\n');

// ---------- 1. 版本号：三个地方必须一致，不一致就报警（不静默挑一个） ----------

const version = JSON.parse(readText('package.json')).version;
const tauriConf = JSON.parse(readText('src-tauri/tauri.conf.json'));
const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(readText('src-tauri/Cargo.toml'))?.[1];
if (tauriConf.version !== version || cargoVersion !== version) {
  console.error(
    `[warn] 版本号不一致：package.json=${version} tauri.conf.json=${tauriConf.version} Cargo.toml=${cargoVersion}`,
  );
}

// ---------- 2. CHANGELOG：取最新版本的日期，用在版本行里 ----------

const sections = [];
let cur = null;
for (const line of norm(readText('CHANGELOG.md')).split('\n')) {
  const m = /^##\s+\[?([^\]\s]+)\]?(?:\s*-\s*(\S+))?/.exec(line);
  if (m) {
    cur = { version: m[1], date: m[2] ?? '' };
    sections.push(cur);
    continue;
  }
}
const latest = sections[0];
const date = latest?.date ? `（${latest.date}）` : '';

// ---------- 3. 两个自动区的正文 ----------

const bodies = {
  version: [
    `当前版本 **v${version}**${date} · [更新日志](CHANGELOG.md) · [下载安装包](${REPO_URL}/releases/latest)`,
  ].join('\n'),

  platforms: [
    '| 平台 | 安装包 | 状态 | 说明 |',
    '| --- | --- | --- | --- |',
    `| Windows 10 / 11 · x64 | \`SuNet_<版本>_x64-setup.exe\`（NSIS）、\`SuNet_<版本>_x64_en-US.msi\`（WiX），以及免安装的 \`SuNet_windows_x64.exe\` | ✅ 已支持 | 三件套（hosts / 系统代理 / DNS）全功能，托盘常驻与全局热键可用；安装包由 GitHub Actions 在打 tag 时自动构建，见 [Releases](${REPO_URL}/releases) |`,
    '| macOS | — | ⏳ 已立项，未适配 | 平台层仍是 Win32 / 注册表 / `netsh` / `hosts`，当前编译不过；移植映射表见 `HANDOFF.md` §6.4 |',
    '| Linux | — | ❌ 未支持 | 同上；托盘、全局热键与提权模型都需要重新设计 |',
  ].join('\n'),
};

// ---------- 4. 写回标记区（保留原有换行风格） ----------

const raw = readFileSync(README_PATH, 'utf8');
const eol = raw.includes('\r\n') ? '\r\n' : '\n';
let text = norm(raw);
const changed = [];

for (const [name, body] of Object.entries(bodies)) {
  const re = new RegExp(`(<!--\\s*BEGIN:${name}\\s*-->)[\\s\\S]*?(<!--\\s*END:${name}\\s*-->)`);
  if (!re.test(text)) {
    console.error(`[warn] README.md 里缺少标记区 BEGIN:${name} / END:${name}，已跳过`);
    continue;
  }
  const next = text.replace(re, `$1\n${body}\n$2`);
  if (next !== text) {
    changed.push(name);
    text = next;
  }
}

if (!changed.length) {
  console.log('README.md 无变化');
} else {
  writeFileSync(README_PATH, eol === '\r\n' ? text.replace(/\n/g, '\r\n') : text, 'utf8');
  console.log(`README.md 已更新：${changed.join('、')}`);
}
