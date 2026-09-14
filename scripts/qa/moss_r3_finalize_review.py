#!/usr/bin/env python3
"""Create transcript-free public R3 score summaries and the final decision."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


EXPECTED_TIMESTAMP_DECISION = (
    "GENERIC_WORD_API_PRESENT_BUT_PINNED_MOSS_WORD_TIMESTAMPS_UNSUPPORTED"
)
EXPECTED_BOUND_MOSS_SHA256 = (
    "16DCCB006FFBA82CF1855B5C055E4D76282EDA8D3F1F3250271FACD6EBD4E5C6"
)
FORBIDDEN_PUBLIC_KEYS = {"text", "transcript", "global_turns", "segments"}
WINDOW_SECONDS = {"start": 70.370, "end": 296.810, "duration": 226.440}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def load_json(path: Path) -> dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(payload, dict):
        raise ValueError(f"json_root_must_be_object:{path}")
    return payload


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def write_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value.rstrip() + "\n", encoding="utf-8", newline="\n")


def write_sidecar(path: Path) -> str:
    digest = sha256_file(path)
    path.with_suffix(path.suffix + ".sha256").write_text(
        f"{digest}  {path.name}\n", encoding="ascii", newline="\n"
    )
    return digest


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def forbidden_key_paths(value: Any, prefix: str = "$") -> list[str]:
    findings: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            child_path = f"{prefix}.{key}"
            if str(key).casefold() in FORBIDDEN_PUBLIC_KEYS:
                findings.append(child_path)
            findings.extend(forbidden_key_paths(child, child_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            findings.extend(forbidden_key_paths(child, f"{prefix}[{index}]"))
    return findings


def public_payload_checks(payload: dict[str, Any]) -> dict[str, bool]:
    rendered = json.dumps(payload, ensure_ascii=False)
    return {
        "no_forbidden_transcript_keys": not forbidden_key_paths(payload),
        "no_windows_absolute_paths": re.search(r"(?i)(?:[a-z]:\\|[a-z]:/)", rendered)
        is None,
        "no_unc_paths": "\\\\" not in rendered,
        "declares_no_transcript": payload.get("transcript_text_included") is False,
        "declares_no_absolute_paths": payload.get("absolute_paths_included") is False,
    }


def gate_metric(
    details: dict[str, Any],
    name: str,
    *,
    actual_key: str = "actual",
    threshold_key: str | None = None,
) -> dict[str, Any]:
    gate = details[name]
    payload: dict[str, Any] = {
        "actual": gate.get(actual_key),
        "pass": gate.get("pass") is True,
    }
    if threshold_key is not None:
        payload["threshold"] = gate.get(threshold_key)
    return payload


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--timestamp-audit", type=Path, required=True)
    parser.add_argument("--binding", type=Path, required=True)
    parser.add_argument("--f3-score", type=Path, required=True)
    parser.add_argument("--f3-audit", type=Path, required=True)
    parser.add_argument("--f4-score", type=Path, required=True)
    parser.add_argument("--f4-audit", type=Path, required=True)
    parser.add_argument("--old-p1-score", type=Path, required=True)
    parser.add_argument("--r2-term-audit", type=Path, required=True)
    parser.add_argument("--r2-tail-audit", type=Path, required=True)
    parser.add_argument("--r2-signing", type=Path, required=True)
    parser.add_argument("--r2-final", type=Path, required=True)
    parser.add_argument("--out-root", type=Path, required=True)
    args = parser.parse_args()

    input_paths = {
        "timestamp_audit": args.timestamp_audit,
        "binding": args.binding,
        "f3_score": args.f3_score,
        "f3_audit": args.f3_audit,
        "f4_score": args.f4_score,
        "f4_audit": args.f4_audit,
        "old_p1_score": args.old_p1_score,
        "r2_term_audit": args.r2_term_audit,
        "r2_tail_audit": args.r2_tail_audit,
        "r2_signing": args.r2_signing,
        "r2_final": args.r2_final,
    }
    missing = [name for name, path in input_paths.items() if not path.is_file()]
    require(not missing, f"missing_inputs:{','.join(missing)}")
    input_hashes = {name: sha256_file(path) for name, path in input_paths.items()}

    timestamp = load_json(args.timestamp_audit)
    binding = load_json(args.binding)
    f3 = load_json(args.f3_score)
    f3_audit = load_json(args.f3_audit)
    f4 = load_json(args.f4_score)
    f4_audit = load_json(args.f4_audit)
    old = load_json(args.old_p1_score)
    term_audit = load_json(args.r2_term_audit)
    tail_audit = load_json(args.r2_tail_audit)
    signing = load_json(args.r2_signing)
    r2_final_text = args.r2_final.read_text(encoding="utf-8-sig")

    require(timestamp.get("status") == "PASS", "timestamp_audit_not_pass")
    require(
        timestamp.get("decision") == EXPECTED_TIMESTAMP_DECISION,
        "timestamp_decision_mismatch",
    )
    require(
        timestamp.get("pinned_moss_source_audit", {}).get("word_request_supported")
        is False,
        "timestamp_word_support_must_remain_false",
    )
    require(
        binding.get("status")
        in {"PASS", "PASS_WITH_RECORDED_NON_SCORING_DOCUMENT_DRIFT"},
        "binding_not_pass",
    )
    require(
        binding.get("source_manifest_rehash", {}).get(
            "all_f3_f4_scoring_inputs_match"
        )
        is True,
        "binding_scoring_inputs_not_all_matched",
    )
    require(
        binding.get("change_audit", {}).get("bound_moss_sha256")
        == EXPECTED_BOUND_MOSS_SHA256,
        "bound_moss_hash_mismatch",
    )
    require(
        binding.get("adapter_validation", {}).get("stage")
        == "R1_MONOLITHIC_VULKAN16K",
        "bound_stage_mismatch",
    )
    require(f3_audit.get("status") == "PASS", "f3_audit_not_pass")
    require(
        f3_audit.get("score_report_sha256", "").upper()
        == input_hashes["f3_score"],
        "f3_score_hash_not_bound_to_audit",
    )
    require(
        f3.get("status") == "LOCAL_P0R_QUANTITATIVE_FAIL_NO_GO",
        "f3_score_status_unexpected",
    )
    require(f3.get("release_go") is False, "f3_release_go_not_false")
    require(f4_audit.get("status") == "PASS", "f4_audit_not_pass")
    require(
        f4_audit.get("independent_recalculation_sha256", "").upper()
        == input_hashes["f4_score"],
        "f4_score_hash_not_bound_to_audit",
    )
    require(
        all(f4.get("metric_comparisons", {}).values()),
        "f4_metric_comparison_mismatch",
    )
    require(
        sorted(f3["gates"]["hard_gate_failures"])
        == sorted(f4["recalculated_failure_set"]),
        "f3_f4_failure_set_mismatch",
    )
    require(
        term_audit.get("source_private_artifact_sha256")
        == EXPECTED_BOUND_MOSS_SHA256,
        "term_audit_source_mismatch",
    )
    require(tail_audit.get("status") == "PASS", "tail_audit_not_pass")
    require(signing.get("status") == "BLOCKED_EXTERNAL", "signing_not_blocked")
    require(signing.get("formal_release_allowed") is False, "signing_release_allowed")
    require(
        signing.get("files")
        and all(item.get("authenticode_status") == "NotSigned" for item in signing["files"]),
        "unexpected_signing_status",
    )
    require(
        "本机内部使用：`GO_WITH_MANDATORY_HUMAN_REVIEW`" in r2_final_text,
        "r2_internal_use_decision_missing",
    )
    require(
        "原始模型准确率验收：`NO-GO`" in r2_final_text,
        "r2_accuracy_decision_missing",
    )
    require("L2 正式发布：`BLOCKED`" in r2_final_text, "r2_l2_decision_missing")

    details = f3["gates"]["hard_gate_details"]
    hard_failures = list(f3["gates"]["hard_gate_failures"])
    now = datetime.now(timezone.utc).isoformat()
    common = {
        "created_at": now,
        "transcript_text_included": False,
        "absolute_paths_included": False,
    }

    c_summary = {
        "schema_version": 1,
        "stage": "MOSS_R3_F3_SINGLE_SESSION_PUBLIC_SCORE_SUMMARY",
        "status": "EXECUTION_PASS_SCORE_FAIL",
        "release_go": False,
        "input_hashes": {
            "timestamp_audit_sha256": input_hashes["timestamp_audit"],
            "binding_sha256": input_hashes["binding"],
            "derived_manifest_sha256": binding["derived_manifest_sha256"],
            "bound_moss_sha256": EXPECTED_BOUND_MOSS_SHA256,
            "f3_score_sha256": input_hashes["f3_score"],
            "f3_audit_sha256": input_hashes["f3_audit"],
            "f3_script_sha256": f3["input_bindings"]["local_scorer_sha256"].upper(),
            "frozen_strict_scorer_sha256": f3["input_bindings"][
                "frozen_strict_scorer_sha256"
            ].upper(),
        },
        "source": {
            "authoritative_stage": f3["moss"]["source_stage"],
            "run_state": f3["moss"]["source_run_state"],
            "selected_chunk_indices": f3["moss"]["selected_chunk_indices"],
            "turn_count": binding["adapter_validation"]["turn_count"],
            "speaker_label_count": binding["adapter_validation"][
                "speaker_label_count"
            ],
            "rtf": f3["moss"]["full_runtime"]["total_rtf"],
            "scope_label_correction": (
                "The frozen F3 report retains a historical P1_CHUNKED scope label. "
                "The payload stage and selected chunk index prove this run is the R1 "
                "single-session Vulkan 16K output."
            ),
        },
        "window_seconds": WINDOW_SECONDS,
        "metrics": {
            "cer": gate_metric(details, "moss_cer", threshold_key="threshold_maximum"),
            "speaker_turn_error_rate": gate_metric(
                details,
                "moss_speaker_turn_error_rate",
                threshold_key="threshold_maximum",
            ),
            "speaker_duration_error_rate": gate_metric(
                details,
                "moss_speaker_duration_error_rate",
                threshold_key="threshold_maximum",
            ),
            "speaker_false_alarm_seconds": gate_metric(
                details,
                "moss_speaker_false_alarm_seconds",
                threshold_key="threshold_maximum",
            ),
            "speaker_false_alarm_rate": gate_metric(
                details,
                "moss_speaker_false_alarm_rate",
                threshold_key="threshold_maximum",
            ),
            "boundary_spill_seconds": gate_metric(
                details,
                "moss_window_boundary_spill",
                threshold_key="threshold_maximum",
            ),
            "positive_terms_exact": {
                "actual": details["moss_positive_terms_exact"]["actual_exact_targets"],
                "target": details["moss_positive_terms_exact"]["target_count"],
                "pass": details["moss_positive_terms_exact"]["pass"] is True,
            },
            "full_last_segment_end_seconds": gate_metric(
                details,
                "moss_full_last_segment_end",
                threshold_key="threshold_minimum",
            ),
            "full_maximum_segment_duration_seconds": gate_metric(
                details,
                "moss_full_maximum_segment_duration",
                threshold_key="threshold_maximum",
            ),
            "full_rtf": gate_metric(
                details, "moss_full_rtf", threshold_key="threshold_maximum"
            ),
        },
        "hard_gate_count": f3["gates"]["hard_gate_count"],
        "hard_gate_failure_count": len(hard_failures),
        "hard_gate_failures": hard_failures,
        "negative_term_gate": f3["gates"]["negative_term_gate"]["status"],
        **common,
    }

    d_summary = {
        "schema_version": 1,
        "stage": "MOSS_R3_F4_INDEPENDENT_PUBLIC_AUDIT_SUMMARY",
        "status": "PASS_CONFIRMING_SCORE_FAIL",
        "release_go": False,
        "method": (
            "F4 independently recalculated the frozen inputs without importing the F3 "
            "script or using the F3 written conclusion as the answer."
        ),
        "input_hashes": {
            "f3_score_sha256": input_hashes["f3_score"],
            "f3_audit_sha256": input_hashes["f3_audit"],
            "f4_score_sha256": input_hashes["f4_score"],
            "f4_audit_sha256": input_hashes["f4_audit"],
            "f4_script_sha256": f4["input_hashes"]["f4_script_sha256"].upper(),
        },
        "audit_checks": {
            "pass_count": f4_audit["summary"]["pass_count"],
            "check_count": f4_audit["summary"]["check_count"],
            "failed_count": f4_audit["summary"]["failed_count"],
        },
        "metric_comparisons": {
            "match_count": sum(f4["metric_comparisons"].values()),
            "comparison_count": len(f4["metric_comparisons"]),
            "mismatches": [
                name for name, passed in f4["metric_comparisons"].items() if not passed
            ],
        },
        "confirmed_f3_status": f4_audit["confirmed_score_status"],
        "confirmed_hard_gate_failures": f4_audit["confirmed_hard_gate_failures"],
        "decision": f4["decision"],
        **common,
    }

    old_failures = list(old["gates"]["hard_gate_failures"])
    comparison = {
        "old_source_stage": old["moss"]["source_stage"],
        "new_source_stage": f3["moss"]["source_stage"],
        "old_hard_failure_count": len(old_failures),
        "new_hard_failure_count": len(hard_failures),
        "resolved_hard_failures": sorted(set(old_failures) - set(hard_failures)),
        "remaining_hard_failures": sorted(hard_failures),
        "metrics": {
            "cer": {"old": old["moss"]["cer"]["cer"], "new": f3["moss"]["cer"]["cer"]},
            "speaker_turn_error_rate": {
                "old": old["moss"]["speaker"]["error_rate"],
                "new": f3["moss"]["speaker"]["error_rate"],
            },
            "speaker_duration_error_rate": {
                "old": old["moss"]["speaker"]["duration_error_rate"],
                "new": f3["moss"]["speaker"]["duration_error_rate"],
            },
            "boundary_spill_seconds": {
                "old": old["moss"]["boundary_spill"]["maximum_spill_seconds"],
                "new": f3["moss"]["boundary_spill"]["maximum_spill_seconds"],
            },
            "positive_terms_exact": {
                "old": old["moss"]["positive_terms"]["exact_occurrence_target_count"],
                "new": f3["moss"]["positive_terms"]["exact_occurrence_target_count"],
                "target": f3["moss"]["positive_terms"]["target_count"],
            },
            "last_segment_end_seconds": {
                "old": old["moss"]["full_runtime"]["last_segment_end_seconds"],
                "new": f3["moss"]["full_runtime"]["last_segment_end_seconds"],
            },
            "maximum_segment_duration_seconds": {
                "old": old["moss"]["full_runtime"]["maximum_segment_duration_seconds"],
                "new": f3["moss"]["full_runtime"]["maximum_segment_duration_seconds"],
            },
        },
    }

    final_decision = {
        "schema_version": 1,
        "stage": "MOSS_R3_FINAL_DECISION",
        "status": "R3_EXECUTION_COMPLETE_PRODUCT_GATES_NOT_ALL_PASS",
        "release_go": False,
        "decisions": {
            "local_internal_use": "GO_WITH_MANDATORY_HUMAN_REVIEW",
            "raw_model_accuracy": "NO_GO",
            "r3_quantitative_gate": "FAIL",
            "negative_term_human_gate": "BLOCKED_EXTERNAL",
            "word_timestamp_capability": "UNSUPPORTED_SEGMENT_ONLY",
            "windows_signing": "BLOCKED_EXTERNAL",
            "l2_formal_release": "BLOCKED",
        },
        "completed_tasks": [
            "R3-A_TIMESTAMP_CAPABILITY_CORRECTED",
            "R3-B_SINGLE_SESSION_SCORE_INPUT_BOUND",
            "R3-C_F3_MECHANICAL_SCORE_COMPLETE",
            "R3-D_F4_INDEPENDENT_RECALCULATION_COMPLETE",
            "R3-E_FINAL_DECISION_AND_SELF_REVIEW_COMPLETE",
        ],
        "quantitative_hard_failures": hard_failures,
        "comparison_to_original_chunked_output": comparison,
        "important_distinctions": {
            "tail": {
                "frozen_completeness_gate": "FAIL",
                "model_last_timestamp_ms": tail_audit["model_last_timestamp_ms"],
                "last_active_audio_end_ms": tail_audit["selected_last_active_end_ms"],
                "acoustic_tail_error_ms": tail_audit["absolute_tail_error_ms"],
                "acoustic_tail_audit": tail_audit["status"],
                "meaning": (
                    "The frozen >=730.4s scoring threshold remains failed. A separate PCM "
                    "audit found the model endpoint within 50ms of the last active audio, so "
                    "there is no measured evidence of lost tail speech."
                ),
            },
            "terms": {
                "strict_alignment_gate": "FAIL_0_OF_3",
                "reviewed_correction_status": term_audit["status"],
                "reviewed_correction_is_raw_model_hit": False,
                "meaning": (
                    "Human-reviewed corrections remain traceable and reversible, but cannot "
                    "be counted as raw-model exact hits."
                ),
            },
            "timestamps": {
                "finest_real_kind": timestamp["pinned_moss_source_audit"][
                    "finest_real_timestamp_kind"
                ],
                "synthetic_word_timestamps_allowed": timestamp[
                    "synthetic_word_timestamps_allowed"
                ],
            },
        },
        "external_blockers": {
            "negative_term_attestation": (
                "The full 737.728-second human statements for Karl and A/B Test are still missing."
            ),
            "signing": {
                "status": signing["status"],
                "not_signed_file_count": sum(
                    item["authenticode_status"] == "NotSigned"
                    for item in signing["files"]
                ),
                "company_signing_key_available": signing[
                    "company_signing_key_available"
                ],
                "tauri_updater_private_key_available": signing[
                    "tauri_updater_private_key_available"
                ],
            },
        },
        "input_hashes": input_hashes,
        **common,
    }

    c_path = args.out_root / "R3-C" / "01-single-session-score-summary.json"
    d_path = args.out_root / "R3-D" / "01-independent-recalculation-summary.json"
    e_path = args.out_root / "R3-E" / "01-R3-final-decision.json"
    for path, payload in ((c_path, c_summary), (d_path, d_summary), (e_path, final_decision)):
        checks = public_payload_checks(payload)
        require(all(checks.values()), f"public_payload_check_failed:{path.name}:{checks}")
        write_json(path, payload)
        write_sidecar(path)

    decision_markdown = f"""# MOSS R3 最终结论

