#!/usr/bin/env python3
"""Generate the isolated D-11 diagnostic checkpoint and command logs."""

from __future__ import annotations

from datetime import datetime, timezone
import importlib.util
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from typing import Any


REPO = Path(__file__).resolve().parents[2]
AUDIT_SCRIPT = Path(__file__).with_name("d11_readiness_audit.py")
TEST_SCRIPT = Path(__file__).with_name("test_d11_readiness_audit.py")
SPEC = importlib.util.spec_from_file_location("d11_readiness_audit", AUDIT_SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load d11_readiness_audit")
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)
EVIDENCE_ROOT = REPO / AUDIT.EVIDENCE_DIRECTORY
LOG_ROOT = EVIDENCE_ROOT / "logs"
AUDIT_RESULT = EVIDENCE_ROOT / "audit-result.json"
COMMAND_RESULTS = EVIDENCE_ROOT / "command-results.json"
REPORT = EVIDENCE_ROOT / "D11-诊断检查点.md"
EVIDENCE_MANIFEST = EVIDENCE_ROOT / "evidence-manifest.json"


def _write_text(path: Path, payload: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(payload, encoding="utf-8", newline="\n")
    temporary.replace(path)


def _write_json(path: Path, document: dict[str, Any]) -> None:
    _write_text(path, json.dumps(document, ensure_ascii=False, indent=2) + "\n")


def _file_record(path: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return {
        "path": path.relative_to(REPO).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": digest.hexdigest().upper(),
    }


def _run(index: int, name: str, command: list[str]) -> dict[str, Any]:
    completed = subprocess.run(
        command,
        cwd=REPO,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
    )
    log_relative = Path("logs") / f"{index:02d}-{name}.log"
    output = completed.stdout
    if output and not output.endswith("\n"):
        output += "\n"
    _write_text(EVIDENCE_ROOT / log_relative, output)
    return {
        "name": name,
        "command": subprocess.list2cmdline(command),
        "exit_code": completed.returncode,
        "log_path": log_relative.as_posix(),
        "log_bytes": (EVIDENCE_ROOT / log_relative).stat().st_size,
    }


def _git_status_paths() -> list[str]:
    completed = subprocess.run(
        ["git", "-c", "core.quotepath=false", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=REPO,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    paths = []
    for line in completed.stdout.splitlines():
        if len(line) >= 4:
            paths.append(line[3:].replace("\\", "/"))
    evidence_paths = [
        path.relative_to(REPO).as_posix()
        for path in EVIDENCE_ROOT.rglob("*")
        if path.is_file()
    ]
    evidence_paths.append(EVIDENCE_MANIFEST.relative_to(REPO).as_posix())
    return sorted(set(paths + evidence_paths))


def _render_report(audit: dict[str, Any], commands: dict[str, Any]) -> str:
    frozen = audit["frozen_q00_registration"]
    cer = audit["q00_cer_implementation"]
    trace = audit["traceability"]
    missing_roles = "、".join(frozen["missing_checked_in_truth_roles"])
    field_rows = []
    for row in trace["fields"]:
        field_rows.append(
            f"| `{row['field']}` | `{row['availability']}` | {row['facts'][0]} |"
        )
    command_rows = []
    for row in commands["commands"]:
        command_rows.append(
            f"| `{row['name']}` | `{row['exit_code']}` | `{row['log_path']}` | `{row['log_bytes']}` |"
        )
    blocker_rows = [f"{index}. {item}" for index, item in enumerate(audit["blockers"], 1)]
    return f"""# D-11 实时转写基线诊断检查点

> 结论：本检查点已经完成当前无需硬件即可完成的核对，但还不能开始三次冷热实时基线。`D-11=IN_PROGRESS`，没有进入 I-11A，没有修改生产音频或转写行为，也没有把任何系统音频遗留项写成通过。

## 范围和起点

- 唯一任务：`D-11`。`P0-RA-01` 只作为计划别名，不是第二个任务。
- 起点分支：`{audit['git_origin']['required_base_branch']}`。
- 起点提交：`{audit['git_origin']['required_base_commit']}`。
- 当前检查点分支：`{audit['git_origin']['current_branch']}`。
- 起点校验：`started_from_required_commit={str(audit['git_origin']['started_from_required_commit']).lower()}`。
- 未读取正式 `com.meetily.ai` 数据，未读取被排除的未跟踪旧草案，未等待或执行硬件复测。

## 已核对的冻结事实

- Q-00 登记的源音频：`{frozen['source']['duration_seconds']}` 秒、`{frozen['source']['bytes']}` 字节、SHA-256 `{frozen['source']['sha256']}`。
- 冻结窗口：`{frozen['window']['start_seconds']}`～`{frozen['window']['end_seconds']}` 秒，共 `{frozen['window']['duration_seconds']}` 秒；16 kHz、单声道、16-bit PCM、`{frozen['window']['frame_count']}` 帧、`{frozen['window']['bytes']}` 字节、SHA-256 `{frozen['window']['sha256']}`。
- 已实际重算当前工作树中 `Q00-FROZEN-TRUTH-MANIFEST.json`：`{frozen['truth_manifest']['bytes']}` 字节、SHA-256 `{frozen['truth_manifest']['sha256']}`，与 Q-00 代码常量相等。
- 已实际重算当前工作树中 `Q00-NEGATIVE-TRUTH.json`：`{frozen['negative_truth']['bytes']}` 字节、SHA-256 `{frozen['negative_truth']['sha256']}`，与清单和 Q-00 代码常量相等。
- Q-00 代码冻结正例术语：`YouTube`、`PWA`、`Google`。已重算的负例文件包含：`M100`、`H5`、`VIP`、`A/B Test`、`TG`。
- 当前检出不含其余批准真值原件，缺少的清单角色为：{missing_roles}。因此源/窗口音频、人工逐字稿、说话人真值和正例术语原件只能核对登记值，不能在本检查点伪称已重算原件字节。

## Q-00 CER 实现

- Q-00 评分器：`{cer['scorer']['path']}`，`{cer['scorer']['bytes']}` 字节，SHA-256 `{cer['scorer']['sha256']}`。
- CER 实现由 `_normalize_cer_text`、`_levenshtein_distance`、`_cer` 三个函数组成；按 LF+UTF-8 拼接后的实现 SHA-256 为 `{cer['function_bundle_sha256_lf_utf8']}`。
- 规则仍是 NFKC、Unicode casefold、只保留 Unicode 字母和数字、按保留后的 Unicode code point 计算 Levenshtein 距离并除以真值字符数。
- 本检查点按 Q-00 公共报告中的 `cer_rule` 对象计算出规则 SHA-256 `{cer['derived_cer_normalization_rule_sha256']}`。这是 D-11 的可复算派生值；现有 Q-00 报告只写规则对象，尚未正式发布 `cer_normalization_rule_sha256` 字段，不能把派生值冒充已存在的生产合同字段。

## 正式实时录音注入合同审计

- 结果：`{audit['formal_realtime_input_contract']['status']}`。
- Q-00 正式执行器实际动作包含 `import-audio` 和 `moss-complete`。它证明的是冻结窗口导入加 MOSS 批处理，不是通过实时录音链路注入同一 PCM。
- 在生产和 QA 源码中没有找到 `audio_input_path_id`。因此无法证明修改前后到达实时转写引擎的是同一份声音，也没有启动三次冷热运行。

## 逐段证据和积压可取性

| 必需字段 | 当前判定 | 直接事实 |
| --- | --- | --- |
{chr(10).join(field_rows)}

逐段必需字段共 `{trace['required_field_count']}` 项，完整可取为 `{trace['fully_available_field_count']}` 项。当前只有内存态或汇总态碎片，不能达到逐段痕迹覆盖率 100%，也不能重算 `streaming_compute_rtf`、`max_chunk_lag_seconds`、`tail_drain_seconds` 和保存阶段耗时。

## 明确阻断项

{chr(10).join(blocker_rows)}

这些阻断项只说明 D-11 基线证据还不能成立，不代表 I-10A、V-10A 或任何系统音频场景通过。用户取消的有线耳机复测仍是 `CANCELLED_BY_USER`，不是 PASS；现有系统音频遗留缺口全部保留。

## 实际命令和日志

| 命令 | 退出码 | 日志 | 字节数 |
| --- | ---: | --- | ---: |
{chr(10).join(command_rows)}

- 日志总数：`{commands['log_count']}`。
- 所有命令退出码为 0：`{str(commands['all_exit_codes_zero']).lower()}`。
- `evidence-manifest.json` 绑定本检查点 10 个非自引用文件的字节数和 SHA-256，清单不把自己列入自身散列。
- 本检查点没有运行正式 Q-00、没有运行实时录音、没有运行 MOSS 模型、没有产生准确率 PASS。
"""


def main() -> int:
    EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
    LOG_ROOT.mkdir(exist_ok=True)
    commands = []
    commands.append(
        _run(
            1,
            "readiness-audit",
            [
                sys.executable,
                str(AUDIT_SCRIPT.relative_to(REPO)),
                "--repo",
                str(REPO),
                "--output",
                str(AUDIT_RESULT),
            ],
        )
    )
    commands.append(
        _run(
            2,
            "contract-tests",
            [sys.executable, str(TEST_SCRIPT.relative_to(REPO)), "-v"],
        )
    )

    audit = json.loads(AUDIT_RESULT.read_text(encoding="utf-8"))
    provisional = {
        "schema_version": 1,
        "stage": "D11_DIAGNOSTIC_COMMAND_RESULTS",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "commands": commands,
        "log_count": len(commands),
        "all_exit_codes_zero": all(row["exit_code"] == 0 for row in commands),
    }
    _write_json(COMMAND_RESULTS, provisional)
    _write_text(REPORT, _render_report(audit, provisional))

    commands.append(_run(3, "git-diff-check", ["git", "diff", "--check"]))
    changed_paths = _git_status_paths()
    production_prefixes = (
        "frontend/src-tauri/src/",
        "frontend/src/",
        "backend/",
        "moss-helper/src/",
    )
    production_changes = [
        path for path in changed_paths if path.startswith(production_prefixes)
    ]
    commands_document = {
        "schema_version": 1,
        "stage": "D11_DIAGNOSTIC_COMMAND_RESULTS",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "commands": commands,
        "log_count": len(commands),
        "all_exit_codes_zero": all(row["exit_code"] == 0 for row in commands),
        "workspace_changed_paths": changed_paths,
        "production_changed_paths": production_changes,
        "production_behavior_modified": bool(production_changes),
    }
    _write_json(COMMAND_RESULTS, commands_document)
    _write_text(REPORT, _render_report(audit, commands_document))
    manifest_inputs = [
        AUDIT_SCRIPT,
        TEST_SCRIPT,
        Path(__file__),
        REPO / AUDIT.PLAN_DIRECTORY / "TASKS.md",
        AUDIT_RESULT,
        COMMAND_RESULTS,
        REPORT,
        LOG_ROOT / "01-readiness-audit.log",
        LOG_ROOT / "02-contract-tests.log",
        LOG_ROOT / "03-git-diff-check.log",
    ]
    evidence_manifest = {
        "schema_version": 1,
        "stage": "D11_DIAGNOSTIC_EVIDENCE_MANIFEST",
        "generated_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "self_included": False,
        "file_count": len(manifest_inputs),
        "files": [_file_record(path) for path in sorted(manifest_inputs)],
    }
    _write_json(EVIDENCE_MANIFEST, evidence_manifest)

    summary = {
        "status": audit["status"],
        "d11_state": audit["d11_state"],
        "blocker_count": audit["blocker_count"],
        "log_count": commands_document["log_count"],
        "all_exit_codes_zero": commands_document["all_exit_codes_zero"],
        "production_behavior_modified": commands_document["production_behavior_modified"],
    }
    print(json.dumps(summary, ensure_ascii=False, sort_keys=True))
    return 0 if commands_document["all_exit_codes_zero"] and not production_changes else 1


if __name__ == "__main__":
    raise SystemExit(main())
