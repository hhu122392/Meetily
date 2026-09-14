from __future__ import annotations

import argparse
import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


MANIFEST_EXCLUDED_RELATIVE_PATHS = {
    "F5-final/MANIFEST.json",
    "F5-final/03-F5-manifest-audit.json",
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"JSON root must be an object: {path}")
    return value


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def write_text(path: Path, value: str) -> None:
    path.write_text(value.rstrip() + "\n", encoding="utf-8")


def record(path: Path, role: str, storage: str) -> dict[str, Any]:
    return {
        "role": role,
        "path": str(path.resolve()),
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
        "storage": storage,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Finalize the P0-R scoring closeout and freeze a truthful NO-GO/L2-BLOCKED decision."
    )
    parser.add_argument("--evidence-root", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--script", action="append", type=Path, default=[])
    return parser


def main() -> int:
    args = build_parser().parse_args()
    out_dir = args.evidence_root / "F5-final"
    out_dir.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).isoformat()
    paths = {
        "f0_audit": args.evidence_root
        / "F0-main"
        / "01-formal-whisper-preflight-audit.json",
        "f1_manifest": args.evidence_root / "F1-bundle" / "MANIFEST.json",
        "f1_lock": args.evidence_root / "F1-bundle" / "01-bundle-lock.json",
        "f1_audit": args.evidence_root / "F1-bundle" / "02-F1-audit.json",
        "f2_positive": args.evidence_root
        / "F2-terms"
        / "01-local-positive-terms-frozen.json",
        "f2_negative": args.evidence_root
        / "F2-terms"
        / "02-negative-term-review-blocker.json",
        "f2_audit": args.evidence_root / "F2-terms" / "03-F2-audit.json",
        "f3_score": args.evidence_root / "F3-score" / "01-local-p0r-score.json",
        "f3_audit": args.evidence_root / "F3-score" / "02-F3-audit.json",
        "f4_recalculation": args.evidence_root
        / "F4-audit"
        / "01-independent-score-recalculation.json",
        "f4_audit": args.evidence_root
        / "F4-audit"
        / "02-F4-independent-audit.json",
        "network_postcheck": args.evidence_root
        / "F4-audit"
        / "04-network-postcheck.json",
        "code_signature_postcheck": args.evidence_root
        / "F4-audit"
        / "05-code-signature-postcheck.json",
        "readme": args.evidence_root / "README.md",
        "tasks": args.evidence_root / "TASKS.md",
        "plan": args.plan,
    }
    missing = sorted(name for name, path in paths.items() if not path.is_file())
    missing_scripts = sorted(str(path) for path in args.script if not path.is_file())
    if missing or missing_scripts:
        write_json(
            out_dir / "03-F5-manifest-audit.json",
            {
                "schema_version": 1,
                "role": "P0R_F5_FINAL_AUDIT",
                "created_at": now,
                "status": "FAIL",
                "missing_named_inputs": missing,
                "missing_scripts": missing_scripts,
            },
        )
        return 2

    f0 = load_json(paths["f0_audit"])
    f1_lock = load_json(paths["f1_lock"])
    f1_audit = load_json(paths["f1_audit"])
    f2_positive = load_json(paths["f2_positive"])
    f2_negative = load_json(paths["f2_negative"])
    f2_audit = load_json(paths["f2_audit"])
    f3_score = load_json(paths["f3_score"])
    f3_audit = load_json(paths["f3_audit"])
    f4_recalc = load_json(paths["f4_recalculation"])
    f4_audit = load_json(paths["f4_audit"])
    network = load_json(paths["network_postcheck"])
    signature = load_json(paths["code_signature_postcheck"])

    hard_failures = sorted(
        str(item) for item in f4_recalc.get("recalculated_failure_set", [])
    )
    moss_cer = float(
        f4_recalc["recalculated_metrics"]["moss_cer"]["cer"]
    )
    whisper_cer = float(
        f4_recalc["recalculated_metrics"]["whisper_cer"]["cer"]
    )
    speaker = f4_recalc["recalculated_metrics"]["moss_speaker"]
    terms = f4_recalc["recalculated_metrics"]["moss_positive_terms"]

    prerequisite_checks: dict[str, bool] = {
        "f0_preflight_pass": f0.get("status") == "PASS",
        "f1_freeze_pass": f1_audit.get("status") == "PASS",
        "f1_local_lane_was_allowed": f1_lock.get(
            "local_p0r_scoring_lane", {}
        ).get("allowed_after_f2_terms")
        is True,
        "f2_positive_frozen": f2_positive.get("status")
        == "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN",
        "f2_structural_pass": f2_audit.get("structural_status") == "PASS",
        "f2_negative_blocker_preserved": f2_negative.get("status")
        == "BLOCKED_HUMAN_FULL_AUDIO_NEGATIVE_ATTESTATION_MISSING",
        "f3_execution_audit_pass": f3_audit.get("status") == "PASS",
        "f3_score_is_truthful_no_go": f3_score.get("status")
        == "LOCAL_P0R_QUANTITATIVE_FAIL_NO_GO"
        and f3_score.get("release_go") is False,
        "f4_independent_audit_pass": f4_audit.get("status") == "PASS",
        "f4_confirms_hard_failures": len(hard_failures) == 7,
        "network_postcheck_pass": network.get("status") == "PASS"
        and network.get("observations", {}).get(
            "meetily_r3_firewall_rule_count"
        )
        == 0,
        "network_postcheck_was_read_only": network.get("mutation_performed")
        is False,
        "release_candidate_not_signed": signature.get("status")
        == "BLOCKED_NOT_SIGNED"
        and signature.get("executable", {}).get("authenticode_status")
        == "NotSigned",
    }
    prerequisite_failures = [
        name for name, passed in prerequisite_checks.items() if not passed
    ]
    if prerequisite_failures:
        write_json(
            out_dir / "03-F5-manifest-audit.json",
            {
                "schema_version": 1,
                "role": "P0R_F5_FINAL_AUDIT",
                "created_at": now,
                "status": "FAIL",
                "prerequisite_checks": prerequisite_checks,
                "failed_checks": prerequisite_failures,
            },
        )
        return 2

    decision = {
        "schema_version": 1,
        "role": "P0R_F5_FINAL_RELEASE_DECISION",
        "created_at": now,
        "plan_execution_status": "F0_THROUGH_F5_COMPLETED",
        "scoring_closeout_status": "COMPLETED_WITH_REAL_FAILURES_AND_BLOCKERS",
        "p0r_accuracy_status": "FAIL_NO_GO",
        "l2_release_status": "BLOCKED_NO_GO",
        "release_go": False,
        "component_results": {
            "moss_text_transcription_cer": {
                "status": "PASS",
                "actual": moss_cer,
                "threshold_maximum": 0.20,
            },
            "formal_whisper_text_baseline_cer": {
                "status": "BASELINE",
                "actual": whisper_cer,
            },
            "moss_speaker_turn_error_rate": {
                "status": "FAIL",
                "actual": speaker["error_rate"],
                "threshold_maximum": 0.10,
            },
            "moss_speaker_duration_error_rate": {
                "status": "FAIL",
                "actual": speaker["duration_error_rate"],
                "threshold_maximum": 0.10,
            },
            "moss_positive_term_alignment": {
                "status": "FAIL",
                "exact_targets": terms["exact_occurrence_target_count"],
                "target_count": terms["target_count"],
            },
            "negative_term_human_gate": {
                "status": "BLOCKED",
                "reason": "缺少完整 737.728 秒录音的专门人工未说出声明",
            },
            "strict_s8_cuda_lane": {
                "status": "BLOCKED",
                "reason": "缺少 LOCKED_FULL_MOSS_OUTPUT schema_version=2 和受监督 CUDA 正式运行",
            },
            "windows_code_signing": {
                "status": "BLOCKED",
                "authenticode": "NotSigned",
            },
            "network": {
                "status": "PASS",
                "meetily_r3_firewall_rule_count": 0,
                "mutation_performed": False,
            },
        },
        "quantitative_hard_gate_failure_count": len(hard_failures),
        "quantitative_hard_gate_failures": hard_failures,
        "what_is_usable_now": [
            "当前电脑上该次 MOSS 输出的纯文字转写准确率通过，CER 为 2.754%。",
            "该结论只适用于已哈希绑定的模型、程序、录音和本次分块输出。",
        ],
        "what_is_not_accepted": [
            "不能把当前结果当成可靠的多人说话人区分。",
            "不能把当前结果当成严格术语时间对齐通过。",
            "不能宣布 P0-R 总准确率通过。",
            "不能宣布 L2 正式发布通过。",
        ],
        "required_next_development": [
            "实现跨分块说话人身份拼接并重新评分。",
            "修复大于 60 秒的片段、窗口边界溢出和尾部覆盖。",
            "修复正向术语的时间对齐，使 3/3 目标及全部出现次数精确通过。",
            "生成新的候选输出重新运行同一冻结门槛，不修改本轮分数。",
            "准确率通过后，使用公司正式 Windows 证书签名并复验发布件。",
        ],
        "input_bindings": {
            name: {"path": str(path.resolve()), "sha256": sha256_file(path)}
            for name, path in paths.items()
        }
        | {
            "f5_script": {
                "path": str(Path(__file__).resolve()),
                "sha256": sha256_file(Path(__file__).resolve()),
            }
        },
        "truth_boundary": (
            "F5 只汇总 F0–F4 的哈希绑定结果。"
            "文字 CER 通过不能覆盖说话人、术语、结构、负向人工声明和代码签名的失败或阻断。"
        ),
    }
    decision_path = out_dir / "01-final-release-decision.json"
    write_json(decision_path, decision)
    decision_hash = sha256_file(decision_path)

    report = f"""# Meetily MOSS P0-R 与 L2 最终验收结论

## 最终结果

- 本轮执行计划：`F0–F5 已执行完成`
- P0-R 总准确率：`FAIL / NO-GO`
- L2 正式发布：`BLOCKED / NO-GO`
- 发布许可：`false`

这不是说 MOSS 的文字转写完全不可用。实测 MOSS CER 为 `{moss_cer:.6f}`（`{moss_cer * 100:.3f}%`），通过 `<=20%` 的文字门槛；同窗口 Whisper CER 为 `{whisper_cer:.6f}`（`{whisper_cer * 100:.3f}%`）。但产品要求还包括说话人、术语、时间和完整性，这些项目没有全部通过。

## 真实失败

| 项目 | 实测 | 门槛 | 状态 |
|---|---:|---:|---|
| MOSS 文字 CER | `{moss_cer:.6f}` | `<=0.20` | `PASS` |
| 说话人轮次错误率 | `{float(speaker['error_rate']):.6f}` | `<=0.10` | `FAIL` |
| 说话人时长错误率 | `{float(speaker['duration_error_rate']):.6f}` | `<=0.10` | `FAIL` |
| 正向术语精确目标 | `{terms['exact_occurrence_target_count']}/{terms['target_count']}` | `3/3` | `FAIL` |
| 数值硬门禁失败 | `{len(hard_failures)}` 项 | `0` 项 | `FAIL` |

7 个硬门禁失败项：

1. `moss_window_boundary_spill`
2. `moss_speaker_turn_error_rate`
3. `moss_speaker_duration_error_rate`
4. `moss_cross_chunk_speaker_identity`
5. `moss_positive_terms_exact`
6. `moss_full_last_segment_end`
7. `moss_full_maximum_segment_duration`

## 阻断项

- 负向术语专门人工声明缺失：`BLOCKED`。
- 严格 S8 CUDA 正式输出缺失：`BLOCKED`。
- Windows 候选发布件 Authenticode 为 `NotSigned`：`BLOCKED`。

## 可以怎么用

当前只能把这次 MOSS 结果作为“纯文字转写效果已经达到门槛”的开发验证。多人会议里的说话人姓名归属、跨分块同一人识别和严格术语时间对齐仍不可靠，不能作为正式发布验收通过。

## 审计可信度

- F3 评分器正常结束。
- F4 没有导入 F3 脚本，从冻结输入重新计算，16/16 检查和 13/13 指标一致。
- 网络只读复核通过，测试防火墙规则为 0 条；本轮没有修改防火墙。
- 最终判定文件 SHA-256：`{decision_hash}`
"""
    report_path = out_dir / "02-最终验收结论.md"
    write_text(report_path, report)

    manifest_entries: list[dict[str, Any]] = []
    for path in sorted(args.evidence_root.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(args.evidence_root).as_posix()
        if relative in MANIFEST_EXCLUDED_RELATIVE_PATHS:
            continue
        manifest_entries.append(
            record(path, f"evidence:{relative}", "IN_BUNDLE")
        )
    manifest_entries.append(record(args.plan, "external:closeout_plan", "EXTERNAL"))
    for index, path in enumerate(sorted(args.script, key=lambda item: str(item))):
        manifest_entries.append(
            record(path, f"external:closeout_script:{index + 1}", "EXTERNAL")
        )
    manifest_entries.sort(key=lambda item: item["role"])
    manifest = {
        "schema_version": 1,
        "role": "P0R_F5_FINAL_EVIDENCE_MANIFEST",
        "created_at": now,
        "status": "FROZEN",
        "decision": "P0R_FAIL_NO_GO_L2_BLOCKED_NO_GO",
        "entry_count": len(manifest_entries),
        "entries": manifest_entries,
        "excluded_self_referential_files": sorted(MANIFEST_EXCLUDED_RELATIVE_PATHS),
        "transcript_text_included": False,
    }
    manifest_path = out_dir / "MANIFEST.json"
    write_json(manifest_path, manifest)
    manifest_hash = sha256_file(manifest_path)

    rehash_mismatches = []
    duplicate_roles = []
    seen_roles: set[str] = set()
    for item in manifest_entries:
        role = str(item["role"])
        if role in seen_roles:
            duplicate_roles.append(role)
        seen_roles.add(role)
        path = Path(str(item["path"]))
        if (
            not path.is_file()
            or path.stat().st_size != int(item["bytes"])
            or sha256_file(path) != item["sha256"]
        ):
            rehash_mismatches.append(role)

    final_checks: dict[str, bool] = {
        "all_prerequisites_pass": all(prerequisite_checks.values()),
        "decision_file_exists": decision_path.is_file(),
        "decision_hash_is_bound": decision_hash == sha256_file(decision_path),
        "report_file_exists": report_path.is_file(),
        "manifest_entries_nonempty": len(manifest_entries) > 0,
        "manifest_roles_unique": not duplicate_roles,
        "manifest_rehash_mismatch_count_zero": not rehash_mismatches,
        "manifest_declares_no_transcript_text": manifest[
            "transcript_text_included"
        ]
        is False,
        "p0r_not_falsely_passed": decision["p0r_accuracy_status"]
        == "FAIL_NO_GO",
        "l2_not_falsely_passed": decision["l2_release_status"]
        == "BLOCKED_NO_GO",
        "release_go_false": decision["release_go"] is False,
        "hard_failure_count_preserved": decision[
            "quantitative_hard_gate_failure_count"
        ]
        == 7,
        "network_mutation_remains_false": decision["component_results"]["network"][
            "mutation_performed"
        ]
        is False,
    }
    final_failed = [name for name, passed in final_checks.items() if not passed]
    audit = {
        "schema_version": 1,
        "role": "P0R_F5_FINAL_AUDIT",
        "created_at": now,
        "status": "PASS" if not final_failed else "FAIL",
        "checks": final_checks,
        "summary": {
            "check_count": len(final_checks),
            "pass_count": sum(final_checks.values()),
            "failed_count": len(final_failed),
            "failed_checks": final_failed,
            "manifest_duplicate_roles": duplicate_roles,
            "manifest_rehash_mismatches": rehash_mismatches,
        },
        "decision_path": str(decision_path.resolve()),
        "decision_sha256": decision_hash,
        "manifest_path": str(manifest_path.resolve()),
        "manifest_sha256": manifest_hash,
        "manifest_entry_count": len(manifest_entries),
        "final_plan_execution_status": "COMPLETE",
        "final_product_status": "P0R_FAIL_NO_GO_L2_BLOCKED_NO_GO",
    }
    audit_path = out_dir / "03-F5-manifest-audit.json"
    write_json(audit_path, audit)
    audit_hash = sha256_file(audit_path)

    print(
        json.dumps(
            {
                "audit_status": audit["status"],
                "plan_execution_status": "COMPLETE",
                "p0r_accuracy_status": decision["p0r_accuracy_status"],
                "l2_release_status": decision["l2_release_status"],
                "hard_gate_failures": hard_failures,
                "manifest_entries": len(manifest_entries),
                "decision_sha256": decision_hash,
                "manifest_sha256": manifest_hash,
                "audit_sha256": audit_hash,
            },
            ensure_ascii=False,
        )
    )
    return 0 if not final_failed else 2


if __name__ == "__main__":
    sys.exit(main())
