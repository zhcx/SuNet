#!/usr/bin/env python3
"""netopt —— 跨平台网络配置巡检与优化命令行工具。

面向软件集成设计：
  · stdout 只输出结果，日志走 stderr，便于父进程直接解析
  · 默认只读巡检；变更必须显式 --commit
  · 退出码语义固定，见 netopt_core.EXIT_*
  · 零第三方依赖，Python 3.8+

示例：
  python3 netopt.py                          # 巡检，自动选择文本/JSON 输出
  python3 netopt.py --json audit --only dns  # 只查 DNS，输出 JSON
  python3 netopt.py apply                    # 预览修复计划（不改动）
  sudo python3 netopt.py apply --commit      # 真正执行
  python3 netopt.py --json rollback --to baseline --commit
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import netopt_core as core


def build_parser():
    # 公共参数同时挂到主命令和子命令上，让 --json audit 与 audit --json 都能用。
    # 子命令侧默认用 SUPPRESS，避免把主命令已解析的值覆盖回 None。
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--json", dest="fmt", action="store_const", const="json",
                        default=argparse.SUPPRESS, help="输出 JSON（管道下为默认）")
    common.add_argument("--text", dest="fmt", action="store_const", const="text",
                        default=argparse.SUPPRESS, help="输出人读文本")
    common.add_argument("--only", default=argparse.SUPPRESS,
                        help="只处理指定子系统，逗号分隔：%s" % ",".join(core.SECTION_ORDER))
    common.add_argument("--dns", default=argparse.SUPPRESS, help="指定的 DNS 目标，逗号分隔")
    common.add_argument("--ipv6-probe", dest="ipv6_probe", default=argparse.SUPPRESS,
                        help="IPv6 可达性探测目标地址")
    common.add_argument("--gateway", default=argparse.SUPPRESS, help="网关地址（默认自动探测）")

    p = argparse.ArgumentParser(
        prog="netopt",
        description="跨平台网络配置巡检与优化（macOS / Linux / Windows）",
        epilog="退出码：0 正常 · 10 有告警 · 20 有故障 · 30 需管理员 · 40 平台不支持 · 1 参数错误",
    )
    p.add_argument("--version", action="version", version="netopt %s" % core.VERSION)
    p.add_argument("--json", dest="fmt", action="store_const", const="json", help="输出 JSON")
    p.add_argument("--text", dest="fmt", action="store_const", const="text", help="输出人读文本")
    p.add_argument("--only", default=None, help="只处理指定子系统，逗号分隔")
    p.add_argument("--dns", default=None, help="指定的 DNS 目标，逗号分隔")
    p.add_argument("--ipv6-probe", dest="ipv6_probe", default=None, help="IPv6 探测目标")
    p.add_argument("--gateway", default=None, help="网关地址")

    sub = p.add_subparsers(dest="cmd")
    sub.add_parser("audit", parents=[common], help="只读巡检（默认动作）")

    ap = sub.add_parser("apply", parents=[common], help="执行修复计划；不带 --commit 只预览")
    ap.add_argument("--commit", action="store_true", help="真正写入变更")

    rb = sub.add_parser("rollback", parents=[common], help="从快照回滚")
    rb.add_argument("--to", choices=["latest", "baseline"], default="latest")
    rb.add_argument("--commit", action="store_true", help="真正执行回退")

    sub.add_parser("capabilities", parents=[common], help="列出本机各子系统支持情况")
    return p


def emit(doc, fmt):
    if fmt == "json":
        print(json.dumps(doc, ensure_ascii=False, indent=2))
    else:
        print(core.render_human(doc))


def parse_only(raw):
    if not raw:
        return None
    wanted = [x.strip() for x in raw.split(",") if x.strip()]
    unknown = [x for x in wanted if x not in core.SECTION_ORDER]
    if unknown:
        raise SystemExit("未知子系统：%s（可选：%s）"
                         % (", ".join(unknown), ", ".join(core.SECTION_ORDER)))
    return wanted


def main(argv=None):
    args = build_parser().parse_args(argv)
    fmt = args.fmt or ("json" if not sys.stdout.isatty() else "text")

    config = {}
    if args.dns:
        config["dns"] = [x.strip() for x in args.dns.split(",") if x.strip()]
    if args.ipv6_probe:
        config["ipv6_probe"] = args.ipv6_probe
    if args.gateway:
        config["gateway"] = args.gateway

    backend = core.load_backend(config)
    if backend is None:
        doc = {
            "schema": core.SCHEMA, "version": core.VERSION, "generated_at": core.now_iso(),
            "mode": args.cmd or "audit", "platform": {"os": sys.platform},
            "error": "当前平台尚未实现后端", "exit_code": core.EXIT_UNSUPPORTED,
        }
        emit(doc, fmt)
        return core.EXIT_UNSUPPORTED

    only = parse_only(args.only)
    cmd = args.cmd or "audit"

    if cmd == "capabilities":
        caps = []
        for sid in core.SECTION_ORDER:
            if only and sid not in only:
                continue
            implemented = (hasattr(backend, "audit_" + sid)
                           and sid not in getattr(backend, "UNIMPLEMENTED", ()))
            caps.append({"id": sid, "title": core.TITLES[sid],
                         "supported": implemented,
                         "restorable": hasattr(backend, "restore_" + sid)})
        doc = {
            "schema": core.SCHEMA, "version": core.VERSION, "generated_at": core.now_iso(),
            "mode": "capabilities", "platform": core.platform_info(backend),
            "capabilities": caps,
            "exit_code": core.EXIT_OK if any(c["supported"] for c in caps) else core.EXIT_UNSUPPORTED,
        }
        emit(doc, fmt)
        return doc["exit_code"]

    if cmd == "rollback":
        snap, path = core.load_snapshot(args.to)
        if not snap:
            doc = {"schema": core.SCHEMA, "version": core.VERSION, "generated_at": core.now_iso(),
                   "mode": "rollback", "platform": core.platform_info(backend),
                   "error": "没有可用快照（%s）" % args.to, "exit_code": core.EXIT_USAGE}
            emit(doc, fmt)
            return core.EXIT_USAGE
        ops = core.rollback_ops(backend, snap)
        records = []
        for item in ops:
            if not args.commit:
                records.append({"desc": item["desc"], "result": "planned"})
                continue
            if not core.is_admin():
                records.append({"desc": item["desc"], "result": "skipped_need_admin"})
                continue
            rc, out, err = core.run(item["argv"], timeout=30)
            records.append({"desc": item["desc"], "result": "applied" if rc == 0 else "error",
                            "exit_code": rc, "stderr": err[:300]})
        doc = {"schema": core.SCHEMA, "version": core.VERSION, "generated_at": core.now_iso(),
               "mode": "rollback", "platform": core.platform_info(backend),
               "snapshot": path, "actions": records,
               "exit_code": core.EXIT_OK if all(r["result"] in ("applied", "planned") for r in records)
               else core.EXIT_FAIL}
        emit(doc, fmt)
        return doc["exit_code"]

    sections = core.audit(backend, only)

    if cmd == "audit":
        doc = core.build_document("audit", backend, sections)
        emit(doc, fmt)
        return doc["exit_code"]

    # apply
    admin = core.is_admin()
    total_ops = sum(len(s["plan"]) for s in sections)
    needs_admin = any(item.get("needs_admin") for s in sections for item in s["plan"])
    if args.commit and needs_admin and not admin:
        doc = core.build_document("apply", backend, sections,
                                  error="需要管理员权限才能执行变更")
        doc["exit_code"] = core.EXIT_NEED_ADMIN
        emit(doc, fmt)
        return core.EXIT_NEED_ADMIN

    snapshot = None
    if args.commit and total_ops:
        snapshot = core.save_snapshot(sections)
    records = core.execute(backend, sections, commit=args.commit, admin=admin)
    after = core.audit(backend, only) if args.commit else sections

    doc = core.build_document("apply" if args.commit else "apply-preview", backend, after,
                              records=records, snapshot=snapshot)
    emit(doc, fmt)
    return doc["exit_code"]


if __name__ == "__main__":
    sys.exit(main())
