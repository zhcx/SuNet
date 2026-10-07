// 从 CHANGELOG.md 里取出某个版本的那一段，供 GitHub Release 说明使用。
//
// 用法：
//   node scripts/changelog-section.mjs v0.0.1 > release-notes.md
//   node scripts/changelog-section.mjs          # 不传参数 → 取最新的那一段
//
// 设计取舍：找不到对应版本时**不报错退出**，而是输出兜底文案。
// 理由：发布流程里"说明文字不全"远好于"release 步骤直接失败"。

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const changelog = readFileSync(join(root, 'CHANGELOG.md'), 'utf8');

// 版本号写法可能是 v0.0.1 / 0.0.1 / [0.0.1]，统一成 0.0.1 再比较
const wanted = (process.argv[2] ?? '').trim().replace(/^v/, '').replace(/^\[|\]$/g, '');

const lines = changelog.split(/\r?\n/);
/** @type {{version: string, body: string[]}[]} */
const sections = [];
let current = null;
for (const line of lines) {
  const m = /^##\s+\[?([^\]\s]+)\]?/.exec(line);
  if (m) {
    current = { version: m[1], body: [] };
    sections.push(current);
    continue;
  }
  if (current) current.body.push(line);
}

const clean = (s) => s.join('\n').trim();

const picked = wanted ? sections.find((s) => s.version === wanted) : sections[0];
if (picked) {
  process.stdout.write(`${clean(picked.body)}\n`);
} else {
  const versions = sections.map((s) => s.version).join('、') || '（CHANGELOG.md 里还没有版本段落）';
  process.stdout.write(
    `本版本未在 \`CHANGELOG.md\` 中找到对应的版本段落（现有：${versions}）。\n\n${clean(sections[0]?.body ?? [])}\n`,
  );
  process.stderr.write(`changelog-section: 未找到 ${wanted}，已回退到最新段落\n`);
}
