// 设置弹层：自启 / 全局热键 / 退出行为 / 日志 / 目录与诊断包

import { api, errText, type Settings, type ShortcutStatus } from "./api";
import * as ui from "./ui";

export async function openSettingsModal(): Promise<void> {
  const { box, close } = ui.openModal("设置");
  const loading = document.createElement("div");
  loading.className = "muted";
  loading.textContent = "加载中…";
  box.appendChild(loading);

  let st;
  try {
    st = await api.getAppState();
  } catch (e) {
    loading.textContent = errText(e);
    return;
  }
  loading.remove();
  const s: Settings = st.settings;

  const form = document.createElement("div");
  form.innerHTML = `
    <div class="section-title">行为</div>
    <div class="field-grid">
      <span>退出时</span>
      <select id="set-on-exit">
        <option value="keep">保持当前状态（默认：开了代理就是一直在用）</option>
        <option value="restore_if_changed">若本次运行改过，则还原</option>
        <option value="always_restore">总是还原为启动前状态</option>
      </select>
      <span>启动</span>
      <div class="actions" style="flex-direction:column;align-items:flex-start">
        <label class="row"><input type="checkbox" id="set-start-min" /> 启动后直接最小化到托盘（首次运行例外，会先显示主窗口）</label>
        <label class="row"><input type="checkbox" id="set-restore-launch" /> 启动后自动恢复上次方案（含 hosts/DNS 时会弹一次 UAC）</label>
      </div>
      <span>提权</span>
      <div class="actions">
        <label class="row"><input type="checkbox" id="set-elev-notice" /> 修改 hosts/DNS 前先显示说明对话框</label>
      </div>
      <span>开机自启</span>
      <div class="actions">
        <label class="row"><input type="checkbox" id="set-autostart" /> 通过计划任务在登录后静默启动（/RL LIMITED，不弹 UAC）</label>
      </div>
    </div>

    <div class="section-title">全局热键（默认直连 ↔ 上一个方案）</div>
    <div class="field-grid">
      <span>按键组合</span>
      <div class="actions">
        <div class="hotkey-capture" id="set-hotkey-box" tabindex="0" role="button" title="点击后直接按下组合键">
          <span class="hotkey-keys" id="set-hotkey-keys"></span>
        </div>
        <button id="set-hotkey-check">检测可用性</button>
        <button id="set-hotkey-apply">应用</button>
      </div>
      <span>状态</span>
      <span id="set-hotkey-status" class="muted"></span>
    </div>
    <p class="hint" id="set-hotkey-tip">点击左侧方框，然后直接按下组合键即可录制（至少包含一个 Ctrl / Alt / Shift / Win 修饰键，按 Esc 取消本次录制）。按一下切到「默认 · 直连」（关代理、清托管 hosts、DNS 还原自动），再按一下切回之前的方案；含 hosts / DNS 的切换会弹一次 UAC，结果以通知提示。托盘菜单与主界面始终可用。</p>

    <div class="section-title">代理</div>
    <label class="row"><input type="checkbox" id="set-probe" /> 开启代理前做连通性自检（强烈建议保持开启，否则代理软件没启动会造成全局断网）</label>

    <div class="section-title">日志与快照</div>
    <div class="field-grid">
      <span>日志级别</span>
      <select id="set-log-level">
        <option value="error">error</option>
        <option value="warn">warn</option>
        <option value="info">info（默认）</option>
        <option value="debug">debug（记录完整载荷，调试后建议切回）</option>
      </select>
      <span>脱敏</span>
      <label class="row"><input type="checkbox" id="set-log-redact" /> 日志中隐藏代理主机、内网地址与用户目录</label>
      <span>日志保留</span>
      <label class="row"><input type="number" id="set-log-days" min="1" max="90" style="width:80px" /> 天</label>
      <span>快照保留</span>
      <label class="row"><input type="number" id="set-snap-keep" min="3" max="100" style="width:80px" /> 份</label>
    </div>

    <div class="section-title">目录与诊断</div>
    <div class="actions">
      <button data-dir="config">配置目录</button>
      <button data-dir="logs">日志目录</button>
      <button data-dir="snapshots">快照目录</button>
      <button data-dir="hosts">hosts 所在目录</button>
      <button id="set-export">导出诊断包（默认脱敏）</button>
    </div>
    <p class="hint" id="set-version"></p>
  `;
  box.appendChild(form);

  const q = <T extends HTMLElement>(sel: string) => box.querySelector(sel) as T;
  q<HTMLSelectElement>("#set-on-exit").value = s.on_exit;
  q<HTMLInputElement>("#set-start-min").checked = s.start_minimized;
  q<HTMLInputElement>("#set-restore-launch").checked = s.restore_on_launch;
  q<HTMLInputElement>("#set-elev-notice").checked = s.confirm_elevation_notice;
  q<HTMLInputElement>("#set-autostart").checked = st.autostart_enabled;
  q<HTMLInputElement>("#set-probe").checked = s.proxy_probe_before_enable;
  q<HTMLSelectElement>("#set-log-level").value = s.log_level;
  q<HTMLInputElement>("#set-log-redact").checked = s.log_redact;
  q<HTMLInputElement>("#set-log-days").value = String(s.log_keep_days);
  q<HTMLInputElement>("#set-snap-keep").value = String(s.hosts_backup_keep);
  q("#set-version").textContent = `程序版本 ${st.version} · 配置目录 ${st.read_only ? "（只读模式）" : ""}`;

  // ------------------------------------------------------------------
  // 全局热键：按键捕获（不再要求用户手打 "Ctrl+Alt+S"）
  //
  // 后端是唯一校验方：这里只按 KeyboardEvent.code 拼串（修饰键名固定 Ctrl/Alt/Shift/Super），
  // 由 shortcut_check / shortcut_set 收敛成规范形态（Ctrl+Alt+KeyS → Ctrl+Alt+S）。
  // 以前是自由文本框 + 后端另一套键名表，同一个组合检测说不行、应用却成功，于是“不稳定”。
  // ------------------------------------------------------------------
  const MOD_KEYS: Record<string, string> = {
    ControlLeft: "Ctrl",
    ControlRight: "Ctrl",
    Control: "Ctrl",
    AltLeft: "Alt",
    AltRight: "Alt",
    Alt: "Alt",
    ShiftLeft: "Shift",
    ShiftRight: "Shift",
    Shift: "Shift",
    MetaLeft: "Super",
    MetaRight: "Super",
    Meta: "Super",
  };
  const MOD_ORDER = ["Ctrl", "Alt", "Shift", "Super"];
  // 展示用符号：配置里永远存规范名，只有界面显示成符号
  const GLYPH: Record<string, string> = {
    Minus: "-",
    Equal: "=",
    BracketLeft: "[",
    BracketRight: "]",
    Backslash: "\\",
    Semicolon: ";",
    Quote: "'",
    Comma: ",",
    Period: ".",
    Slash: "/",
    Backquote: "`",
    Up: "↑",
    Down: "↓",
    Left: "←",
    Right: "→",
  };

  const boxHotkey = q<HTMLDivElement>("#set-hotkey-box");
  const keysEl = q<HTMLSpanElement>("#set-hotkey-keys");
  const statusEl = q<HTMLSpanElement>("#set-hotkey-status");
  const tipEl = q<HTMLParagraphElement>("#set-hotkey-tip");
  const tipIdle = tipEl.textContent ?? "";

  // applied = 真正生效的那个（只有它会被写进「保存」的载荷）
  // pending = 刚录制/探测出的候选（只有它会被发给 shortcut_set）
  let applied = s.shortcuts.clear_proxy.binding;
  let pending = applied;
  let enabled = s.shortcuts.clear_proxy.enabled;
  let recording = false;
  let held: string[] = [];

  const chips = (parts: string[]): string =>
    parts
      .filter(Boolean)
      .map((p) => `<kbd class="kbd">${ui.esc(GLYPH[p] ?? p)}</kbd>`)
      .join('<span class="hotkey-plus">+</span>');

  const renderKeys = (binding: string): void => {
    const parts = binding.split("+").map((p) => p.trim()).filter(Boolean);
    keysEl.innerHTML = parts.length
      ? chips(parts)
      : '<span class="muted">未设置</span>';
  };

  const setHotkeyStatus = (cls: string, text: string): void => {
    statusEl.className = `muted ${cls}`.trim();
    statusEl.textContent = text;
  };

  // 录制期间要临时让出全局热键：全局注册是系统级的，按到当前生效的组合会真的切一次方案
  // （含 hosts / DNS 还会弹 UAC）。让出/收回必须串行 —— 并发 await 会让“收回”先落地、
  // “让出”后落地，热键就永久处于释放状态了。
  let captureOp: Promise<unknown> = Promise.resolve();
  const queueCapture = (on: boolean): Promise<unknown> => {
    const next = captureOp
      .catch(() => undefined)
      .then(() => api.shortcutCapture(on).catch(() => undefined));
    captureOp = next;
    return next;
  };

  const stopRecording = async (): Promise<void> => {
    if (!recording) return;
    detachCapture();
    // 等“收回热键”真正落地：后面的探测/应用才不会把自己占用的组合判成“被占用”
    await queueCapture(false);
    renderKeys(pending);
  };

  const detachCapture = (): void => {
    recording = false;
    held = [];
    boxHotkey.classList.remove("capturing");
    tipEl.textContent = tipIdle;
    document.removeEventListener("keydown", onCaptureKey, true);
    document.removeEventListener("keyup", onCaptureUp, true);
  };

  // 弹层被关掉时（点遮罩、Esc 等）盒子会脱离文档。监听挂在 document 上，
  // 如果此时不摘掉，之后每一次按键都会被 preventDefault 吃掉 —— 整个界面键盘全死。
  // 这里不依赖“关闭方是否记得调用”，而是在事件里自查：盒子没了就自己摘监听。
  const abandonIfClosed = (): boolean => {
    if (boxHotkey.isConnected) return false;
    if (recording) {
      detachCapture();
      void queueCapture(false);
    }
    return true;
  };

  const commit = async (candidate: string): Promise<void> => {
    await stopRecording();
    setHotkeyStatus("", "检测中…");
    try {
      const probe = await api.shortcutCheck(candidate);
      pending = probe.binding;
      renderKeys(pending);
      if (probe.is_current) {
        setHotkeyStatus("ok-text", "当前正在生效的组合，无需重复应用");
      } else if (probe.available) {
        setHotkeyStatus("ok-text", "可用，尚未应用 —— 点「应用」生效");
      } else {
        setHotkeyStatus("warn-text", "已被其他程序占用，请点方框重新录制");
      }
    } catch (e) {
      renderKeys(pending);
      setHotkeyStatus("warn-text", errText(e));
    }
  };

  const onCaptureKey = (e: KeyboardEvent): void => {
    // 盒子已经不在文档里：先摘监听，且不要 preventDefault（这一下按键还给界面）
    if (abandonIfClosed()) return;
    // 捕获阶段必须吃掉事件：否则 Esc 会被 ui.openModal 的关闭监听收走，弹层直接关掉
    e.preventDefault();
    e.stopImmediatePropagation();
    const mod = MOD_KEYS[e.code];
    if (mod) {
      if (!held.includes(mod)) held.push(mod);
      keysEl.innerHTML = chips([...MOD_ORDER.filter((m) => held.includes(m)), "…"]);
      tipEl.textContent = "继续按下一个主键（Esc 取消录制）";
      return;
    }
    if (e.code === "Escape" && held.length === 0) {
      void stopRecording();
      return;
    }
    const mods = MOD_ORDER.filter((m) => held.includes(m));
    if (mods.length === 0) {
      // 没有修饰键的全局热键会把正常打字全部抢走，后端也会拒绝，这里先讲清楚
      tipEl.textContent =
        "必须包含至少一个修饰键（Ctrl / Alt / Shift / Win），请重新按";
      held = [];
      renderKeys(pending);
      return;
    }
    void commit([...mods, e.code].join("+"));
  };

  const onCaptureUp = (e: KeyboardEvent): void => {
    if (abandonIfClosed()) return;
    const mod = MOD_KEYS[e.code];
    if (!mod) return;
    held = held.filter((m) => m !== mod);
    keysEl.innerHTML = held.length
      ? chips([...MOD_ORDER.filter((m) => held.includes(m)), "…"])
      : '<span class="muted">按下组合键…</span>';
  };

  const startRecording = (): void => {
    if (recording) return;
    recording = true;
    held = [];
    boxHotkey.classList.add("capturing");
    keysEl.innerHTML = '<span class="muted">按下组合键…</span>';
    tipEl.textContent =
      "请按下组合键（至少含一个 Ctrl / Alt / Shift / Win；Esc 取消录制）";
    // 立即挂监听，避免漏掉紧接着的第一个按键
    document.addEventListener("keydown", onCaptureKey, true);
    document.addEventListener("keyup", onCaptureUp, true);
    void queueCapture(true);
  };

  boxHotkey.addEventListener("click", () => {
    if (!recording) startRecording();
  });
  boxHotkey.addEventListener("keydown", (e) => {
    // 键盘用户：聚焦方框后按 Enter / 空格即可开始录制（录制中这些键已被捕获阶段拦下）
    if (!recording && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      startRecording();
    }
  });
  // 点弹层里别处也算结束录制。挂在遮罩上：遮罩包含整个弹层，其任何子元素的
  // pointerdown 都会冒泡到这里，不用担心只盖住“按键组合”那一行。
  const backdrop = boxHotkey.closest(".backdrop");
  backdrop?.addEventListener("pointerdown", (e) => {
    if (recording && !boxHotkey.contains(e.target as Node)) void stopRecording();
  });

  // 热键真实状态：只允许在注册成功后显示“已启用”
  const showShortcut = (sc: ShortcutStatus): void => {
    applied = sc.binding;
    pending = sc.binding;
    enabled = sc.enabled;
    renderKeys(applied);
    if (sc.registered) {
      setHotkeyStatus("ok-text", `已启用（${sc.binding}，注册成功）`);
    } else if (!sc.binding) {
      setHotkeyStatus("warn-text", "未设置热键 —— 点方框录制一个组合键");
    } else {
      setHotkeyStatus(
        "warn-text",
        `未生效：${sc.error ? `${sc.error.code} ${sc.error.message}` : "未注册"}`,
      );
    }
  };
  try {
    showShortcut(await api.shortcutStatus());
  } catch (e) {
    renderKeys(applied);
    setHotkeyStatus("warn-text", errText(e));
  }

  q<HTMLButtonElement>("#set-hotkey-check").addEventListener("click", async () => {
    await stopRecording();
    const target = pending || applied;
    if (!target) {
      ui.toast("error", "还没有按键组合", "请先点方框录制一个组合键");
      return;
    }
    try {
      const probe = await api.shortcutCheck(target);
      if (probe.is_current) {
        ui.toast("info", "这就是当前生效的组合", `${probe.binding} 已注册并正在工作`);
      } else if (probe.available) {
        ui.toast(
          "info",
          "组合可用",
          `${probe.binding} 当前未被占用（仍可能在正式注册瞬间被抢占）`,
        );
      } else {
        ui.toast(
          "error",
          "组合已被占用",
          `[E5001] ${probe.binding} 已被其他程序占用，请更换`,
        );
      }
    } catch (e) {
      ui.toast("error", "检测失败", errText(e));
    }
  });

  q<HTMLButtonElement>("#set-hotkey-apply").addEventListener("click", async () => {
    await stopRecording();
    const target = pending || applied;
    if (!target) {
      ui.toast("error", "还没有按键组合", "请先点方框录制一个组合键");
      return;
    }
    try {
      const sc = await api.shortcutSet(target);
      showShortcut(sc);
      ui.toast("info", "热键已更新", `当前监听：${sc.binding}`);
    } catch (e) {
      ui.toast("error", "热键注册失败", errText(e));
      try {
        showShortcut(await api.shortcutStatus());
      } catch {
        /* 忽略 */
      }
    }
  });

  q<HTMLInputElement>("#set-autostart").addEventListener("change", async (ev) => {
    const enable = (ev.target as HTMLInputElement).checked;
    try {
      await api.autostartSet(enable);
      ui.toast("info", enable ? "已启用开机自启" : "已取消开机自启");
    } catch (e) {
      ui.toast("error", "自启设置失败", errText(e));
      (ev.target as HTMLInputElement).checked = !enable;
    }
  });

  box.querySelectorAll<HTMLButtonElement>("[data-dir]").forEach((btn) => {
    btn.addEventListener("click", () => void api.systemOpenDir(btn.dataset.dir ?? "config"));
  });

  q<HTMLButtonElement>("#set-export").addEventListener("click", async () => {
    const yes = await ui.confirmModal({
      title: "导出诊断包",
      body: "将打包最近 3 天日志、当前三层状态、环境信息与配置 schema 版本。\n代理主机名、内网地址、用户目录会在导出时脱敏。",
      confirmText: "导出",
    });
    if (!yes) return;
    try {
      const path = await api.logsExport();
      ui.toast("info", "诊断包已生成", path);
    } catch (e) {
      ui.toast("error", "导出失败", errText(e));
    }
  });

  const bar = document.createElement("div");
  bar.className = "modal-actions";
  const cancel = document.createElement("button");
  cancel.textContent = "取消";
  cancel.addEventListener("click", () => {
    void stopRecording();
    close();
  });
  const save = document.createElement("button");
  save.className = "primary";
  save.textContent = "保存";
  save.addEventListener("click", async () => {
    void stopRecording();
    const next: Settings = {
      ...s,
      on_exit: q<HTMLSelectElement>("#set-on-exit").value,
      start_minimized: q<HTMLInputElement>("#set-start-min").checked,
      restore_on_launch: q<HTMLInputElement>("#set-restore-launch").checked,
      confirm_elevation_notice: q<HTMLInputElement>("#set-elev-notice").checked,
      proxy_probe_before_enable: q<HTMLInputElement>("#set-probe").checked,
      log_level: q<HTMLSelectElement>("#set-log-level").value,
      log_redact: q<HTMLInputElement>("#set-log-redact").checked,
      log_keep_days: Number(q<HTMLInputElement>("#set-log-days").value) || 7,
      hosts_backup_keep: Number(q<HTMLInputElement>("#set-snap-keep").value) || 20,
      autostart: { ...s.autostart, enabled: q<HTMLInputElement>("#set-autostart").checked },
      // 热键只能用“真实生效”的 applied。settings_save 是整体替换 settings，
      // 展开弹层打开时的旧快照会把刚用「应用」改好的组合写回旧值（改绑被静默回滚）。
      shortcuts: {
        ...s.shortcuts,
        clear_proxy: { ...s.shortcuts.clear_proxy, enabled, binding: applied },
      },
    };
    try {
      await api.settingsSave(next);
      ui.toast("info", "设置已保存");
      close();
    } catch (e) {
      ui.toast("error", "保存失败", errText(e));
    }
  });
  bar.appendChild(cancel);
  bar.appendChild(save);
  box.appendChild(bar);
}
