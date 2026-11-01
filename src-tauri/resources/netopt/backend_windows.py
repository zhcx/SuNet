"""Windows 后端：PowerShell / netsh / 注册表。

约定与另两个平台一致。Windows 的 DNS 按"接口索引"生效，
所以先定位默认路由所在接口，再对该接口读写。
"""

from __future__ import annotations

import json
import re

from netopt_core import dns_probe, finding, op, run, section, status_from

PS = ["powershell", "-NoProfile", "-NonInteractive", "-Command"]
PROXY_KEY = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings"


class Backend:
    id = "windows"

    def __init__(self, config=None):
        self.config = config or {}
        self.iface = self._default_iface()

    # ------------------------------------------------------------ 辅助
    @staticmethod
    def _ps(script, timeout=20):
        return run(PS + [script], timeout=timeout)

    def _default_iface(self):
        script = (
            "$c = Get-NetIPConfiguration | Where-Object { $_.IPv4DefaultGateway -ne $null } "
            "| Select-Object -First 1; "
            "if ($c) { [pscustomobject]@{ alias=$c.InterfaceAlias; index=$c.InterfaceIndex; "
            "gw=$c.IPv4DefaultGateway.NextHop } | ConvertTo-Json -Compress }"
        )
        rc, out, err = self._ps(script)
        if rc == 0 and out.startswith("{"):
            try:
                return json.loads(out)
            except ValueError:
                return None
        return None

    def _dns_servers(self):
        if not self.iface:
            return []
        script = ("(Get-DnsClientServerAddress -InterfaceIndex %d -AddressFamily IPv4)"
                  ".ServerAddresses -join ','") % self.iface["index"]
        rc, out, _ = self._ps(script)
        if rc != 0 or not out:
            return []
        return [s for s in out.split(",") if s.strip()]

    # ------------------------------------------------------------ DNS
    def audit_dns(self):
        if not self.iface:
            return section("dns", supported=False, reason="未找到带默认网关的接口")
        servers = self._dns_servers()
        source = "manual" if servers else "dhcp"

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
            arr = ",".join("'%s'" % t for t in target)
            plan.append(op("设置接口 %d 的 DNS 为 %s" % (self.iface["index"], ", ".join(target)),
                           PS + ["Set-DnsClientServerAddress -InterfaceIndex %d -ServerAddresses @(%s)"
                                 % (self.iface["index"], arr)],
                           undo=PS + ["Set-DnsClientServerAddress -InterfaceIndex %d -ResetServerAddresses"
                                      % self.iface["index"]]))
        elif dead and not reachable and not target:
            findings.append(finding("dns.no_fallback", "warn", "全部解析器失效且未提供替代，可用 --dns 指定"))
        if len(servers) == 1:
            findings.append(finding("dns.single", "warn", "只有一台解析器，故障时无备份"))

        status = status_from(findings)
        return section("dns", status=status, current={
            "servers": servers, "source": source, "interface": self.iface, "measured": measured,
        }, recommended={"servers": target or reachable}, findings=findings, plan=plan)

    def restore_dns(self, data):
        if not self.iface:
            return []
        servers = data.get("servers") or []
        if not servers:
            return [op("恢复为自动获取 DNS",
                       PS + ["Set-DnsClientServerAddress -InterfaceIndex %d -ResetServerAddresses"
                             % self.iface["index"]])]
        arr = ",".join("'%s'" % s for s in servers)
        return [op("恢复 DNS 为 %s" % ", ".join(servers),
                   PS + ["Set-DnsClientServerAddress -InterfaceIndex %d -ServerAddresses @(%s)"
                         % (self.iface["index"], arr)])]

    # ------------------------------------------------------------ 代理
    def audit_proxy(self):
        script = ("$k = Get-ItemProperty '%s'; "
                  "[pscustomobject]@{ enable=$k.ProxyEnable; server=$k.ProxyServer; "
                  "override=$k.ProxyOverride } | ConvertTo-Json -Compress") % PROXY_KEY
        rc, out, _ = self._ps(script)
        if rc != 0 or not out.startswith("{"):
            return section("proxy", supported=False, reason="读取 WinINET 代理设置失败")
        try:
            data = json.loads(out)
        except ValueError:
            return section("proxy", supported=False, reason="解析代理设置失败")

        enabled = bool(data.get("enable"))
        server = (data.get("server") or "").strip()
        override = [x for x in (data.get("override") or "").split(";") if x]
        findings, plan = [], []

        want = ["localhost", "127.0.0.1", "<local>"]
        missing = [w for w in want if w not in override]
        if enabled and missing:
            findings.append(finding("proxy.bypass_incomplete", "warn",
                                    "绕过列表缺少 %s" % ", ".join(missing), missing=missing))
            merged = override + [m for m in missing if m not in override]
            plan.append(op("补全代理绕过列表",
                           PS + ["Set-ItemProperty '%s' -Name ProxyOverride -Value '%s'"
                                 % (PROXY_KEY, ";".join(merged))],
                           undo=PS + ["Set-ItemProperty '%s' -Name ProxyOverride -Value '%s'"
                                      % (PROXY_KEY, ";".join(override))]))
        if not enabled:
            findings.append(finding("proxy.disabled", "info", "WinINET 代理未启用"))

        status = status_from(findings)
        return section("proxy", status=status, current={
            "enabled": enabled, "server": server, "bypass": override,
        }, recommended={"bypass": want}, findings=findings, plan=plan)

    def restore_proxy(self, data):
        override = data.get("bypass") or []
        return [op("恢复代理绕过列表",
                   PS + ["Set-ItemProperty '%s' -Name ProxyOverride -Value '%s'"
                         % (PROXY_KEY, ";".join(override))])]

    # ------------------------------------------------------------ 防火墙
    def audit_firewall(self):
        rc, out, _ = run(["netsh", "advfirewall", "show", "allprofiles", "state"])
        states = re.findall(r"State\s+(ON|OFF)", out, re.I)
        if not states:
            return section("firewall", supported=False, reason="读取防火墙状态失败")
        enabled = all(s.upper() == "ON" for s in states)
        findings, plan = [], []
        if not enabled:
            findings.append(finding("firewall.disabled", "warn",
                                    "部分或全部配置文件的防火墙处于关闭状态"))
            plan.append(op("启用全部配置文件的防火墙",
                           ["netsh", "advfirewall", "set", "allprofiles", "state", "on"],
                           undo=["netsh", "advfirewall", "set", "allprofiles", "state", "off"]))
        return section("firewall", status=status_from(findings),
                       current={"enabled": enabled, "profiles": len(states)},
                       findings=findings, plan=plan)

    def restore_firewall(self, data):
        state = "on" if data.get("enabled") else "off"
        return [op("恢复防火墙状态为 %s" % state,
                   ["netsh", "advfirewall", "set", "allprofiles", "state", state])]

    # ------------------------------------------------------------ 未实现项
    def audit_ipv6(self):
        return section("ipv6", supported=False,
                       reason="Windows 逐接口 IPv6 地址管理与探测尚未实现")

    def audit_power(self):
        return section("power", supported=False,
                       reason="Windows 的待机策略与网络唤醒不通过该工具调整")
