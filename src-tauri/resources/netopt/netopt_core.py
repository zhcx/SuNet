"""netopt 核心：数据模型、调度、执行、快照、输出。

设计约束（供软件集成）：
  · 零第三方依赖，Python 3.8+ 即可运行
  · stdout 只输出结果（JSON 或人读文本），日志一律走 stderr
  · 不用 shell 执行外部命令，全部 argv 数组 + 超时
  · 后端只产出"操作计划"，执行与快照统一在这里做，三个平台行为一致
"""

from __future__ import annotations

import json
import os
import random
import socket
import struct
import sys
import subprocess
import time
from datetime import datetime

SCHEMA = "netopt/v1"
VERSION = "1.0.0"

# 退出码：软件侧据此分支，不依赖字符串
EXIT_OK = 0            # 全部正常
EXIT_USAGE = 1         # 参数错误
EXIT_WARN = 10         # 有告警，但可继续
EXIT_FAIL = 20         # 有失败项
EXIT_NEED_ADMIN = 30   # 需要管理员权限才能 apply
EXIT_UNSUPPORTED = 40  # 当前平台或子系统不支持

SECTION_ORDER = ("dns", "ipv6", "proxy", "firewall", "power")

TITLES = {
    "dns": "DNS 解析器",
    "ipv6": "IPv6 地址与前缀",
    "proxy": "系统代理与绕过",
    "firewall": "主机防火墙",
    "power": "电源与唤醒",
}


# ---------------------------------------------------------------- 基础工具

def now_iso() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def log(msg: str) -> None:
    print(msg, file=sys.stderr)


def run(argv, timeout=15):
    """执行外部命令，返回 (returncode, stdout, stderr)。绝不经过 shell。"""
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
        return p.returncode, (p.stdout or "").strip(), (p.stderr or "").strip()
    except FileNotFoundError:
        return 127, "", "command not found: %s" % argv[0]
    except subprocess.TimeoutExpired:
        return 124, "", "timeout after %ss" % timeout
    except OSError as exc:
        return 126, "", str(exc)


def finding(code, level, message, **extra):
    """结构化发现项。code 供软件匹配，message 供人读。"""
    item = {"code": code, "level": level, "message": message}
    item.update(extra)
    return item


def dns_probe(server, name="www.baidu.com", timeout=2.0):
    """纯 socket 发一个 A 查询，不依赖 dig/nslookup，三平台通用。

    返回 (alive, rtt_ms, detail)。rcode 3(NXDOMAIN) 说明服务器活着，算存活。
    """
    tid = random.randint(0, 0xFFFF)
    header = struct.pack("!HHHHHH", tid, 0x0100, 1, 0, 0, 0)
    qname = b"".join(bytes([len(p)]) + p.encode() for p in name.split(".") if p) + b"\x00"
    query = header + qname + struct.pack("!HH", 1, 1)
    fam = socket.AF_INET6 if ":" in server else socket.AF_INET
    sock = socket.socket(fam, socket.SOCK_DGRAM)
    sock.settimeout(timeout)
    try:
        started = time.perf_counter()
        sock.sendto(query, (server, 53))
        data, _ = sock.recvfrom(512)
        rtt = (time.perf_counter() - started) * 1000
        rcode = struct.unpack("!H", data[2:4])[0] & 0xF
        return rcode in (0, 3), round(rtt, 1), "rcode=%d" % rcode
    except socket.timeout:
        return False, None, "timeout"
    except OSError as exc:
        return False, None, str(exc)
    finally:
        sock.close()


def op(desc, argv, needs_admin=True, undo=None):
    """一条待执行操作。undo 为可回退时使用的 argv。"""
    return {"desc": desc, "argv": argv, "needs_admin": needs_admin, "undo": undo}


def section(sid, *, supported=True, status="ok", current=None, recommended=None,
            findings=None, plan=None, reason=None):
    return {
        "id": sid,
        "title": TITLES.get(sid, sid),
        "supported": supported,
        "status": "unsupported" if not supported else status,
        "current": current or {},
        "recommended": recommended or {},
        "findings": findings or [],
        "plan": plan or [],
        "unsupported_reason": reason,
    }


def status_from(findings):
    """按发现项级别收敛状态：fail > warn > ok。info 只作说明，不影响状态。"""
    levels = {f.get("level") for f in findings or []}
    if "fail" in levels:
        return "fail"
    if "warn" in levels:
        return "warn"
    return "ok"


def worst(statuses):
    order = {"unsupported": 0, "ok": 0, "warn": 1, "fail": 2}
    return max(statuses, key=lambda s: order.get(s, 0)) if statuses else "ok"


# ---------------------------------------------------------------- 权限与平台

def is_admin() -> bool:
    if os.name == "nt":
        try:
            import ctypes
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except Exception:
            return False
    try:
        return os.geteuid() == 0
    except AttributeError:
        return False


def load_backend(config=None):
    if sys.platform == "darwin":
        import backend_macos as m
    elif os.name == "nt":
        import backend_windows as m
    elif sys.platform.startswith("linux"):
        import backend_linux as m
    else:
        return None
    return m.Backend(config)


def platform_info(backend):
    import platform as p
    return {
        "os": sys.platform,
        "release": p.release(),
        "arch": p.machine(),
        "python": p.python_version(),
        "elevated": is_admin(),
        "backend": getattr(backend, "id", "none"),
    }


# ---------------------------------------------------------------- 快照

def state_dir() -> str:
    if sys.platform == "darwin":
        base = os.path.expanduser("~/Library/Application Support")
    elif os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    else:
        base = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    path = os.path.join(base, "netopt")
    os.makedirs(path, exist_ok=True)
    return path