- R3 执行任务：`全部完成`
- 本机内部使用：`GO_WITH_MANDATORY_HUMAN_REVIEW`
- 原始模型准确率：`NO-GO`
- L2 正式发布：`BLOCKED`

## 本轮确认的结果

- F3 评分程序：`PASS`；模型数值门禁：`FAIL`。
- F4 独立复算：`{f4_audit['summary']['pass_count']}/{f4_audit['summary']['check_count']}` 检查通过，`{sum(f4['metric_comparisons'].values())}/{len(f4['metric_comparisons'])}` 指标一致。
- CER：`{f3['moss']['cer']['cer']:.6f}`，门槛 `<= {details['moss_cer']['threshold_maximum']}`，通过。
- 说话人轮次错误率：`{f3['moss']['speaker']['error_rate']:.6f}`，通过。
- 说话人时长错误率：`{f3['moss']['speaker']['duration_error_rate']:.6f}`，通过。
- 数值硬失败：`{len(hard_failures)}` 项：`{', '.join(hard_failures)}`。

## 不能写成已通过的部分

1. 固定评分窗边界溢出 `29.37 秒`，门槛 `<=2 秒`。
2. 正向术语严格对齐为 `0/3`；人工确认修订不能算原始模型命中。
3. 最后分段结束于 `729.11 秒`，固定门槛要求 `>=730.4 秒`。另一个 PCM 审计显示它距离最后有效声音仅 `50 ms`，所以没有测到漏掉尾部讲话，但固定门槛仍然失败。
4. 当前固定 MOSS 只能提供 segment 时间，不能提供真实 word 时间。
5. `Karl`、`A/B Test` 的完整录音人工负向声明仍缺失。
6. 应用、helper、安装包共 3 个文件均为 `NotSigned`，正式签名密钥也未提供。

