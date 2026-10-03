"""Compare production probe results with required behavior, not with known bugs."""
import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("results", type=Path)
    parser.add_argument("--desktop-evidence", type=Path)
    args = parser.parse_args()
    cases = {case["case"]: case for case in json.loads(args.results.read_text(encoding="utf-8-sig"))}
    checks = []
    direct = cases["explicit_values_with_context"]["output"]
    for value in ["阻断问题为0", "进行中", "测试环境就绪"]:
        checks.append(("explicit_field:" + value, value in direct))
    people = cases["confirmed_people_but_missing_in_output"]["output"]
    checks.append(("confirmed_people", all(name in people for name in ["林舟", "陈岚", "顾然"])))
    saved = cases.get("desktop_saved_metadata_to_summary_context")
    if saved:
        checks.append(("default_attendance", len(saved["summary_context"]["verified_meeting_facts"]["attending"]) == 3))
    if args.desktop_evidence:
        block = (args.desktop_evidence / "block-menu-before.txt").read_text(encoding="utf-8")
        insert = (args.desktop_evidence / "insert-menu-before-stable.txt").read_text(encoding="utf-8")
        checks.append(("zh_block_menu", "菜单项 删除" in block and "菜单项 颜色" in block))
        checks.append(("zh_insert_menu", "一级标题" in insert and "Top-level heading" not in insert))
    for label, passed in checks:
        print(("PASS " if passed else "FAIL ") + label)
    return 0 if all(passed for _, passed in checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