def save_snapshot(sections) -> str:
    d = os.path.join(state_dir(), "snapshots")
    os.makedirs(d, exist_ok=True)
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    data = {
        "schema": SCHEMA,
        "taken_at": now_iso(),
        "sections": {s["id"]: s["current"] for s in sections},
    }
    path = os.path.join(d, stamp + ".json")
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, ensure_ascii=False, indent=2)
    baseline = os.path.join(d, "baseline.json")
    if not os.path.exists(baseline):
        with open(baseline, "w", encoding="utf-8") as fh:
            json.dump(data, fh, ensure_ascii=False, indent=2)
    # 只保留最近 10 份，baseline 单独存放不受影响
    files = sorted(f for f in os.listdir(d) if f.endswith(".json") and f != "baseline.json")
    for old in files[:-10]:
        try:
            os.remove(os.path.join(d, old))
        except OSError:
            pass
    return path


def load_snapshot(which="latest"):
    d = os.path.join(state_dir(), "snapshots")
    if which == "baseline":
        path = os.path.join(d, "baseline.json")
    else:
        files = sorted(f for f in os.listdir(d)) if os.path.isdir(d) else []
        files = [f for f in files if f.endswith(".json") and f != "baseline.json"]
        if not files:
            return None, None
        path = os.path.join(d, files[-1])
    if not os.path.exists(path):
        return None, None
    with open(path, encoding="utf-8") as fh:
        return json.load(fh), path


# ---------------------------------------------------------------- 审计 / 执行

def audit(backend, only=None):
    secs = []
    for sid in SECTION_ORDER:
        if only and sid not in only:
            continue
        fn = getattr(backend, "audit_" + sid, None)
        if fn is None:
            secs.append(section(sid, supported=False, reason="当前平台未实现该检查"))
            continue
        try:
            secs.append(fn())
        except Exception as exc:  # 单个子系统出错不拖垮整体
            secs.append(section(
                sid, supported=True, status="fail",
                findings=[finding("section.error", "fail", "检查失败：%s" % exc)],
            ))
    return secs


def execute(backend, sections, commit: bool, admin: bool):
    """按计划执行。commit=False 只预览。返回执行记录。"""
    records = []
    for sec in sections:
        for item in sec["plan"]:
            rec = {"section": sec["id"], "desc": item["desc"], "argv": item["argv"]}
            if not commit:
                rec["result"] = "planned"
                records.append(rec)
                continue
            if item.get("needs_admin") and not admin:
                rec["result"] = "skipped_need_admin"
                records.append(rec)
                continue
            rc, out, err = run(item["argv"], timeout=30)
            rec["exit_code"] = rc
            rec["stdout"] = out[:500]
            rec["stderr"] = err[:500]
            rec["result"] = "applied" if rc == 0 else "error"
            records.append(rec)
    return records


def rollback_ops(backend, snap):
    """根据快照生成回退操作。各后端按自身能力实现。"""
    ops = []
    for sid, data in (snap.get("sections") or {}).items():
        fn = getattr(backend, "restore_" + sid, None)
        if fn:
            try:
                ops.extend(fn(data) or [])
            except Exception as exc:
                log("回退 %s 失败：%s" % (sid, exc))
    return ops


# ---------------------------------------------------------------- 输出

def summarize(sections):
    counts = {"ok": 0, "warn": 0, "fail": 0, "unsupported": 0}
    for s in sections:
        counts[s["status"]] = counts.get(s["status"], 0) + 1
    return counts


def exit_code_for(sections):
    statuses = [s["status"] for s in sections]
    if "fail" in statuses:
        return EXIT_FAIL
    if "warn" in statuses:
        return EXIT_WARN
    if statuses and all(s == "unsupported" for s in statuses):
        return EXIT_UNSUPPORTED
    return EXIT_OK


def build_document(mode, backend, sections, *, records=None, snapshot=None, error=None):
    doc = {
        "schema": SCHEMA,
        "version": VERSION,
        "generated_at": now_iso(),
        "mode": mode,
        "platform": platform_info(backend),
        "summary": summarize(sections),
        "sections": sections,
        "exit_code": exit_code_for(sections),
    }
    if records is not None:
        doc["actions"] = records
    if snapshot:
        doc["snapshot"] = snapshot
    if error:
        doc["error"] = error
    return doc


def render_human(doc) -> str:
    lines = []
    p = doc["platform"]
    lines.append("netopt %s  ·  %s %s (%s)%s" % (
        doc["version"], p["os"], p["release"], p["arch"],
        "  [管理员]" if p["elevated"] else "",
    ))
    lines.append("-" * 64)
    for s in doc["sections"]:
        mark = {"ok": "正常", "warn": "注意", "fail": "故障", "unsupported": "不支持"}[s["status"]]
        lines.append("[%s] %s" % (mark, s["title"]))
        if not s["supported"]:
            lines.append("       %s" % (s["unsupported_reason"] or "未实现"))
            continue
        for f in s["findings"]:
            lines.append("       · %s" % f["message"])
        for item in s["plan"]:
            lines.append("       → %s" % item["desc"])
    if doc.get("actions"):
        lines.append("-" * 64)
        for a in doc["actions"]:
            lines.append("  %-22s %s" % (a["result"], a["desc"]))
    sm = doc["summary"]
    lines.append("-" * 64)
    lines.append("正常 %d · 注意 %d · 故障 %d · 不支持 %d" % (
        sm.get("ok", 0), sm.get("warn", 0), sm.get("fail", 0), sm.get("unsupported", 0)))
    return "\n".join(lines)