## 与原分块输出相比

- 硬失败从 `{len(old_failures)}` 项降到 `{len(hard_failures)}` 项。
- 已解决：说话人轮次、说话人时长、跨分块身份、最大片段时长。
- 未解决：边界溢出、术语严格命中、固定尾部完整性门槛。

结论不是“功能全部通过”。当前安装包可继续用于本机内部测试，但每份转写和摘要仍必须人工复核；准确率总门禁和 L2 正式发布都没有通过。
"""
    md_path = args.out_root / "R3-E" / "02-R3-final-decision.md"
    require(re.search(r"(?i)(?:[a-z]:\\|[a-z]:/)", decision_markdown) is None, "markdown_absolute_path")
    write_text(md_path, decision_markdown)
    write_sidecar(md_path)

    output_paths = [c_path, d_path, e_path, md_path]
    output_hashes = {path.name: sha256_file(path) for path in output_paths}
    self_checks = {
        "timestamp_evidence_pass": timestamp["status"] == "PASS",
        "binding_scoring_inputs_match": binding["source_manifest_rehash"][
            "all_f3_f4_scoring_inputs_match"
        ]
        is True,
        "binding_only_one_role_changed": binding["change_audit"][
            "changed_role_count"
        ]
        == 1,
        "f3_execution_audit_pass": f3_audit["status"] == "PASS",
        "f3_failure_not_hidden": bool(hard_failures)
        and c_summary["status"] == "EXECUTION_PASS_SCORE_FAIL",
        "f4_independent_audit_pass": f4_audit["status"] == "PASS",
        "f4_all_metrics_match": all(f4["metric_comparisons"].values()),
        "negative_gate_remains_blocked": f3["gates"]["negative_term_gate"][
            "status"
        ]
        == "BLOCKED",
        "word_timestamp_not_fabricated": final_decision["decisions"][
            "word_timestamp_capability"
        ]
        == "UNSUPPORTED_SEGMENT_ONLY",
        "signing_remains_blocked": final_decision["decisions"]["windows_signing"]
        == "BLOCKED_EXTERNAL",
        "release_go_false": final_decision["release_go"] is False,
        "all_public_json_outputs_safe": all(
            all(public_payload_checks(payload).values())
            for payload in (c_summary, d_summary, final_decision)
        ),
        "all_public_outputs_have_sha256_sidecars": all(
            path.with_suffix(path.suffix + ".sha256").is_file()
            for path in output_paths
        ),
    }
    failed_self_checks = [name for name, passed in self_checks.items() if not passed]
    self_review = {
        "schema_version": 1,
        "stage": "MOSS_R3_FINAL_SELF_REVIEW",
        "status": "PASS" if not failed_self_checks else "FAIL",
        "checks": self_checks,
        "summary": {
            "check_count": len(self_checks),
            "pass_count": sum(self_checks.values()),
            "failed_count": len(failed_self_checks),
            "failed_checks": failed_self_checks,
        },
        "output_hashes": output_hashes,
        "recorded_execution_corrections": [
            {
                "attempt": "R3-B-1",
                "result": "STOPPED_BEFORE_BINDING",
                "reason": (
                    "A later-edited closeout plan no longer matched its old F1 hash. The "
                    "scorer source was checked before narrowly classifying it as non-scoring."
                ),
            },
            {
                "attempt": "R3-B-2",
                "result": "STOPPED_BEFORE_BINDING",
                "reason": (
                    "The new validator initially checked start_ms/end_ms while the frozen "
                    "adapter schema uses global_start_ms/global_end_ms. The validator and "
                    "tests were corrected before the evidence binding was produced."
                ),
            },
            {
                "attempt": "R3-C-1",
                "result": "STOPPED_BEFORE_SCORING_OUTPUT",
                "reason": (
                    "The first CLI used the bundle lock instead of the frozen human-window "
                    "source lock. F3 raised KeyError before creating a score, then was rerun "
                    "with the source lock already bound in the prior accepted R1 score."
                ),
            },
        ],
        "transcript_text_included": False,
        "absolute_paths_included": False,
    }
    require(not failed_self_checks, f"self_review_failed:{','.join(failed_self_checks)}")
    require(all(public_payload_checks(self_review).values()), "self_review_public_safety_failed")
    review_path = args.out_root / "R3-E" / "03-final-self-review.json"
    write_json(review_path, self_review)
    review_hash = write_sidecar(review_path)

    print(
        json.dumps(
            {
                "status": self_review["status"],
                "self_review": f"{self_review['summary']['pass_count']}/{self_review['summary']['check_count']}",
                "r3_quantitative_gate": final_decision["decisions"][
                    "r3_quantitative_gate"
                ],
                "hard_gate_failures": hard_failures,
                "f4_checks": f"{f4_audit['summary']['pass_count']}/{f4_audit['summary']['check_count']}",
                "f4_metric_matches": f"{sum(f4['metric_comparisons'].values())}/{len(f4['metric_comparisons'])}",
                "final_decision_sha256": output_hashes[e_path.name],
                "self_review_sha256": review_hash,
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
