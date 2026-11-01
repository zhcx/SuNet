"""Linux 后端：ip / resolvectl / nmcli / ufw / firewalld / gsettings。

与 macOS 后端保持同样的产出契约：只读状态、出计划、给回退命令。
不同发行版能力差异较大，缺失的子系统一律标记 unsupported 而不是报错。
"""

from __future__ import annotations

import os
import re
import shutil

from netopt_core import dns_probe, finding, op, run, section, status_from


class Backend:
    id = "linux"
    # capabilities 用：stub 仍保留（能给出具体原因），但不算"已实现"
    UNIMPLEMENTED = ("power",)

    def __init__(self, config=None):
        self.config = config or {}
        self.device = self._default_device()
        self.dns_flavor = self._detect_dns()
        self.conn_name = self._connection_name()

    # ------------------------------------------------------------ 探测辅助
    @staticmethod
    def _default_device():
        rc, out, _ = run(["ip", "route", "show", "default"])
        m = re.search(r"\bdev\s+(\S+)", out)
        return m.group(1) if m else None

    def _detect_dns(self):
        if shutil.which("resolvectl"):
            return "resolvectl"
        if shutil.which("nmcli"):
            return "nmcli"
        if os.path.exists("/etc/resolv.conf"):
            return "resolvconf"
        return "none"

    def _connection_name(self):
        rc, out, _ = run(["nmcli", "-t", "-f", "NAME,DEVICE", "connection", "show", "--active"])
        for line in out.splitlines():
            parts = line.split(":")
            if len(parts) >= 2 and parts[1] == self.device:
                return parts[0]
        return None

    # ------------------------------------------------------------ DNS
    def _dns_servers(self):
        if self.dns_flavor == "resolvectl" and self.device:
            rc, out, _ = run(["resolvectl", "status", self.device])
            for line in out.splitlines():
                if "DNS Servers:" in line:
                    return re.findall(r"[0-9a-fA-F:.]+:[0-9a-fA-F:]+|\d+\.\d+\.\d+\.\d+",
                                      line.split(":", 1)[1]), "systemd-resolved"
            return [], "systemd-resolved"
        if os.path.exists("/etc/resolv.conf"):
            servers = []
            with open("/etc/resolv.conf", encoding="utf-8", errors="ignore") as fh:
                for line in fh:
                    if line.strip().startswith("nameserver"):
                        servers.append(line.split()[1])
            return servers, "resolv.conf"
        return [], "none"

    def audit_dns(self):
        servers, source = self._dns_servers()
        if not servers:
            return section("dns", status="warn", current={"servers": [], "source": source},
                           findings=[finding("dns.none", "warn", "未读到任何解析器配置")])

        measured, findings = [], []
        for srv in servers[:4]:
            alive, rtt, detail = dns_probe(srv)
            measured.append({"server": srv, "alive": alive, "rtt_ms": rtt, "detail": detail})
            if not alive:
                findings.append(finding("dns.unreachable", "fail",
                                        "%s 无响应（%s）" % (srv, detail), server=srv))
            elif rtt and rtt > 150:
                findings.append(finding("dns.slow", "warn",
                                        "%s 响应 %.0fms，偏慢" % (srv, rtt), server=srv))

        reachable = [m["server"] for m in measured if m["alive"]]
        dead = [m["server"] for m in measured if not m["alive"]]
        target = self.config.get("dns") or (reachable if dead and reachable else None)

        plan = []
        if target and target != servers:
            if self.dns_flavor == "resolvectl" and self.device:
                plan.append(op("设置 %s 的 DNS 为 %s" % (self.device, ", ".join(target)),
                               ["resolvectl", "dns", self.device] + list(target),
                               undo=["resolvectl", "dns", self.device] + (servers or ["''"])))
                plan.append(op("刷新解析缓存", ["resolvectl", "flush-caches"], needs_admin=False))
            elif self.dns_flavor == "nmcli" and self.conn_name:
                plan.append(op("修改连接 %s 的 DNS" % self.conn_name,
                               ["nmcli", "connection", "modify", self.conn_name,
                                "ipv4.dns", " ".join(target)]))
                plan.append(op("重新应用连接", ["nmcli", "connection", "up", self.conn_name]))
        elif dead and not reachable and not target:
            findings.append(finding("dns.no_fallback", "warn", "全部解析器失效且未提供替代，可用 --dns 指定"))

        if self.dns_flavor == "resolvconf":
            findings.append(finding("dns.managed_externally", "info",
                                    "由 /etc/resolv.conf 管理，本工具只读不改写"))
        status = status_from(findings)
        return section("dns", status=status, current={
            "servers": servers, "source": source, "measured": measured,
        }, recommended={"servers": target or reachable}, findings=findings, plan=plan)

    def restore_dns(self, data):
        servers = data.get("servers") or []
        if not servers:
            return []
        if self.dns_flavor == "resolvectl" and self.device:
            return [op("恢复 DNS", ["resolvectl", "dns", self.device] + list(servers))]
        if self.dns_flavor == "nmcli" and self.conn_name:
            return [op("恢复 DNS", ["nmcli", "connection", "modify", self.conn_name,
                                    "ipv4.dns", " ".join(servers)])]
        return []

    # ------------------------------------------------------------ IPv6
    def audit_ipv6(self):
        if not self.device:
            return section("ipv6", supported=False, reason="未找到默认路由网卡")
        rc, out, _ = run(["ip", "-6", "addr", "show", "dev", self.device, "scope", "global"])
        addrs = re.findall(r"inet6\s+([0-9a-fA-F:]+)/(\d+)", out)
        if not addrs:
            return section("ipv6", status="warn", current={"addresses": []},
                           findings=[finding("ipv6.none", "warn", "未获取到全局 IPv6 地址")])

        prefixes = {}
        for addr, plen in addrs:
            prefixes.setdefault(":".join(addr.split(":")[:4]), []).append((addr, plen))

        target = self.config.get("ipv6_probe", "2606:4700:4700::1111")
        state, findings, plan = {}, [], []
        for pfx, group in prefixes.items():
            alive = False
            for addr, _plen in group[:2]:
                rc, out2, _ = run(["ping", "-6", "-c", "1", "-W", "2", "-S", addr, target], timeout=5)
                if rc == 0 and " 1 received" in out2.replace("packets", ""):
                    alive = True
                    break
            state[pfx] = {"addresses": ["%s/%s" % g for g in group], "reachable": alive}

        dead = [p for p, v in state.items() if not v["reachable"]]
        if len(dead) == len(state):
            findings.append(finding("ipv6.all_unreachable", "fail",
                                    "所有 IPv6 前缀均不可达，判定为出口故障，不建议删地址"))
        elif dead:
            for p in dead:
                findings.append(finding("ipv6.stale_prefix", "warn",
                                        "前缀 %s::/64 已失效但仍挂着地址" % p, prefix=p))
                for addr, plen in prefixes[p]:
                    plan.append(op("删除失效地址 %s" % addr,
                                   ["ip", "-6", "addr", "del", "%s/%s" % (addr, plen), "dev", self.device]))

        status = "fail" if len(dead) == len(state) else ("warn" if dead else "ok")
        return section("ipv6", status=status, current={
            "addresses": state, "probe_target": target,
        }, findings=findings, plan=plan)

    # ------------------------------------------------------------ 代理
    def audit_proxy(self):
        if not shutil.which("gsettings"):
            return section("proxy", supported=False,
                           reason="图形环境无 gsettings，系统代理需桌面环境支持")
        rc, out, err = run(["gsettings", "get", "org.gnome.system.proxy", "mode"])
        if rc != 0:
            return section("proxy", supported=False,
                           reason="gsettings 不可用（非 GNOME 桌面或缺 schema）：%s" % (err[:80] or rc))
        mode = out.strip().strip("'")
        rc2, host, _ = run(["gsettings", "get", "org.gnome.system.proxy.https", "host"])
        host = host.strip().strip("'")
        findings, plan = [], []
        if mode == "none":
            findings.append(finding("proxy.disabled", "info", "系统代理未启用（mode=none）"))
        if host and not re.match(r"^(\d{1,3}\.){3}\d{1,3}$", host):
            findings.append(finding("proxy.host_not_ip", "info",
                                    "代理主机为域名 %s，绕过列表需手工维护" % host))
        status = status_from(findings)
        return section("proxy", status=status, current={"mode": mode, "https_host": host},
                       findings=findings, plan=plan)

    # ------------------------------------------------------------ 防火墙
    def audit_firewall(self):
        rc, out, _ = run(["ufw", "status"])
        if "Status:" in out:
            active = "active" in out
            plan = [] if active else [op("启用 ufw", ["ufw", "--force", "enable"],
                                         undo=["ufw", "disable"])]
            findings = [] if active else [finding("firewall.disabled", "warn", "ufw 未启用")]
            return section("firewall", status=status_from(findings),
                           current={"backend": "ufw", "enabled": active},
                           findings=findings, plan=plan)
        rc2, out2, _ = run(["firewall-cmd", "--state"])
        if "running" in out2:
            return section("firewall", status="ok",
                           current={"backend": "firewalld", "enabled": True})
        if "not running" in out2:
            return section("firewall", status="warn",
                           current={"backend": "firewalld", "enabled": False},
                           findings=[finding("firewall.disabled", "warn", "firewalld 未运行")],
                           plan=[op("启动 firewalld", ["systemctl", "enable", "--now", "firewalld"])])
        return section("firewall", supported=False, reason="未检测到 ufw/firewalld")

    def restore_firewall(self, data):
        if data.get("backend") == "ufw":
            return [op("恢复 ufw 状态", ["ufw", "enable" if data.get("enabled") else "disable"])]
        return []

    # ------------------------------------------------------------ 电源
    def audit_power(self):
        return section("power", supported=False, reason="Linux 无 Power Nap 对应机制")
