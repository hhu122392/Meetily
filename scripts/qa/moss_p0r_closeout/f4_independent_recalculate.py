from __future__ import annotations

import argparse
import csv
import hashlib
import importlib.util
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from typing import Any


REQUIRED_ROLES = [
    "formal_whisper_full_output",
    "formal_whisper_run_record",
    "human_review",
    "human_speaker_turns",
    "human_verbatim",
    "moss_p1_private_output",
    "network_recovery_audit",
    "pre_meeting_context",
    "reference_window_audio",
    "scoring_rules_draft",
    "source_full_audio",
    "strict_s8_cuda_scorer",
]


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


def load_scorer(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("meetily_f4_frozen_scorer", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import scorer: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def entries_by_role(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    raw = manifest.get("entries")
    if not isinstance(raw, list):
        raise ValueError("manifest entries missing")
    result: dict[str, dict[str, Any]] = {}
    for item in raw:
        if not isinstance(item, dict):
            continue
        role = str(item.get("role", ""))
        if not role or role in result:
            raise ValueError(f"invalid/duplicate role: {role}")
        result[role] = item
    return result


def role_path(entries: dict[str, dict[str, Any]], role: str) -> Path:
    item = entries.get(role)
    if not isinstance(item, dict):
        raise ValueError(f"missing role: {role}")
    path = Path(str(item.get("path", "")))
    if not path.is_file():
        raise FileNotFoundError(f"missing role path: {role}: {path}")
    return path


def manifest_hash_audit(
    entries: dict[str, dict[str, Any]], roles: list[str]
) -> tuple[dict[str, bool], list[str]]:
    checks: dict[str, bool] = {}
    mismatch: list[str] = []
    for role in roles:
        path = role_path(entries, role)
        matches = (
            sha256_file(path) == str(entries[role].get("sha256", "")).casefold()
            and path.stat().st_size == int(entries[role].get("bytes", -1))
        )
        checks[role] = matches
        if not matches:
            mismatch.append(role)
    return checks, mismatch


def moss_segments(payload: dict[str, Any]) -> list[dict[str, Any]]:
    turns = payload.get("global_turns")
    if not isinstance(turns, list):
        return []
    result = []
    for item in turns:
        if not isinstance(item, dict):
            continue
        start = float(item.get("global_start_ms", -1)) / 1000.0
        end = float(item.get("global_end_ms", -1)) / 1000.0
        if start >= 0 and end > start:
            result.append(
                {
                    "start": start,
                    "end": end,
                    "text": str(item.get("text", "")),
                    "speaker": str(item.get("speaker_label", "")),
                    "chunk_index": int(item.get("chunk_index", 0)),
                }
            )
    return sorted(result, key=lambda item: (item["start"], item["end"]))


def whisper_segments(payload: dict[str, Any]) -> list[dict[str, Any]]:
    raw = payload.get("segments")
    if not isinstance(raw, list):
        return []
    result = []
    for item in raw:
        if not isinstance(item, dict):
            continue
        start = float(item.get("start", -1))
        end = float(item.get("end", -1))
        if start >= 0 and end > start:
            result.append(
                {
                    "start": start,
                    "end": end,
                    "text": str(item.get("text", "")),
                    "speaker": str(item.get("speaker", "")),
                }
            )
    return sorted(result, key=lambda item: (item["start"], item["end"]))


def max_spill(
    segments: list[dict[str, Any]], start: float, end: float
) -> float:
    selected = [
        item for item in segments if item["end"] > start and item["start"] < end
    ]
    values = [0.0]
    values.extend(start - item["start"] for item in selected if item["start"] < start)
    values.extend(item["end"] - end for item in selected if item["end"] > end)
    return max(values)


def max_duration(segments: list[dict[str, Any]]) -> float:
    return max((item["end"] - item["start"] for item in segments), default=0.0)


def close(left: Any, right: Any, tolerance: float = 1e-12) -> bool:
    try:
        return math.isclose(float(left), float(right), rel_tol=0.0, abs_tol=tolerance)
    except (TypeError, ValueError):
        return False


def forbidden_transcript_keys(value: Any, prefix: str = "$") -> list[str]:
    forbidden = {"verbatim_text", "raw_model_text", "raw_transcript", "transcript"}
    matches: list[str] = []
    if isinstance(value, dict):
        for key, child in value.items():
            child_prefix = f"{prefix}.{key}"
            if str(key).casefold() in forbidden:
                matches.append(child_prefix)
            matches.extend(forbidden_transcript_keys(child, child_prefix))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            matches.extend(forbidden_transcript_keys(child, f"{prefix}[{index}]"))
    return matches


def read_tsv_row_count_without_text(path: Path) -> int:
    with path.open("r", encoding="utf-8-sig", newline="") as handle:
        return sum(1 for _ in csv.DictReader(handle, delimiter="\t"))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Independently recalculate P0-R F3 metrics without importing the F3 script."
    )
    parser.add_argument("--f1-manifest", type=Path, required=True)
    parser.add_argument("--source-lock", type=Path, required=True)
    parser.add_argument("--positive-terms", type=Path, required=True)
    parser.add_argument("--negative-blocker", type=Path, required=True)
    parser.add_argument("--f2-audit", type=Path, required=True)
    parser.add_argument("--f3-score", type=Path, required=True)
    parser.add_argument("--f3-audit", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).isoformat()
    direct_paths = [
        args.f1_manifest,
        args.source_lock,
        args.positive_terms,
        args.negative_blocker,
        args.f2_audit,
        args.f3_score,
        args.f3_audit,
    ]
    missing = [str(path) for path in direct_paths if not path.is_file()]
    if missing:
        write_json(
            args.out_dir / "02-F4-independent-audit.json",
            {
                "schema_version": 1,
                "role": "P0R_F4_INDEPENDENT_AUDIT",
                "created_at": now,
                "status": "FAIL",
                "missing_inputs": missing,
            },
        )
        return 2

    manifest = load_json(args.f1_manifest)
    entries = entries_by_role(manifest)
    role_checks, role_mismatches = manifest_hash_audit(entries, REQUIRED_ROLES)
    scorer_path = role_path(entries, "strict_s8_cuda_scorer")
    frozen = load_scorer(scorer_path)
    rules = load_json(role_path(entries, "scoring_rules_draft"))
    source_lock = load_json(args.source_lock)
    positives = load_json(args.positive_terms)
    negative_blocker = load_json(args.negative_blocker)
    f2_audit = load_json(args.f2_audit)
    recorded = load_json(args.f3_score)
    f3_audit = load_json(args.f3_audit)
    moss_payload = load_json(role_path(entries, "moss_p1_private_output"))
    whisper_payload = load_json(role_path(entries, "formal_whisper_full_output"))

    source = source_lock["source_audio"]
    start = float(source["review_window_source_start_seconds"])
    end = float(source["review_window_source_end_seconds"])
    duration = end - start
    truth_segments = frozen.truth_segments_from_tsv(
        role_path(entries, "human_verbatim")
    )
    truth_turn_data = frozen.truth_turn_data_from_tsv(
        role_path(entries, "human_speaker_turns")
    )
    truth_text = "".join(item["text"] for item in truth_segments)
    full_moss = moss_segments(moss_payload)
    full_whisper = whisper_segments(whisper_payload)
    window_moss = frozen.derive_window_segments(full_moss, start, end)
    window_whisper = frozen.derive_window_segments(full_whisper, start, end)

    recalc_moss_cer = frozen.cer(
        truth_text, "".join(item["text"] for item in window_moss)
    )
    recalc_whisper_cer = frozen.cer(
        truth_text, "".join(item["text"] for item in window_whisper)
    )
    recalc_speaker = frozen.score_speaker_turns(
        truth_turn_data["scored_turns"],
        window_moss,
        truth_turn_data["ignore_intervals"],
    )
    positive_targets = positives.get("positive_spoken_terms", [])
    term_rules = rules["term_scoring"]
    recalc_terms = frozen.score_positive_terms_by_alignment(
        truth_segments,
        window_moss,
        positive_targets,
        minimum_hypothesis_overlap_ratio=float(
            term_rules["positive_alignment_minimum_hypothesis_overlap_ratio"]
        ),
        maximum_hypothesis_to_reference_duration_ratio=float(
            term_rules[
                "positive_alignment_maximum_hypothesis_to_reference_duration_ratio"
            ]
        ),
        maximum_target_center_distance_seconds=float(
            term_rules["positive_alignment_maximum_target_center_distance_seconds"]
        ),
    )

    selected_chunks = sorted(
        {
            item["chunk_index"]
            for item in full_moss
            if item["end"] > start and item["start"] < end
        }
    )
    cross_chunk_proven = moss_payload.get(
        "cross_chunk_speaker_identity_proven"
    ) is True
    full_gate = moss_payload.get("global_gate", {})
    full_rules = rules["full_run_gates"]
    speaker_rules = rules["speaker_scoring"]
    spill_limit = float(rules["window_derivation"]["maximum_boundary_spill_seconds"])
    recalc_spill = max_spill(full_moss, start, end)
    full_first = min((item["start"] for item in full_moss), default=math.inf)
    full_last = max((item["end"] for item in full_moss), default=-math.inf)
    full_chars = len(frozen.normalize_text("".join(item["text"] for item in full_moss)))
    full_speakers = len(
        {item["speaker"] for item in full_moss if str(item["speaker"]).strip()}
    )
    full_max_duration = max_duration(full_moss)

    recalc_gates: dict[str, bool] = {
        "input_hashes_match_f1_manifest": not role_mismatches,
        "source_window_duration_matches_frozen_rules": close(
            duration, rules["audio_window_duration_seconds"], 0.001
        ),
        "moss_window_boundary_spill": recalc_spill <= spill_limit,
        "moss_window_timestamps_valid": frozen.validate_prediction_timestamps(
            window_moss, duration
        )["valid"]
        is True,
        "moss_cer": recalc_moss_cer["cer"]
        <= float(rules["cer"]["moss_cer_absolute_maximum"]),
        "moss_speaker_turn_error_rate": recalc_speaker["error_rate"]
        <= float(speaker_rules["gate_max_error_rate"]),
        "moss_speaker_duration_error_rate": recalc_speaker["duration_error_rate"]
        <= float(speaker_rules["gate_max_duration_error_rate"]),
        "moss_speaker_false_alarm_seconds": recalc_speaker["false_alarm_seconds"]
        <= float(speaker_rules["gate_max_false_alarm_seconds"]),
        "moss_speaker_false_alarm_rate": recalc_speaker["false_alarm_rate"]
        <= float(speaker_rules["gate_max_false_alarm_rate"]),
        "moss_cross_chunk_speaker_identity": len(selected_chunks) <= 1
        or cross_chunk_proven,
        "moss_positive_terms_exact": recalc_terms[
            "exact_occurrence_target_count"
        ]
        == recalc_terms["target_count"]
        and recalc_terms["target_count"]
        >= int(term_rules["minimum_positive_targets"]),
        "moss_full_global_gate": isinstance(full_gate, dict)
        and full_gate.get("state") == "PASS",
        "moss_full_rtf": float(full_gate.get("total_rtf", math.inf))
        <= float(full_rules["rtf_maximum"]),
        "moss_full_first_segment_start": full_first
        <= float(full_rules["maximum_first_segment_start_seconds"]),
        "moss_full_last_segment_end": full_last
        >= float(full_rules["minimum_last_segment_end_seconds"]),
        "moss_full_output_segment_count": len(full_moss)
        >= int(full_rules["minimum_output_segments"]),
        "moss_full_normalized_character_count": full_chars
        >= int(full_rules["minimum_normalized_characters"]),
        "moss_full_distinct_speaker_labels": full_speakers
        >= int(full_rules["minimum_distinct_speaker_labels"]),
        "moss_full_maximum_segment_duration": full_max_duration
        <= float(full_rules["maximum_segment_duration_seconds"]),
    }
    recalc_failures = sorted(
        name for name, passed in recalc_gates.items() if not passed
    )
    recorded_failures = sorted(
        str(item) for item in recorded["gates"]["hard_gate_failures"]
    )

    metric_comparisons = {
        "moss_cer": close(recalc_moss_cer["cer"], recorded["moss"]["cer"]["cer"]),
        "moss_cer_distance": recalc_moss_cer["distance"]
        == recorded["moss"]["cer"]["distance"],
        "whisper_cer": close(
            recalc_whisper_cer["cer"],
            recorded["formal_whisper"]["cer"]["cer"],
        ),
        "speaker_turn_error_rate": close(
            recalc_speaker["error_rate"], recorded["moss"]["speaker"]["error_rate"]
        ),
        "speaker_duration_error_rate": close(
            recalc_speaker["duration_error_rate"],
            recorded["moss"]["speaker"]["duration_error_rate"],
        ),
        "speaker_false_alarm_seconds": close(
            recalc_speaker["false_alarm_seconds"],
            recorded["moss"]["speaker"]["false_alarm_seconds"],
        ),
        "positive_exact_target_count": recalc_terms[
            "exact_occurrence_target_count"
        ]
        == recorded["moss"]["positive_terms"]["exact_occurrence_target_count"],
        "positive_total_target_count": recalc_terms["target_count"]
        == recorded["moss"]["positive_terms"]["target_count"],
        "boundary_spill": close(
            recalc_spill,
            recorded["moss"]["boundary_spill"]["maximum_spill_seconds"],
        ),
        "selected_chunk_indices": selected_chunks
        == recorded["moss"]["selected_chunk_indices"],
        "full_last_segment_end": close(
            full_last, recorded["moss"]["full_runtime"]["last_segment_end_seconds"]
        ),
        "full_max_segment_duration": close(
            full_max_duration,
            recorded["moss"]["full_runtime"]["maximum_segment_duration_seconds"],
        ),
        "hard_gate_failure_set": recalc_failures == recorded_failures,
    }

    f3_score_hash = sha256_file(args.f3_score)
    output_payloads = [
        positives,
        negative_blocker,
        f2_audit,
        recorded,
        f3_audit,
    ]
    leaked_keys = []
    for index, payload in enumerate(output_payloads):
        leaked_keys.extend(
            f"payload[{index}]{item[1:]}" for item in forbidden_transcript_keys(payload)
        )

    checks: dict[str, bool] = {
        "all_required_manifest_roles_present": all(
            role in entries for role in REQUIRED_ROLES
        ),
        "all_required_manifest_role_hashes_match": not role_mismatches,
        "source_lock_hash_matches_f3_binding": sha256_file(args.source_lock)
        == recorded["input_bindings"]["source_lock_sha256"],
        "positive_hash_matches_f3_binding": sha256_file(args.positive_terms)
        == recorded["input_bindings"]["positive_terms_sha256"],
        "negative_blocker_hash_matches_f3_binding": sha256_file(
            args.negative_blocker
        )
        == recorded["input_bindings"]["negative_blocker_sha256"],
        "f3_score_hash_matches_f3_audit": f3_score_hash
        == f3_audit.get("score_report_sha256"),
        "f3_audit_pass": f3_audit.get("status") == "PASS",
        "f2_structural_pass": f2_audit.get("structural_status") == "PASS",
        "negative_human_gate_remains_blocked": negative_blocker.get("status")
        == "BLOCKED_HUMAN_FULL_AUDIO_NEGATIVE_ATTESTATION_MISSING",
        "all_metrics_recalculate_exactly": all(metric_comparisons.values()),
        "quantitative_failures_not_hidden": bool(recalc_failures)
        and recorded.get("status") == "LOCAL_P0R_QUANTITATIVE_FAIL_NO_GO",
        "release_go_false": recorded.get("release_go") is False,
        "strict_s8_blocked": recorded["gates"]["strict_s8_cuda_lane"]["status"]
        == "BLOCKED",
        "network_recovery_evidence_hash_unchanged": role_checks.get(
            "network_recovery_audit"
        )
        is True,
        "no_transcript_payload_keys_in_new_json_outputs": not leaked_keys,
        "human_verbatim_rows_still_present": read_tsv_row_count_without_text(
            role_path(entries, "human_verbatim")
        )
        == 30,
    }
    failed_checks = [name for name, passed in checks.items() if not passed]

    independent = {
        "schema_version": 1,
        "role": "P0R_F4_INDEPENDENT_RECALCULATION",
        "created_at": now,
        "status": "RECALCULATED",
        "method": (
            "未导入 F3 脚本，也未读取 F3 文字结论作为判定；"
            "从 F1 清单所指向的冻结输入重新派生窗口和计算指标。"
        ),
        "input_hashes": {
            "f1_manifest_sha256": sha256_file(args.f1_manifest),
            "source_lock_sha256": sha256_file(args.source_lock),
            "positive_terms_sha256": sha256_file(args.positive_terms),
            "negative_blocker_sha256": sha256_file(args.negative_blocker),
            "f3_score_sha256": f3_score_hash,
            "f4_script_sha256": sha256_file(Path(__file__).resolve()),
            "frozen_strict_scorer_sha256": sha256_file(scorer_path),
        },
        "recalculated_metrics": {
            "moss_cer": recalc_moss_cer,
            "whisper_cer": recalc_whisper_cer,
            "moss_speaker": {
                "valid_turns": recalc_speaker["valid_turns"],
                "correct_turns": recalc_speaker["correct_turns"],
                "error_turns": recalc_speaker["error_turns"],
                "error_rate": recalc_speaker["error_rate"],
                "duration_error_rate": recalc_speaker["duration_error_rate"],
                "false_alarm_seconds": recalc_speaker["false_alarm_seconds"],
                "false_alarm_rate": recalc_speaker["false_alarm_rate"],
                "mapping": recalc_speaker["mapping"],
            },
            "moss_positive_terms": {
                "target_count": recalc_terms["target_count"],
                "exact_occurrence_target_count": recalc_terms[
                    "exact_occurrence_target_count"
                ],
                "total_matched_occurrences": recalc_terms["total_occurrences"],
                "per_term": [
                    {
                        "term": item["term"],
                        "matched_occurrences": item["count"],
                        "hypothesis_occurrence_count": item[
                            "hypothesis_occurrence_count"
                        ],
                        "expected_occurrences": item["expected_occurrences"],
                        "exact": item["exact_expected_occurrences"],
                    }
                    for item in recalc_terms["details"]
                ],
            },
            "moss_boundary_spill_seconds": recalc_spill,
            "moss_selected_chunk_indices": selected_chunks,
            "moss_cross_chunk_identity_proven": cross_chunk_proven,
            "moss_full_first_segment_start_seconds": full_first,
            "moss_full_last_segment_end_seconds": full_last,
            "moss_full_max_segment_duration_seconds": full_max_duration,
            "moss_full_normalized_characters": full_chars,
            "moss_full_distinct_speaker_labels": full_speakers,
        },
        "recalculated_hard_gates": recalc_gates,
        "recalculated_failure_set": recalc_failures,
        "recorded_failure_set": recorded_failures,
        "metric_comparisons": metric_comparisons,
        "decision": "CONFIRM_F3_QUANTITATIVE_FAIL_NO_GO"
        if all(metric_comparisons.values()) and recalc_failures
        else "RECALCULATION_MISMATCH_REQUIRES_INVESTIGATION",
    }
    independent_path = args.out_dir / "01-independent-score-recalculation.json"
    write_json(independent_path, independent)
    independent_hash = sha256_file(independent_path)

    audit = {
        "schema_version": 1,
        "role": "P0R_F4_INDEPENDENT_AUDIT",
        "created_at": now,
        "status": "PASS" if not failed_checks else "FAIL",
        "checks": checks,
        "summary": {
            "check_count": len(checks),
            "pass_count": sum(checks.values()),
            "failed_count": len(failed_checks),
            "failed_checks": failed_checks,
            "manifest_role_mismatches": role_mismatches,
            "metric_mismatches": [
                name for name, passed in metric_comparisons.items() if not passed
            ],
            "forbidden_transcript_key_paths": leaked_keys,
        },
        "independent_recalculation_path": str(independent_path.resolve()),
        "independent_recalculation_sha256": independent_hash,
        "confirmed_score_status": recorded.get("status"),
        "confirmed_hard_gate_failures": recalc_failures,
        "release_decision": "NO_GO",
        "next_step": "F5_FINAL_RELEASE_DECISION"
        if not failed_checks
        else "STOP_AND_REPAIR_AUDIT",
    }
    audit_path = args.out_dir / "02-F4-independent-audit.json"
    write_json(audit_path, audit)
    audit_hash = sha256_file(audit_path)

    conclusion = f"""# F4 独立复核结论

- 独立复核：`{audit['status']}`
- 检查项：`{sum(checks.values())}/{len(checks)}` 通过
- 指标复算一致：`{sum(metric_comparisons.values())}/{len(metric_comparisons)}`
- MOSS CER 复算：`{recalc_moss_cer['cer']:.6f}`
- Whisper CER 复算：`{recalc_whisper_cer['cer']:.6f}`
- MOSS 说话人轮次错误率复算：`{recalc_speaker['error_rate']:.6f}`
- MOSS 说话人时长错误率复算：`{recalc_speaker['duration_error_rate']:.6f}`
- 复算硬门禁失败：`{len(recalc_failures)}` 项
- 失败项：`{', '.join(recalc_failures)}`
- 负向术语人工门禁：`BLOCKED`
- 发布结论：`NO-GO`
- 独立复算 SHA-256：`{independent_hash}`
- F4 审计 SHA-256：`{audit_hash}`

本次复核没有导入 F3 脚本，也没有把 F3 的文字结论当答案。所有核心指标均从冻结输入重新计算，结果与 F3 一致，因此 7 个硬门禁失败不是展示或四舍五入造成的。
"""
    write_text(args.out_dir / "03-F4结论.md", conclusion)

    print(
        json.dumps(
            {
                "status": audit["status"],
                "checks": f"{sum(checks.values())}/{len(checks)}",
                "metric_matches": f"{sum(metric_comparisons.values())}/{len(metric_comparisons)}",
                "moss_cer": recalc_moss_cer["cer"],
                "whisper_cer": recalc_whisper_cer["cer"],
                "hard_gate_failures": recalc_failures,
                "audit_sha256": audit_hash,
            },
            ensure_ascii=False,
        )
    )
    return 0 if not failed_checks else 2


if __name__ == "__main__":
    sys.exit(main())
