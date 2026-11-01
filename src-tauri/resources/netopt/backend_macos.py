"""macOS 后端：networksetup / scutil / ifconfig / socketfilterfw / pmset。

只负责"读状态、出计划、给回退命令"，不直接执行——执行统一在 core。
"""

from __future__ import annotations

import re

from netopt_core import dns_probe, finding, op, run, section, status_from


class Backend:
    id = "macos"

    def __init__(self, config=None):
        self.config = config or {}
        self.device = self._primary_device()
        self.service = self._service_for(self.device) if self.device else None
        self.gateway = self.config.get("gateway") or self._gateway()

    # ------------------------------------------------------------ 探测辅助
    @staticmethod
    def _primary_device():
        rc, out, _ = run(["route", "-n", "get", "default"])
        for line in out.splitlines():
            if line.strip().startswith("interface:"):
                return line.split(":", 1)[1].strip()
        return None

    @staticmethod
    def _gateway():
        rc, out, _ = run(["route", "-n", "get", "default"])
        for line in out.splitlines():
            if line.strip().startswith("gateway:"):
                return line.split(":", 1)[1].strip()
        return None

    @staticmethod
    def _service_for(device):
        rc, out, _ = run(["networksetup", "-listnetworkserviceorder"])
        current = None
        for line in out.splitlines():
            line = line.strip()
            m = re.match(r"^\(\d+\)\s*(.+)$", line)
            if m:
                current = m.group(1).strip()
            elif "Device:" in line and device and line.rstrip(")").endswith(device):
                return current
            elif current and line.startswith("(Hardware Port"):
                pass
        return current

    def _sh(self, argv, timeout=15):
        return run(argv, timeout=timeout)

    # ------------------------------------------------------------ DNS
    def _dns_servers(self):
        rc, out, _ = self._sh(["networksetup", "-getdnsservers", self.service])
        if "aren't any" in out:
            return [], "dhcp"
        return [l.strip() for l in out.splitlines() if l.strip()], "manual"

    def audit_dns(self):
        if not self.service:
            return section("dns", supported=False, reason="未找到活动网络服务")
        servers, source = self._dns_servers()
        findings, measured = [], []

        for srv in servers[:4]:
            alive, rtt, detail = dns_probe(srv)
            measured.append({"server": srv, "alive": alive, "rtt_ms": rtt, "detail": detail})
            if not alive:
                findings.append(finding(
                    "dns.unreachable", "fail",
                    "%s 无响应（%s），解析会在这里空等超时" % (srv, detail), server=srv))
            elif rtt and rtt > 150:
                findings.append(finding(
                    "dns.slow", "warn",
                    "%s 响应 %.0fms，偏慢" % (srv, rtt), server=srv))

        reachable = [m["server"] for m in measured if m["alive"]]
        dead = [m["server"] for m in measured if not m["alive"]]

        if not servers:
            findings.append(finding("dns.none", "warn", "由 DHCP/路由通告下发解析器，本机未显式配置"))
        elif len(servers) == 1:
            findings.append(finding("dns.single", "warn", "只有一台解析器，故障时无备份"))
        if dead and not reachable:
            findings.append(finding("dns.all_down", "fail", "所有解析器均无响应"))

        target = self.config.get("dns")
        if target is None:
            target = reachable if (dead and reachable) else None

        plan = []
        if target and target != servers:
            plan.append(op(
                "设置 DNS 为 %s" % ", ".join(target),
                ["networksetup", "-setdnsservers", self.service] + list(target),
                undo=["networksetup", "-setdnsservers", self.service] + (servers or ["Empty"]),
            ))
        elif not reachable and not dead:
            pass
        elif dead and not reachable and not target:
            findings.append(finding(
                "dns.no_fallback", "warn",
                "全部解析器失效且未提供替代，可用 --dns 指定"))

        status = status_from(findings)
        return section("dns", status=status, current={
            "servers": servers, "source": source, "measured": measured,
        }, recommended={"servers": target or reachable}, findings=findings, plan=plan)

    def restore_dns(self, data):
        servers = data.get("servers") or []
        if not servers:
            return [op("清空 DNS（回到 DHCP）",
                       ["networksetup", "-setdnsservers", self.service, "Empty"])]
        return [op("恢复 DNS 为 %s" % ", ".join(servers),
                   ["networksetup", "-setdnsservers", self.service] + list(servers))]

    # ------------------------------------------------------------ IPv6
    def _global_v6(self):
        rc, out, _ = self._sh(["ifconfig", self.device])
        addrs = re.findall(r"inet6\s+([0-9a-fA-F:]+)\s+prefixlen\s+(\d+)", out)
        return [a for a, plen in addrs if not a.lower().startswith("fe80") and a != "::1"]

    def audit_ipv6(self):
        if not self.device:
            return section("ipv6", supported=False, reason="未找到活动网卡")
        addrs = self._global_v6()
        if not addrs:
            return section("ipv6", status="warn", current={"addresses": []},
                           findings=[finding("ipv6.none", "warn", "未获取到全局 IPv6 地址")])

        prefixes = {}
        for a in addrs:
            prefixes.setdefault(":".join(a.split(":")[:4]), []).append(a)

        target = self.config.get("ipv6_probe", "2606:4700:4700::1111")
        state, findings, plan = {}, [], []
        for pfx, group in prefixes.items():
            alive = False
            for a in group[:2]:
                rc, out, _ = self._sh(
                    ["ping6", "-c", "1", "-S", a, target], timeout=4)
                if rc == 0 and "1 packets received" in out:
                    alive = True
                    break
            state[pfx] = {"addresses": group, "reachable": alive}

        dead = [p for p, v in state.items() if not v["reachable"]]
        alive_cnt = len(state) - len(dead)

        if alive_cnt == 0:
            findings.append(finding(
                "ipv6.all_unreachable", "fail",
                "所有 IPv6 前缀均不可达，判定为出口故障，不建议删地址"))
        elif dead:
            for p in dead:
                findings.append(finding(
                    "ipv6.stale_prefix", "warn",
                    "前缀 %s::/64 已失效但仍挂着地址，应用选中它必然失败" % p, prefix=p))
                for a in state[p]["addresses"]:
                    plan.append(op("删除失效地址 %s" % a,
                                   ["ifconfig", self.device, "inet6", a, "delete"]))

        status = "fail" if alive_cnt == 0 else ("warn" if dead else "ok")
        return section("ipv6", status=status, current={
            "addresses": addrs, "prefixes": state, "probe_target": target,
        }, findings=findings, plan=plan)

    # ------------------------------------------------------------ 代理
    def _proxy_state(self):
        rc, out, _ = self._sh(["scutil", "--proxy"])
        def grab(key):
            m = re.search(r"%s\s*:\s*(\S+)" % key, out)
            return m.group(1) if m else None
        return {
            "https_enabled": grab("HTTPSEnable") == "1",
            "https_host": grab("HTTPSProxy"),
            "https_port": int(grab("HTTPSPort") or 0) or None,
            "http_enabled": grab("HTTPEnable") == "1",
        }

    def _bypass(self):
        rc, out, _ = self._sh(["networksetup", "-getproxybypassdomains", self.service])
        if "aren't any" in out:
            return []
        return [l.strip() for l in out.splitlines() if l.strip()]

    def audit_proxy(self):
        if not self.service:
            return section("proxy", supported=False, reason="未找到活动网络服务")
        state, bypass = self._proxy_state(), self._bypass()
        findings, plan = [], []

        gw = self.gateway
        want = ["localhost", "127.0.0.1", "*.local", "169.254/16"]
        if gw:
            want.append(gw)
        if state.get("https_host"):
            want.append(state["https_host"])

        missing = [w for w in want if w not in bypass]
        if state["https_enabled"] and missing:
            findings.append(finding(
                "proxy.bypass_incomplete", "warn",
                "绕过列表缺少 %s，代理故障时连路由器都进不去" % ", ".join(missing),
                missing=missing))
            merged = bypass + [m for m in missing if m not in bypass]
            plan.append(op("补全代理绕过列表",
                           ["networksetup", "-setproxybypassdomains", self.service] + merged,
                           undo=["networksetup", "-setproxybypassdomains", self.service] + (bypass or ["Empty"])))

        if not state["https_enabled"]:
            findings.append(finding("proxy.disabled", "info", "系统 HTTPS 代理未启用，全部直连"))

        status = status_from(findings)
        return section("proxy", status=status, current={
            "bypass": bypass, **state,
        }, recommended={"bypass": want}, findings=findings, plan=plan)

    def restore_proxy(self, data):
        ops = []
        bypass = data.get("bypass") or []
        ops.append(op("恢复代理绕过列表",
                      ["networksetup", "-setproxybypassdomains", self.service] + (bypass or ["Empty"])))
        if data.get("https_enabled") and data.get("https_host"):
            ops.append(op("恢复 HTTPS 代理",
                          ["networksetup", "-setsecurewebproxy", self.service,
                           data["https_host"], str(data.get("https_port") or 443)]))
            ops.append(op("启用 HTTPS 代理",
                          ["networksetup", "-setsecurewebproxystate", self.service, "on"]))
        return ops

    # ------------------------------------------------------------ 防火墙
    def audit_firewall(self):
        rc, out, _ = self._sh(["/usr/libexec/ApplicationFirewall/socketfilterfw", "--getglobalstate"])
        enabled = "enabled" in out
        rc2, out2, _ = self._sh(["/usr/libexec/ApplicationFirewall/socketfilterfw", "--getstealthmode"])
        stealth = "on" in out2.lower()
        findings, plan = [], []
        if not enabled:
            findings.append(finding("firewall.disabled", "warn", "应用层防火墙未启用"))
            plan.append(op("启用防火墙",
                           ["/usr/libexec/ApplicationFirewall/socketfilterfw", "--setglobalstate", "on"]))
        if not stealth:
            findings.append(finding("firewall.stealth_off", "info", "隐身模式未开启"))
            plan.append(op("开启隐身模式",
                           ["/usr/libexec/ApplicationFirewall/socketfilterfw", "--setstealthmode", "on"]))
        status = status_from(findings)
        return section("firewall", status=status, current={"enabled": enabled, "stealth": stealth},
                       findings=findings, plan=plan)

    def restore_firewall(self, data):
        state = "on" if data.get("enabled") else "off"
        return [op("恢复防火墙状态为 %s" % state,
                   ["/usr/libexec/ApplicationFirewall/socketfilterfw", "--setglobalstate", state])]

    # ------------------------------------------------------------ 电源
    def audit_power(self):
        rc, out, _ = self._sh(["pmset", "-g", "custom"])
        block = out.split("Battery Power:")[-1].split("AC Power:")[0] if "Battery Power:" in out else out
        m = re.search(r"powernap\s+(\d)", block)
        battery_powernap = int(m.group(1)) if m else None
        findings, plan = [], []
        if battery_powernap == 1:
            findings.append(finding(
                "power.powernap_on", "warn",
                "电池供电下 Power Nap 开启，睡眠中会周期性唤醒并重建网络栈"))
            plan.append(op("电池下关闭 Power Nap",
                           ["pmset", "-b", "powernap", "0"],
                           undo=["pmset", "-b", "powernap", "1"]))
        return section("power", status=status_from(findings),
                       current={"powernap_battery": battery_powernap}, findings=findings, plan=plan)

    def restore_power(self, data):
        val = data.get("powernap_battery")
        if val in (0, 1):
            return [op("恢复 Power Nap 为 %d" % val, ["pmset", "-b", "powernap", str(val)])]
        return []
