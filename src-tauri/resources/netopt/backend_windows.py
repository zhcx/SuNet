"""Windows 后端：PowerShell / netsh / 注册表。

约定与另两个平台一致。Windows 的 DNS 按"接口索引"生效，
所以先定位默认路由所在接口，再对该接口读写。
"""

from __future__ import annotations

import fnmatch
import json

from netopt_core import dns_probe, finding, op, run, section, status_from

PS = ["powershell", "-NoProfile", "-NonInteractive", "-Command"]
PROXY_KEY = r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings"

# PowerShell 5.1 的 stdout 编码跟随控制台代码页（中文系统是 GBK），
# 而 SuNet 按 UTF-8 解码；界面拿到的是 "以太网" 这类中文，一旦编码对不上
# 整个 JSON 解析就失败。显式切成 UTF-8，不依赖用户控制台的代码页。
PS_UTF8 = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; "

# 改完 WinINET 设置后必须通知系统（InternetSetOption 39 / 37），否则 Chromium
# 等应用会继续用启动时缓存的那份配置，表现为"改了没生效"。
# Add-Type 在受限环境可能编译失败，所以整段 try/catch：通知失败不影响设置本身。
_NOTIFY = (
    "$sig='[DllImport(\"wininet.dll\",SetLastError=true)] public static extern "
    "bool InternetSetOption(IntPtr h,int o,IntPtr b,int l);';"
    "try{Add-Type -MemberDefinition $sig -Name WinInet -Namespace Native;"
    "[void][Native.WinInet]::InternetSetOption([IntPtr]::Zero,39,[IntPtr]::Zero,0);"
    "[void][Native.WinInet]::InternetSetOption([IntPtr]::Zero,37,[IntPtr]::Zero,0)}catch{}"
)


def _ps_quote(value):
    """PowerShell 单引号字符串转义：单引号写两遍。"""
    return str(value).replace("'", "''")


def _bypassed(patterns, item):
    """绕过列表是否已覆盖该地址。

    Windows 的 ProxyOverride 支持通配（默认配置就是 `localhost;127.*;<local>`）：
    按字符串相等判断会把 `127.*` 误报成"缺少 127.0.0.1"，于是每次一键优化都
    白弹一次 UAC 去加一条重复项。
    """
    return any(fnmatch.fnmatch(item, p) for p in patterns)


def _as_bool(value):
    """Get-NetFirewallProfile 的 Enabled 在 PowerShell 5.1 的 JSON 里是 1/0。"""
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    return str(value).strip().lower() in ("true", "1", "on", "enabled")


def _override_argv(values):
    """写 ProxyOverride 的 argv（同时发 WinINET 设置变更通知）。"""
    script = "Set-ItemProperty '%s' -Name ProxyOverride -Value '%s';%s" % (
        PROXY_KEY, _ps_quote(";".join(values)), _NOTIFY)
    return PS + [script]


class Backend:
    id = "windows"
    # capabilities 用：stub 仍保留（能给出一句具体原因），但不算"已实现"
    UNIMPLEMENTED = ("ipv6", "power")

    def __init__(self, config=None):
        self.config = config or {}
        self.iface = self._default_iface()

    # ------------------------------------------------------------ 辅助
    @staticmethod
    def _ps(script, timeout=20):
        return run(PS + [PS_UTF8 + script], timeout=timeout)

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
        missing = [w for w in want if not _bypassed(override, w)]
        if enabled and missing:
            findings.append(finding("proxy.bypass_incomplete", "warn",
                                    "绕过列表缺少 %s" % ", ".join(missing), missing=missing))
            merged = override + missing
            # HKCU 是当前用户自己的注册表，不需要管理员权限
            plan.append(op("补全代理绕过列表", _override_argv(merged), needs_admin=False,
                           undo=_override_argv(override)))
        if not enabled:
            findings.append(finding("proxy.disabled", "info", "WinINET 代理未启用"))

        status = status_from(findings)
        return section("proxy", status=status, current={
            "enabled": enabled, "server": server, "bypass": override,
        }, recommended={"bypass": want}, findings=findings, plan=plan)

    def restore_proxy(self, data):
        override = data.get("bypass") or []
        return [op("恢复代理绕过列表", _override_argv(override), needs_admin=False)]

    # ------------------------------------------------------------ 防火墙
    def audit_firewall(self):
        # 不用 netsh 的文本输出：它是本地化的（中文系统输出「状态 启用」），
        # 而且中文按 OEM 代码页写管道。Get-NetFirewallProfile 直接给布尔值。
        rc, out, _ = self._ps(
            "ConvertTo-Json -InputObject @(Get-NetFirewallProfile | "
            "Select-Object Name, Enabled) -Compress")
        if rc != 0 or not out.startswith("["):
            return section("firewall", supported=False, reason="读取防火墙状态失败")
        try:
            profiles = json.loads(out)
        except ValueError:
            return section("firewall", supported=False, reason="解析防火墙状态失败")
        if isinstance(profiles, dict):
            profiles = [profiles]
        states = [p.get("Enabled") for p in profiles if isinstance(p, dict)]
        if not states:
            return section("firewall", supported=False, reason="读取防火墙状态失败")
        enabled = all(_as_bool(s) for s in states)
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
