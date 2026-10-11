# -*- coding: utf-8 -*-
"""Generate docs/INDEX.md — a categorized table of contents for the docs/ directory.

Usage:
    python scripts/gen_doc_index.py

The script walks ``docs/`` recursively, groups every file into a category
based on filename-prefix / keyword heuristics, and writes ``docs/INDEX.md``
with, for each entry: a relative markdown link, the first non-empty heading
(or first non-empty line, truncated to ~80 chars) and the file mtime date.

Runs on both Windows and Linux; all I/O is explicit UTF-8.
"""

from __future__ import annotations

import datetime
import re
import sys
from pathlib import Path
from typing import List, Optional, Tuple

REPO_ROOT = Path(__file__).resolve().parent.parent
DOCS_DIR = REPO_ROOT / "docs"
INDEX_PATH = DOCS_DIR / "INDEX.md"

TITLE_TRUNC = 80

# ----------------------------------------------------------------------------
# Category rules, evaluated in order. First match wins.
# Each rule: (category name, matcher) where matcher takes the filename
# (original case) and the lower-cased filename.
# ----------------------------------------------------------------------------

def _match_any(name_lower: str, keywords) -> bool:
    return any(k in name_lower for k in keywords)


def _top_dir(posix: str) -> str:
    """First path segment of the docs-relative posix path ('' if none)."""
    return posix.split("/", 1)[0] if "/" in posix else ""


CATEGORY_RULES = [
    # --- subdirectory based rules -------------------------------------------
    ("架构决策记录（ADR）", lambda n, l: _top_dir(l) == "adr"),
    ("任务简报（Briefs）", lambda n, l: _top_dir(l) == "briefs"),
    ("API 契约（Contracts）", lambda n, l: _top_dir(l) == "contracts"
        or "contract" in l),
    ("计划（Plans）", lambda n, l: _top_dir(l) == "plans"),
    # --- filename heuristics -------------------------------------------------
    ("审计与代码审查", lambda n, l: _match_any(l, [
        "audit", "审查", "code_review", "code-review", "inspect-"])),
    ("吞噬计划（TavernWeave）", lambda n, l: "吞噬" in n),
    ("UI 与前端体验", lambda n, l: n.startswith("UI_") or _match_any(l, [
        "immersive_ui", "webui", "motion_pass", "responsive", "appearance",
        "ui_整改"])),
    ("Story Tavern 与酒馆集成",
        lambda n, l: (len(n) > 2 and n.startswith("ST")
                      and (n[2].isdigit() or n[2] in "-_"))
        or _match_any(l, ["tavern", "shelf"])),
    ("作者区（AZ 系列）", lambda n, l: n.startswith("AZ") or "author" in l),
    ("工作流任务（W 系列）", lambda n, l: re.match(r"^W\d", n) is not None),
    ("S 系列阶段记录", lambda n, l: re.match(r"^S\d", n) is not None),
    ("阶段任务（P 系列）", lambda n, l: re.match(r"^P\d", n) is not None),
    ("桌面与移动端", lambda n, l: _match_any(l, [
        "android", "tauri", "desktop"])),
    ("部署运维与安全", lambda n, l: _match_any(l, [
        "deploy", "hardening", "security", "jobs_", "decisions"])),
    ("剧情与连续性分析", lambda n, l: _match_any(n, [
        "宿醉", "度蜜月", "罗盘", "原著", "连续性", "KISS", "PLOT"])),
    ("验收与验证记录", lambda n, l: re.match(r"^U\d", n) is not None
        or _match_any(l, [
            "verify", "smoke", "acceptance", "e2e", "gate", "checklist",
            "regression"])),
    ("计划与设计文档", lambda n, l: _match_any(l, [
        "plan", "任务书", "任务计划", "design", "spec", "sop"])),
    ("会话与进度日志", lambda n, l: _match_any(n, [
        "SESSION_", "LOG", "ROUNDUP", "REMAINING", "DAMAGE", "CHECKPOINT",
        "TODO", "历史迭代", "项目脉络"])),
    ("功能笔记与问题分析", lambda n, l: _match_any(l, [
        "notes", "analysis", "mojibake", "embed", "issue"])),
]

FALLBACK_CATEGORY = "其他文档"


