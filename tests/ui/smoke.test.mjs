import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";
import { chromium } from "playwright-core";

let server;
let browser;
before(async () => {
  server = await createServer({ server: { host: "127.0.0.1", port: 5181, strictPort: true } });
  await server.listen();
  browser = await chromium.launch({
    headless: true,
    channel: process.env.SUNET_TEST_BROWSER || (process.platform === "win32" ? "msedge" : "chrome"),
  });
});
after(async () => {
  await browser?.close();
  await server?.close();
});

// 只模拟应用命令的边界；渲染、事件绑定、标签切换和表单提交均运行真实前端。
// 不接触用户配置、系统代理、hosts 或 DNS。
async function openApp(t) {
  const page = await browser.newPage({ viewport: { width: 900, height: 640 } });
  t.after(() => page.close());
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  t.after(() => assert.deepEqual(errors, [], "no unhandled page errors"));
  await page.addInitScript(() => {
    const proxy = { enable: true, server: "socks=127.0.0.1:1080", bypass: "localhost;127.*;<local>", pac_present: false };
    const profile = {
      id: "builtin-direct", name: "默认 · 直连", is_builtin: true, note: "默认直连",
      hosts: { enabled: true, entry_ids: [], source_ids: [] },
      proxy: { enabled: false, host: "", port: 0, bypass: "localhost", https_host: "", https_port: 0, socks_host: "", socks_port: 0 },
      dns: { enabled: true, mode: "dhcp", interface_alias: "", v4: [], v6: [], enable_v4: true, enable_v6: false },
    };
    const stats = { path: "C:\\Windows\\System32\\drivers\\etc\\hosts", exists: true, block_present: false, total_entries: 0, enabled_entries: 0 };
    const settings = { hosts_backup_keep: 20, log_keep_days: 7, shortcuts: { clear_proxy: { binding: "Ctrl+Alt+S" } }, proxy_probe_before_enable: true };
    const state = { initialized: true, version: "test", proxy, hosts: stats, settings, profiles: [profile], prand: { is_elevated: false }, runtime: { lock_contended: false }, tray: { dns_v4: "IPv4 自动获取", dns_v6: "IPv6 自动获取" } };
    window.__test = { proxy, profile, state, calls: [], delayHosts: false, proxyResult: null };
    let callback = 0;
    window.__TAURI_INTERNALS__ = {
      transformCallback() { return ++callback; }, unregisterCallback() {},
      async invoke(cmd, args) {
        window.__test.calls.push({ cmd, args });
        if (cmd === "get_app_state") return state;
        if (cmd === "profile_list") return [profile];
        if (cmd === "proxy_get") return { state: proxy, can_undo: false, probe_before_enable: true, scope: [] };
        if (cmd === "hosts_list") {
          if (window.__test.delayHosts) await new Promise((resolve) => setTimeout(resolve, 300));
          return { entries: [], stats, duplicates: [], pending: false, preview: "" };
        }
        if (cmd === "sources_list") return [{ id: "test-sub", name: "测试订阅", url: "https://example.com/hosts", interval_hours: 24 }];
        if (cmd === "shortcut_status") return {};
        if (cmd === "profile_save") { Object.assign(profile, args.profile); return [profile]; }
        if (cmd === "proxy_set") return window.__test.proxyResult || { ok: true, steps: [], message: "切换完成" };
        if (cmd === "logs_recent") return Array.from({ length: 100 }, (_, i) => ({ ts: `12:00:${i}`, level: "info", target: "test", message: `日志 ${i}` }));
        return [];
      },
    };
  });
  await page.goto("http://127.0.0.1:5181/");
  await page.waitForSelector("#pf-list button");
  return page;
}

async function proxyTab(page) {
  await page.getByRole("tab", { name: "代理", exact: true }).click();
  await page.waitForSelector("#px-host");
}

