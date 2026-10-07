// 重新生成 README.md 里的「自动区」：当前版本 / 平台对照表 / 项目规模 / 最新发布日志。
//
// 数据全部来自仓库里的真实文件（package.json、CHANGELOG.md、源码扫描），
// 所以 README 里的数字不会随代码演进而过期；本地可以直接跑：
//   node scripts/update-readme.mjs
//
// 由 .github/workflows/release.yml 的 readme job 在打 tag 之后调用，
// 无变化则不写文件（工作流据此跳过提交）。

import { readFileSync, writeFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = process.env.GITHUB_REPOSITORY || 'zhcx/SuNet';
const REPO_URL = `https://github.com/${repo}`;
const README_PATH = join(root, 'README.md');

const readText = (rel) => readFileSync(join(root, rel), 'utf8');
const norm = (t) => t.replace(/\r\n/g, '\n');

// ---------- 1. 项目规模：真实扫描，不手写 ----------

function walk(absDir, exts, out = []) {
  for (const name of readdirSync(absDir)) {
    if (name === 'node_modules' || name === 'target' || name === '.git') continue;
    const full = join(absDir, name);
    if (statSync(full).isDirectory()) walk(full, exts, out);
    else if (exts.some((e) => name.endsWith(e))) out.push(full);
  }
  return out;
}

const lineCount = (file) => {
  const ls = norm(readFileSync(file, 'utf8')).split('\n');
  if (ls.length && ls[ls.length - 1] === '') ls.pop();
  return ls.length;
};

const sumLines = (files) => files.reduce((n, f) => n + lineCount(f), 0);
const countMatches = (files, re) =>
  files.reduce((n, f) => n + (norm(readFileSync(f, 'utf8')).match(re) ?? []).length, 0);

const rustFiles = walk(join(root, 'src-tauri/src'), ['.rs']);
const feFiles = walk(join(root, 'src'), ['.ts', '.css']);

const metrics = {
  rustFiles: rustFiles.length,
  rustLines: sumLines(rustFiles),
  feFiles: feFiles.length,
  feLines: sumLines(feFiles),
  commands: countMatches(rustFiles, /#\[tauri::command\]/g),
  tests: countMatches(rustFiles, /#\[(?:tokio::)?test\]/g),
  errorCodes: countMatches([join(root, 'src-tauri/src/error.rs')], /\bpub const E[A-Z0-9_]+\b/g),
};

// ---------- 2. 版本号：三处必须一致，不一致就报警（不静默挑一个） ----------

const pkg = JSON.parse(readText('package.json'));
const version = pkg.version;
const tauriConf = JSON.parse(readText('src-tauri/tauri.conf.json'));
const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(readText('src-tauri/Cargo.toml'))?.[1];

if (tauriConf.version !== version || cargoVersion !== version) {
  console.error(
    `[warn] 版本号不一致：package.json=${version} tauri.conf.json=${tauriConf.version} Cargo.toml=${cargoVersion}`,
  );
}

// ---------- 3. CHANGELOG：拿最新段落当「发布日志」 ----------

const changelogRaw = readText('CHANGELOG.md');
const changelogLines = norm(changelogRaw).split('\n');
const sections = [];
let cur = null;
for (const line of changelogLines) {
  const m = /^##\s+\[?([^\]\s]+)\]?(?:\s*-\s*(\S+))?/.exec(line);
  if (m) {
    cur = { version: m[1], date: m[2] ?? '', body: [] };
    sections.push(cur);
    continue;
  }
  if (cur) cur.body.push(line);
}
const latest = sections[0];
const trim = (s) => s.join('\n').replace(/^\n+|\n+$/g, '');

// README 只放「亮点」小节，避免把整段日志抄一遍；完整内容看 CHANGELOG.md。
// 找不到指定小节时宁可退回完整段落，也不要让 README 里出现空块。
const README_SECTIONS = ['新增'];
function pickSubsections(bodyLines, wanted) {
  const blocks = [];
  let now = null;
  for (const line of bodyLines) {
    const m = /^###\s+(.+?)\s*$/.exec(line);
    if (m) {
      now = { name: m[1].trim(), lines: [] };
      blocks.push(now);
      continue;
    }
    if (now) now.lines.push(line);
  }
  const picked = blocks.filter((b) => wanted.includes(b.name));
  if (!picked.length) return trim(bodyLines);
  return picked.map((b) => `### ${b.name}\n\n${trim(b.lines)}`).join('\n\n');
}

// ---------- 4. 各自动区的正文 ----------

const bodies = {
  version: [
    `当前版本 **v${version}**${latest?.date ? `（${latest.date}）` : ''} · ` +
      `[更新日志](CHANGELOG.md) · [下载安装包](${REPO_URL}/releases/latest)`,
  ].join('\n'),

  platforms: [
    '| 平台 | 安装包 | 状态 | 说明 |',
    '| --- | --- | --- | --- |',
    `| Windows 10 / 11 · x64 | \`SuNet_<版本>_x64-setup.exe\`（NSIS）、\`SuNet_<版本>_x64_<语言>.msi\`（WiX）、免安装的 \`SuNet.exe\` | ✅ 已支持 | 三件套（hosts / 系统代理 / DNS）全功能，托盘常驻与全局热键可用；安装包由 GitHub Actions 在打 tag 时自动构建，见 [Releases](${REPO_URL}/releases) |`,
    '| macOS | — | ⏳ 已立项，未适配 | 平台层仍是 Win32 / 注册表 / `netsh` / `hosts`，当前编译不过；移植映射表见 `HANDOFF.md` §6.4 |',
    '| Linux | — | ❌ 未支持 | 同上；托盘、全局热键与提权模型都需要重新设计 |',
  ].join('\n'),

  stats: [
    '| 项 | 数量 |',
    '| --- | --- |',
    `| 后端（\`src-tauri/src\`） | ${metrics.rustFiles} 个文件 / ${metrics.rustLines.toLocaleString('en-US')} 行 Rust |`,
    `| 前端（\`src\`） | ${metrics.feFiles} 个文件 / ${metrics.feLines.toLocaleString('en-US')} 行 TS + CSS |`,
    `| Tauri 命令 | ${metrics.commands} 个 |`,
    `| Rust 单元测试 | ${metrics.tests} 个（\`cd src-tauri && cargo test\`） |`,
    `| 错误码 | ${metrics.errorCodes} 个（集中定义在 \`src-tauri/src/error.rs\`） |`,
    `| 当前版本 | v${version} |`,
  ].join('\n'),

  latest: [
    `**v${latest?.version ?? version}**${latest?.date ? `（${latest.date}）` : ''}的变更（摘要）：`,
    '',
    pickSubsections(latest?.body ?? [], README_SECTIONS),
    '',
    `完整历史见 [CHANGELOG.md](CHANGELOG.md)，全部版本与安装包见 [Releases](${REPO_URL}/releases)。`,
  ].join('\n'),
};

// ---------- 5. 写回标记区（保留原有换行风格） ----------

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