def classify(rel_path: Path) -> str:
    name = rel_path.name
    lower = name.lower()
    posix = rel_path.as_posix().lower()
    for category, matcher in CATEGORY_RULES:
        try:
            if matcher(name, posix) or matcher(name, lower):
                return category
        except Exception:
            continue
    return FALLBACK_CATEGORY


def extract_title(path: Path) -> str:
    """First non-empty heading line, else first non-empty line, ~80 chars.

    Non-markdown files (json, ...) have no meaningful first line: use the stem.
    """
    if path.suffix.lower() not in (".md", ".markdown"):
        return path.stem
    try:
        with path.open("r", encoding="utf-8", errors="replace") as fh:
            for _ in range(200):  # only look at the first 200 lines
                line = fh.readline()
                if not line:
                    break
                stripped = line.strip()
                if not stripped:
                    continue
                # Prefer a markdown heading; strip leading #'s.
                if stripped.startswith("#"):
                    stripped = stripped.lstrip("#").strip()
                if not stripped:
                    continue
                # Collapse internal whitespace (incl. full-width) for tidiness.
                stripped = re.sub(r"\s+", " ", stripped)
                if len(stripped) > TITLE_TRUNC:
                    stripped = stripped[: TITLE_TRUNC - 1] + "…"
                return stripped
    except OSError:
        pass
    return path.stem


def mtime_date(path: Path) -> str:
    try:
        ts = path.stat().st_mtime
    except OSError:
        return "unknown"
    return datetime.datetime.fromtimestamp(ts).strftime("%Y-%m-%d")


def collect_files() -> List[Path]:
    files: List[Path] = []
    if not DOCS_DIR.is_dir():
        print(f"error: docs directory not found: {DOCS_DIR}", file=sys.stderr)
        return files
    for path in sorted(DOCS_DIR.rglob("*")):
        if not path.is_file():
            continue
        if path.name == INDEX_PATH.name and path.parent == DOCS_DIR:
            continue  # never index the index itself
        if "__pycache__" in path.parts:
            continue
        files.append(path)
    return files


def md_link(rel_path: Path) -> str:
    text = rel_path.as_posix()
    # Escape spaces for markdown links.
    return text.replace(" ", "%20")


def build_index() -> str:
    files = collect_files()

    # Group files by category.
    grouped: "dict[str, List[Tuple[Path, str, str]]]" = {}
    for path in files:
        rel = path.relative_to(REPO_ROOT) if REPO_ROOT in path.parents else path
        # Link relative to docs/ so INDEX.md links work on GitHub.
        docs_rel = path.relative_to(DOCS_DIR)
        grouped.setdefault(classify(docs_rel), []).append(
            (docs_rel, extract_title(path), mtime_date(path))
        )

    # Stable category order: as declared in rules, then fallback last,
    # then any stray categories alphabetically.
    declared = [c for c, _ in CATEGORY_RULES]
    order = declared + [FALLBACK_CATEGORY]
    present = [c for c in order if c in grouped]
    for stray in sorted(k for k in grouped if k not in order):
        present.append(stray)

    today = datetime.date.today().isoformat()
    lines: List[str] = []
    lines.append("# 文档索引（INDEX）")
    lines.append("")
    lines.append("> 本文件由脚本自动生成，请勿手工编辑。")
    lines.append("> 重新生成命令：`python scripts/gen_doc_index.py`")
    lines.append(f"> 生成时间：{today}　共收录文档 {len(files)} 篇。")
    lines.append("")
    lines.append("## 分类概览")
    lines.append("")
    lines.append("| 分类 | 数量 |")
    lines.append("| --- | ---: |")
    for cat in present:
        lines.append(f"| {cat} | {len(grouped[cat])} |")
    lines.append(f"| 合计 | {len(files)} |")
    lines.append("")

    for cat in present:
        entries = grouped[cat]
        lines.append(f"## {cat}（{len(entries)}）")
        lines.append("")
        for docs_rel, title, date in sorted(entries):
            link = md_link(docs_rel)
            lines.append(f"- [{docs_rel.as_posix()}]({link}) — {title}（{date}）")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    if not DOCS_DIR.is_dir():
        print(f"error: docs directory not found: {DOCS_DIR}", file=sys.stderr)
        return 1
    content = build_index()
    INDEX_PATH.write_text(content, encoding="utf-8", newline="\n")
    n_files = content.count("\n- [")
    print(f"OK: wrote {INDEX_PATH} ({n_files} entries)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