test("window stays within viewport with long proxy status and minimum size", async (t) => {
  const page = await openApp(t);
  await page.evaluate(() => {
    window.__test.proxy.server = `http=${"very-long-hostname.".repeat(12)}:7890;https=localhost:7891;socks=localhost:1080`;
  });
  await page.click("#pf-refresh");
  for (const [width, height] of [[900, 640], [760, 540]]) {
    await page.setViewportSize({ width, height });
    const size = await page.evaluate(() => {
      const root = document.documentElement;
      const view = document.querySelector("#view");
      const footer = document.querySelector(".statusbar").getBoundingClientRect();
      return { sw: root.scrollWidth, cw: root.clientWidth, sh: root.scrollHeight, ch: root.clientHeight, viewSw: view.scrollWidth, viewCw: view.clientWidth, footerBottom: footer.bottom };
    });
    assert.equal(size.sw, size.cw);
    assert.equal(size.sh, size.ch);
    assert.equal(size.viewSw, size.viewCw);
    assert.ok(size.footerBottom <= height);
  }
});

test("legacy SOCKS switches to HTTP without scheme text or stale SOCKS payload", async (t) => {
  const page = await openApp(t);
  await proxyTab(page);
  assert.equal(await page.locator("#px-host").inputValue(), "");
  assert.equal(await page.locator("#px-socks-host").inputValue(), "127.0.0.1");
  await page.locator("#px-socks-host").fill("");
  await page.locator("#px-socks-port").fill("");
  await page.locator("#px-host").fill("127.0.0.1");
  await page.locator("#px-port").fill("7890");
  await page.click("#px-enable");
  await page.waitForFunction(() => window.__test.calls.some((c) => c.cmd === "proxy_set"));
  const { cfg } = await page.evaluate(() => window.__test.calls.find((c) => c.cmd === "proxy_set").args);
  assert.equal(cfg.host, "127.0.0.1");
  assert.equal(cfg.port, 7890);
  assert.equal(cfg.socks, undefined);
  await page.getByRole("tab", { name: "方案", exact: true }).click();
  await proxyTab(page);
  assert.equal(await page.locator("#px-port").inputValue(), "7890", "draft survives navigation");
});

test("default profile can select subscription while network controls stay protected", async (t) => {
  const page = await openApp(t);
  await page.locator("[data-act=edit]").click();
  await page.waitForSelector("#pe-save");
  assert.equal(await page.locator("#pe-name").isDisabled(), true);
  assert.equal(await page.locator("#pe-proxy-host").isVisible(), false);
  assert.equal(await page.locator("#pe-dns-mode").isVisible(), false);
  await page.locator("#pe-sources input").check();
  await page.click("#pe-save");
  await page.waitForFunction(() => window.__test.calls.some((c) => c.cmd === "profile_save"));
  assert.deepEqual(await page.evaluate(() => window.__test.profile.hosts.source_ids), ["test-sub"]);
});

test("slow previous tab cannot overwrite current selection", async (t) => {
  const page = await openApp(t);
  await page.evaluate(() => { window.__test.delayHosts = true; });
  await page.getByRole("tab", { name: "Hosts", exact: true }).click();
  await proxyTab(page);
  await page.waitForTimeout(400);
  assert.equal(await page.locator("#px-host").count(), 1);
  assert.equal(await page.getByRole("tab", { name: "代理", exact: true }).getAttribute("aria-selected"), "true");
});

test("validation failure remains visible after refresh without force-enable prompt", async (t) => {
  const page = await openApp(t);
  await proxyTab(page);
  await page.evaluate(() => {
    window.__test.proxyResult = { ok: false, steps: [{ kind: "proxy", applied: false, verified: false, message: "[E4002] 地址格式非法" }], message: "失败已回滚" };
  });
  await page.click("#px-enable");
  await page.waitForFunction(() => window.__test.calls.filter((c) => c.cmd === "proxy_get").length >= 3);
  assert.equal(await page.locator("#px-report").isVisible(), true);
  assert.match(await page.locator("#px-report").innerText(), /E4002/);
  assert.equal(await page.getByRole("button", { name: "仍然开启", exact: true }).count(), 0);
});

test("logs scroll to latest rows after tab mounts", async (t) => {
  const page = await openApp(t);
  await page.getByRole("tab", { name: "日志", exact: true }).click();
  await page.waitForFunction(() => {
    const logs = document.querySelector("#lg-view");
    return logs && logs.scrollHeight > logs.clientHeight && logs.scrollTop > 0;
  });
});
