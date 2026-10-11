#!/usr/bin/env python3
"""R7 依赖方向机器化：把 docs/REFACTOR_20260927.md 的模块依赖规则变成 CI 断言。

规则（违例即 exit 1）：
1. turn/ 与 protocol/ 的部件模块不得引用 `entry::` —— entry 是编排入口，
   只能被别人调用，不能被部件反向依赖（mod.rs 的 `pub use entry::*;` 转发除外）。
2. turn/ 叶子模块（leak_strip / bg_llm / session_repair / prompt_ctx）不得显式
   import 兄弟 turn 模块 —— 叶子只经 `use super::super::*` glob 桥接收数据。
3. pre_refine 与 post_parse 互不引用 —— 两阶段之间只经 entry 传数据。
"""
import os
import re
import sys

ROOT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "kaleido-server", "src", "story_tavern", "parts",
)
TURN = os.path.join(ROOT, "turn")
PROTOCOL = os.path.join(ROOT, "protocol")

LEAVES = ("leak_strip.rs", "bg_llm.rs", "session_repair.rs", "prompt_ctx.rs")
SIBLING_MODULES = (
    "bg_llm", "checks", "clock", "ctx", "director_exec", "entry", "leak_strip",
    "memory", "persist", "post_parse", "pre_refine", "prompt_ctx",
    "session_repair", "stream",
)

violations = []


def check(path, rules):
    with open(path, encoding="utf-8", errors="replace") as fh:
        for i, line in enumerate(fh, 1):
            for rule in rules:
                if re.search(rule[0], line):
                    violations.append(
                        f"{os.path.relpath(path, os.path.dirname(ROOT))}:{i} "
                        f"[{rule[1]}] {line.strip()[:120]}"
                    )


# 规则 1：entry:: 反向引用（mod.rs 转发行除外）
for d in (TURN, PROTOCOL):
    if not os.path.isdir(d):
        continue
    for f in os.listdir(d):
        if not f.endswith(".rs"):
            continue
        if d == TURN and f == "mod.rs":
            continue  # mod.rs 持有合法的 pub use entry::*;
        check(os.path.join(d, f), [(r"\bentry::", "R7-1 entry:: 反向引用")])

# 规则 2：叶子模块 import 兄弟模块（explicit `xxx::` 路径，glob 桥接不在此列）
for f in LEAVES:
    p = os.path.join(TURN, f)
    if os.path.isfile(p):
        for sib in SIBLING_MODULES:
            if sib == f.removesuffix(".rs"):
                continue
            check(p, [(rf"\b{sib}::", f"R7-2 叶子模块引用兄弟模块 {sib}")])

# 规则 3：pre_refine 与 post_parse 互不引用
for a, b in (("pre_refine.rs", "post_parse"), ("post_parse.rs", "pre_refine")):
    p = os.path.join(TURN, a)
    if os.path.isfile(p):
        check(p, [(rf"\b{b}\b", f"R7-3 {a} 引用 {b}")])

if violations:
    print(f"dependency-direction violations: {len(violations)}")
    for v in violations:
        print(" ", v)
    sys.exit(1)
print("dependency-direction check: OK "
      f"(turn/={len(os.listdir(TURN))} files, protocol/={len(os.listdir(PROTOCOL))} files)")
