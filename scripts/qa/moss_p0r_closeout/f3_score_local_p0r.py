from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import sys
from datetime import datetime, timezone
from pathlib import Path
from types import ModuleType
from typing import Any


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


def load_frozen_scorer(path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location("meetily_frozen_s8_scorer", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot import frozen scorer: {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def manifest_by_role(manifest: dict[str, Any]) -> dict[str, dict[str, Any]]:
    entries = manifest.get("entries")
    if not isinstance(entries, list):
        raise ValueError("F1 manifest entries must be a list")
    result: dict[str, dict[str, Any]] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        role = str(entry.get("role", ""))
        if not role or role in result:
            raise ValueError(f"invalid or duplicate F1 role: {role}")
        result[role] = entry
    return result


def path_for_role(entries: dict[str, dict[str, Any]], role: str) -> Path:
    entry = entries.get(role)
    if not isinstance(entry, dict):
        raise ValueError(f"F1 manifest missing role: {role}")
    path = Path(str(entry.get("path", "")))
    if not path.is_file():
        raise FileNotFoundError(f"F1 role path missing: {role}: {path}")
    return path


def rehash_manifest_roles(
    entries: dict[str, dict[str, Any]], roles: list[str]
) -> tuple[dict[str, dict[str, Any]], list[str]]:
    results: dict[str, dict[str, Any]] = {}
    mismatches: list[str] = []
    for role in roles:
        path = path_for_role(entries, role)
        actual_hash = sha256_file(path)
        expected_hash = str(entries[role].get("sha256", "")).casefold()
        actual_bytes = path.stat().st_size
        expected_bytes = int(entries[role].get("bytes", -1))
        matches = actual_hash == expected_hash and actual_bytes == expected_bytes
        results[role] = {
            "path": str(path.resolve()),
            "expected_sha256": expected_hash,
            "actual_sha256": actual_hash,
            "expected_bytes": expected_bytes,
            "actual_bytes": actual_bytes,
            "matches": matches,
        }
        if not matches:
            mismatches.append(role)
    return results, mismatches


def moss_full_segments(payload: dict[str, Any]) -> list[dict[str, Any]]:
    turns = payload.get("global_turns")
    if not isinstance(turns, list):
        return []
    segments: list[dict[str, Any]] = []
    for item in turns:
        if not isinstance(item, dict):
            continue
        start = float(item.get("global_start_ms", -1)) / 1000.0
        end = float(item.get("global_end_ms", -1)) / 1000.0
        text = str(item.get("text", ""))
        speaker = str(item.get("speaker_label", ""))
        if start >= 0 and end > start:
            segments.append(
                {
                    "start": start,
                    "end": end,
                    "text": text,
                    "speaker": speaker,
                    "chunk_index": int(item.get("chunk_index", 0)),
                }
            )
    return sorted(segments, key=lambda item: (item["start"], item["end"]))


def whisper_full_segments(payload: dict[str, Any]) -> list[dict[str, Any]]:
    raw = payload.get("segments")
    if not isinstance(raw, list):
        return []
    segments: list[dict[str, Any]] = []
    for item in raw:
        if not isinstance(item, dict):
            continue
        start = float(item.get("start", -1))
        end = float(item.get("end", -1))
        if start >= 0 and end > start:
            segments.append(
                {
                    "start": start,
                    "end": end,
                    "text": str(item.get("text", "")),
                    "speaker": str(item.get("speaker", "")),
                }
            )
    return sorted(segments, key=lambda item: (item["start"], item["end"]))


def boundary_spill_seconds(
    segments: list[dict[str, Any]], start_seconds: float, end_seconds: float
) -> dict[str, Any]:
    selected = [
        segment
        for segment in segments
        if segment["end"] > start_seconds and segment["start"] < end_seconds
    ]
    start_spill = max(
        (
            start_seconds - float(segment["start"])
            for segment in selected
            if segment["start"] < start_seconds
        ),
        default=0.0,
    )
    end_spill = max(
        (
            float(segment["end"]) - end_seconds
            for segment in selected
            if segment["end"] > end_seconds
        ),
        default=0.0,
    )
    return {
        "start_spill_seconds": start_spill,
        "end_spill_seconds": end_spill,
        "maximum_spill_seconds": max(start_spill, end_spill),
        "selected_full_segment_count": len(selected),
    }


def compact_timeline_metrics(metrics: dict[str, Any]) -> dict[str, Any]:
    return {
        key: value
        for key, value in metrics.items()
        if key not in {"overlapping_segment_pairs"}
    } | {
        "overlapping_segment_pair_count": len(
            metrics.get("overlapping_segment_pairs", [])
        )
    }


def max_segment_duration(segments: list[dict[str, Any]]) -> float:
    return max(
        (float(item["end"]) - float(item["start"]) for item in segments),
        default=0.0,
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Score the frozen 226.440s P0-R window using MOSS P1 global turns and formal Whisper."
    )
    parser.add_argument("--f1-manifest", type=Path, required=True)
    parser.add_argument("--source-lock", type=Path, required=True)
    parser.add_argument("--positive-terms", type=Path, required=True)
    parser.add_argument("--negative-blocker", type=Path, required=True)
    parser.add_argument("--f2-audit", type=Path, required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    now = datetime.now(timezone.utc).isoformat()

    direct_inputs = [
        args.f1_manifest,
        args.source_lock,
        args.positive_terms,
        args.negative_blocker,
        args.f2_audit,
    ]
    missing_direct = [str(path) for path in direct_inputs if not path.is_file()]
    if missing_direct:
        write_json(
            args.out_dir / "02-F3-audit.json",
            {
                "schema_version": 1,
                "role": "P0R_F3_LOCAL_SCORING_AUDIT",
                "created_at": now,
                "status": "FAIL",
                "missing_direct_inputs": missing_direct,
            },
        )
        return 2

    manifest = load_json(args.f1_manifest)
    entries = manifest_by_role(manifest)
    required_roles = [
        "formal_whisper_full_output",
        "formal_whisper_run_record",
        "human_review",
        "human_speaker_turns",
        "human_verbatim",
        "moss_p1_private_output",
        "pre_meeting_context",
        "reference_window_audio",
        "scoring_rules_draft",
        "source_full_audio",
        "strict_s8_cuda_scorer",
    ]
    role_audit, role_mismatches = rehash_manifest_roles(entries, required_roles)
    if role_mismatches:
        write_json(
            args.out_dir / "02-F3-audit.json",
            {
                "schema_version": 1,
                "role": "P0R_F3_LOCAL_SCORING_AUDIT",
                "created_at": now,
                "status": "FAIL",
                "manifest_role_mismatches": role_mismatches,
                "role_audit": role_audit,
                "decision": "STOP_SCORING_INPUT_HASH_MISMATCH",
            },
        )
        return 2

    scorer_path = path_for_role(entries, "strict_s8_cuda_scorer")
    frozen = load_frozen_scorer(scorer_path)
    rules = load_json(path_for_role(entries, "scoring_rules_draft"))
    source_lock = load_json(args.source_lock)
    positive_payload = load_json(args.positive_terms)
    negative_blocker = load_json(args.negative_blocker)
    f2_audit = load_json(args.f2_audit)
    human_review = load_json(path_for_role(entries, "human_review"))
    formal_payload = load_json(path_for_role(entries, "formal_whisper_full_output"))
    formal_record = load_json(path_for_role(entries, "formal_whisper_run_record"))
    moss_payload = load_json(path_for_role(entries, "moss_p1_private_output"))

    window_lock = source_lock.get("source_audio", {})
    window_start = float(window_lock["review_window_source_start_seconds"])
    window_end = float(window_lock["review_window_source_end_seconds"])
    window_duration = window_end - window_start
    expected_window_duration = float(rules["audio_window_duration_seconds"])

    truth_segments = frozen.truth_segments_from_tsv(
        path_for_role(entries, "human_verbatim")
    )
    truth_turn_data = frozen.truth_turn_data_from_tsv(
        path_for_role(entries, "human_speaker_turns")
    )
    truth_turns = truth_turn_data["scored_turns"]
    ignored_turn_intervals = truth_turn_data["ignore_intervals"]
    truth_text = "".join(str(item["text"]) for item in truth_segments)

    moss_full = moss_full_segments(moss_payload)
    whisper_full = whisper_full_segments(formal_payload)
    moss_window = frozen.derive_window_segments(
        moss_full, window_start, window_end
    )
    whisper_window = frozen.derive_window_segments(
        whisper_full, window_start, window_end
    )

    moss_spill = boundary_spill_seconds(moss_full, window_start, window_end)
    whisper_spill = boundary_spill_seconds(whisper_full, window_start, window_end)
    moss_timestamp_audit = frozen.validate_prediction_timestamps(
        moss_window, window_duration
    )
    whisper_timestamp_audit = frozen.validate_prediction_timestamps(
        whisper_window, window_duration
    )
    moss_timeline = compact_timeline_metrics(
        frozen.output_timeline_metrics(moss_window)
    )
    whisper_timeline = compact_timeline_metrics(
        frozen.output_timeline_metrics(whisper_window)
    )

    moss_text = "".join(str(item["text"]) for item in moss_window)
    whisper_text = "".join(str(item["text"]) for item in whisper_window)
    moss_cer = frozen.cer(truth_text, moss_text)
    whisper_cer = frozen.cer(truth_text, whisper_text)

    moss_speaker = frozen.score_speaker_turns(
        truth_turns, moss_window, ignored_turn_intervals
    )
    whisper_has_speaker_labels = any(
        str(item.get("speaker", "")).strip() for item in whisper_window
    )
    whisper_speaker = (
        frozen.score_speaker_turns(
            truth_turns, whisper_window, ignored_turn_intervals
        )
        if whisper_has_speaker_labels
        else {
            "status": "NOT_APPLICABLE_FORMAL_WHISPER_HAS_NO_SPEAKER_LABELS",
            "predicted_speaker_count": 0,
        }
    )

    positive_targets = positive_payload.get("positive_spoken_terms", [])
    if not isinstance(positive_targets, list):
        raise ValueError("positive_spoken_terms must be a list")
    term_rules = rules["term_scoring"]
    term_kwargs = {
        "minimum_hypothesis_overlap_ratio": float(
            term_rules["positive_alignment_minimum_hypothesis_overlap_ratio"]
        ),
        "maximum_hypothesis_to_reference_duration_ratio": float(
            term_rules[
                "positive_alignment_maximum_hypothesis_to_reference_duration_ratio"
            ]
        ),
        "maximum_target_center_distance_seconds": float(
            term_rules["positive_alignment_maximum_target_center_distance_seconds"]
        ),
    }
    moss_positive_terms = frozen.score_positive_terms_by_alignment(
        truth_segments, moss_window, positive_targets, **term_kwargs
    )
    whisper_positive_terms = frozen.score_positive_terms_by_alignment(
        truth_segments, whisper_window, positive_targets, **term_kwargs
    )

    negative_candidates = negative_blocker.get("negative_review_candidates", [])
    if not isinstance(negative_candidates, list):
        negative_candidates = []
    candidate_targets = [
        {"term": item.get("term"), "type": item.get("type")}
        for item in negative_candidates
        if isinstance(item, dict) and item.get("term")
    ]
    moss_negative_candidate_search = frozen.score_terms(
        [str(item["text"]) for item in moss_full], candidate_targets
    )
    whisper_negative_candidate_search = frozen.score_terms(
        [str(item["text"]) for item in whisper_full], candidate_targets
    )

    selected_moss_chunks = sorted(
        {
            int(item["chunk_index"])
            for item in moss_full
            if item["end"] > window_start and item["start"] < window_end
        }
    )
    cross_chunk_proven = moss_payload.get(
        "cross_chunk_speaker_identity_proven"
    ) is True
    speaker_identity_scope_ok = len(selected_moss_chunks) <= 1 or cross_chunk_proven

    full_gate = moss_payload.get("global_gate", {})
    if not isinstance(full_gate, dict):
        full_gate = {}
    full_rules = rules["full_run_gates"]
    full_normalized_characters = len(
        frozen.normalize_text("".join(str(item["text"]) for item in moss_full))
    )
    full_distinct_speakers = len(
        {str(item["speaker"]) for item in moss_full if str(item["speaker"]).strip()}
    )
    full_max_segment_duration = max_segment_duration(moss_full)
    first_segment_start = min(
        (float(item["start"]) for item in moss_full), default=math.inf
    )
    last_segment_end = max(
        (float(item["end"]) for item in moss_full), default=-math.inf
    )

    speaker_rules = rules["speaker_scoring"]
    max_spill = float(rules["window_derivation"]["maximum_boundary_spill_seconds"])
    local_hard_gates: dict[str, dict[str, Any]] = {
        "input_hashes_match_f1_manifest": {
            "pass": not role_mismatches,
            "actual": len(role_mismatches),
            "threshold": 0,
        },
        "source_window_duration_matches_frozen_rules": {
            "pass": math.isclose(
                window_duration, expected_window_duration, abs_tol=0.001
            ),
            "actual": window_duration,
            "threshold": expected_window_duration,
        },
        "moss_window_boundary_spill": {
            "pass": moss_spill["maximum_spill_seconds"] <= max_spill,
            "actual": moss_spill["maximum_spill_seconds"],
            "threshold_maximum": max_spill,
        },
        "moss_window_timestamps_valid": {
            "pass": moss_timestamp_audit["valid"] is True,
            "actual_invalid_segments": moss_timestamp_audit[
                "invalid_segment_count"
            ],
            "threshold": 0,
        },
        "moss_cer": {
            "pass": float(moss_cer["cer"])
            <= float(rules["cer"]["moss_cer_absolute_maximum"]),
            "actual": moss_cer["cer"],
            "threshold_maximum": rules["cer"]["moss_cer_absolute_maximum"],
        },
        "moss_speaker_turn_error_rate": {
            "pass": float(moss_speaker["error_rate"])
            <= float(speaker_rules["gate_max_error_rate"]),
            "actual": moss_speaker["error_rate"],
            "threshold_maximum": speaker_rules["gate_max_error_rate"],
        },
        "moss_speaker_duration_error_rate": {
            "pass": float(moss_speaker["duration_error_rate"])
            <= float(speaker_rules["gate_max_duration_error_rate"]),
            "actual": moss_speaker["duration_error_rate"],
            "threshold_maximum": speaker_rules["gate_max_duration_error_rate"],
        },
        "moss_speaker_false_alarm_seconds": {
            "pass": float(moss_speaker["false_alarm_seconds"])
            <= float(speaker_rules["gate_max_false_alarm_seconds"]),
            "actual": moss_speaker["false_alarm_seconds"],
            "threshold_maximum": speaker_rules["gate_max_false_alarm_seconds"],
        },
        "moss_speaker_false_alarm_rate": {
            "pass": float(moss_speaker["false_alarm_rate"])
            <= float(speaker_rules["gate_max_false_alarm_rate"]),
            "actual": moss_speaker["false_alarm_rate"],
            "threshold_maximum": speaker_rules["gate_max_false_alarm_rate"],
        },
        "moss_cross_chunk_speaker_identity": {
            "pass": speaker_identity_scope_ok,
            "selected_chunk_indices": selected_moss_chunks,
            "cross_chunk_speaker_identity_proven": cross_chunk_proven,
        },
        "moss_positive_terms_exact": {
            "pass": moss_positive_terms["exact_occurrence_target_count"]
            == moss_positive_terms["target_count"]
            and moss_positive_terms["target_count"]
            >= int(term_rules["minimum_positive_targets"]),
            "actual_exact_targets": moss_positive_terms[
                "exact_occurrence_target_count"
            ],
            "target_count": moss_positive_terms["target_count"],
        },
        "moss_full_global_gate": {
            "pass": full_gate.get("state") == "PASS",
            "actual": full_gate.get("state"),
            "threshold": "PASS",
        },
        "moss_full_rtf": {
            "pass": float(full_gate.get("total_rtf", math.inf))
            <= float(full_rules["rtf_maximum"]),
            "actual": full_gate.get("total_rtf"),
            "threshold_maximum": full_rules["rtf_maximum"],
        },
        "moss_full_first_segment_start": {
            "pass": first_segment_start
            <= float(full_rules["maximum_first_segment_start_seconds"]),
            "actual": first_segment_start,
            "threshold_maximum": full_rules[
                "maximum_first_segment_start_seconds"
            ],
        },
        "moss_full_last_segment_end": {
            "pass": last_segment_end
            >= float(full_rules["minimum_last_segment_end_seconds"]),
            "actual": last_segment_end,
            "threshold_minimum": full_rules["minimum_last_segment_end_seconds"],
        },
        "moss_full_output_segment_count": {
            "pass": len(moss_full) >= int(full_rules["minimum_output_segments"]),
            "actual": len(moss_full),
            "threshold_minimum": full_rules["minimum_output_segments"],
        },
        "moss_full_normalized_character_count": {
            "pass": full_normalized_characters
            >= int(full_rules["minimum_normalized_characters"]),
            "actual": full_normalized_characters,
            "threshold_minimum": full_rules["minimum_normalized_characters"],
        },
        "moss_full_distinct_speaker_labels": {
            "pass": full_distinct_speakers
            >= int(full_rules["minimum_distinct_speaker_labels"]),
            "actual": full_distinct_speakers,
            "threshold_minimum": full_rules["minimum_distinct_speaker_labels"],
        },
        "moss_full_maximum_segment_duration": {
            "pass": full_max_segment_duration
            <= float(full_rules["maximum_segment_duration_seconds"]),
            "actual": full_max_segment_duration,
            "threshold_maximum": full_rules["maximum_segment_duration_seconds"],
        },
    }
    hard_failures = [
        name for name, result in local_hard_gates.items() if result.get("pass") is not True
    ]
    negative_gate_blocked = (
        negative_blocker.get("status")
        == "BLOCKED_HUMAN_FULL_AUDIO_NEGATIVE_ATTESTATION_MISSING"
        and negative_blocker.get("current_human_attestation_present") is False
    )
    quantitative_status = "PASS" if not hard_failures else "FAIL"
    if hard_failures:
        overall_status = "LOCAL_P0R_QUANTITATIVE_FAIL_NO_GO"
    elif negative_gate_blocked:
        overall_status = "LOCAL_P0R_QUANTITATIVE_PASS_NEGATIVE_GATE_BLOCKED_NO_GO"
    else:
        overall_status = "LOCAL_P0R_PASS_PENDING_INDEPENDENT_AUDIT"

    report = {
        "schema_version": 1,
        "role": "P0R_F3_LOCAL_SCORING_REPORT",
        "created_at": now,
        "status": overall_status,
        "scope": "LOCAL_CURRENT_PC_MOSS_P1_CHUNKED_VS_FORMAL_WHISPER_226_440_SECONDS",
        "release_go": False,
        "input_bindings": {
            "f1_manifest_path": str(args.f1_manifest.resolve()),
            "f1_manifest_sha256": sha256_file(args.f1_manifest),
            "source_lock_path": str(args.source_lock.resolve()),
            "source_lock_sha256": sha256_file(args.source_lock),
            "positive_terms_path": str(args.positive_terms.resolve()),
            "positive_terms_sha256": sha256_file(args.positive_terms),
            "negative_blocker_path": str(args.negative_blocker.resolve()),
            "negative_blocker_sha256": sha256_file(args.negative_blocker),
            "f2_audit_path": str(args.f2_audit.resolve()),
            "f2_audit_sha256": sha256_file(args.f2_audit),
            "local_scorer_path": str(Path(__file__).resolve()),
            "local_scorer_sha256": sha256_file(Path(__file__).resolve()),
            "frozen_strict_scorer_sha256": sha256_file(scorer_path),
            "role_rehash_audit": role_audit,
        },
        "source_window": {
            "source_start_seconds": window_start,
            "source_end_seconds": window_end,
            "duration_seconds": window_duration,
            "window_audio_sha256": role_audit["reference_window_audio"][
                "actual_sha256"
            ],
            "full_audio_sha256": role_audit["source_full_audio"]["actual_sha256"],
        },
        "reference": {
            "speech_segment_count": len(truth_segments),
            "speaker_turn_count": len(truth_turns),
            "speaker_count": len({item["speaker"] for item in truth_turns}),
            "normalized_character_count": len(frozen.normalize_text(truth_text)),
            "human_review_approved": human_review.get("approved_as_ground_truth")
            is True,
        },
        "moss": {
            "source_stage": moss_payload.get("stage"),
            "source_run_state": moss_payload.get("run_state"),
            "source_release_go": moss_payload.get("release_go"),
            "full_segment_count": len(moss_full),
            "window_segment_count": len(moss_window),
            "selected_chunk_indices": selected_moss_chunks,
            "cross_chunk_speaker_identity_proven": cross_chunk_proven,
            "boundary_spill": moss_spill,
            "timestamp_audit": moss_timestamp_audit,
            "timeline": moss_timeline,
            "cer": moss_cer,
            "speaker": moss_speaker,
            "positive_terms": moss_positive_terms,
            "negative_candidate_search": moss_negative_candidate_search,
            "full_runtime": {
                "global_gate_state": full_gate.get("state"),
                "total_rtf": full_gate.get("total_rtf"),
                "first_segment_start_seconds": first_segment_start,
                "last_segment_end_seconds": last_segment_end,
                "normalized_character_count": full_normalized_characters,
                "distinct_speaker_label_count": full_distinct_speakers,
                "maximum_segment_duration_seconds": full_max_segment_duration,
            },
        },
        "formal_whisper": {
            "run_id": formal_payload.get("run_id"),
            "execution_status": formal_record.get("execution_status"),
            "backend": formal_payload.get("backend"),
            "full_segment_count": len(whisper_full),
            "window_segment_count": len(whisper_window),
            "boundary_spill": whisper_spill,
            "timestamp_audit": whisper_timestamp_audit,
            "timeline": whisper_timeline,
            "cer": whisper_cer,
            "speaker": whisper_speaker,
            "positive_terms": whisper_positive_terms,
            "negative_candidate_search": whisper_negative_candidate_search,
        },
        "gates": {
            "quantitative_status": quantitative_status,
            "hard_gate_count": len(local_hard_gates),
            "hard_gate_failure_count": len(hard_failures),
            "hard_gate_failures": hard_failures,
            "hard_gate_details": local_hard_gates,
            "negative_term_gate": {
                "status": "BLOCKED" if negative_gate_blocked else "UNKNOWN",
                "reason": (
                    "完整 737.728 秒负向术语专门人工声明缺失"
                    if negative_gate_blocked
                    else "状态不符合已知阻断记录"
                ),
            },
            "strict_s8_cuda_lane": {
                "status": "BLOCKED",
                "reason": (
                    "当前输入是 MOSS_V3_P1_CHUNKED schema_version=1，"
                    "不是 LOCKED_FULL_MOSS_OUTPUT schema_version=2，也没有受监督 CUDA 正式运行。"
                ),
            },
        },
        "truth_boundary": (
            "报告没有写入完整逐字正文；CER、说话人和术语结果由冻结输入机械计算。"
            "负向人工条件缺失和任一数值失败都不能改写成通过。"
        ),
    }
    score_path = args.out_dir / "01-local-p0r-score.json"
    write_json(score_path, report)
    score_hash = sha256_file(score_path)

    audit_checks = {
        "all_required_f1_roles_rehashed": len(role_audit) == len(required_roles),
        "all_required_f1_role_hashes_match": not role_mismatches,
        "f2_structural_status_pass": f2_audit.get("structural_status") == "PASS",
        "f2_positive_status_frozen": positive_payload.get("status")
        == "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN",
        "f2_negative_blocker_preserved": negative_gate_blocked,
        "human_review_approved": human_review.get("approved_as_ground_truth") is True,
        "window_duration_matches": math.isclose(
            window_duration, expected_window_duration, abs_tol=0.001
        ),
        "moss_window_nonempty": len(moss_window) > 0,
        "whisper_window_nonempty": len(whisper_window) > 0,
        "moss_metric_report_complete": all(
            key in report["moss"]
            for key in ("cer", "speaker", "positive_terms", "full_runtime")
        ),
        "whisper_metric_report_complete": all(
            key in report["formal_whisper"]
            for key in ("cer", "speaker", "positive_terms")
        ),
        "hard_failures_not_hidden": (
            bool(hard_failures)
            and overall_status == "LOCAL_P0R_QUANTITATIVE_FAIL_NO_GO"
        )
        or (not hard_failures),
        "release_go_is_false": report["release_go"] is False,
        "strict_s8_remains_blocked": report["gates"]["strict_s8_cuda_lane"][
            "status"
        ]
        == "BLOCKED",
    }
    audit_failed = [name for name, passed in audit_checks.items() if not passed]
    audit = {
        "schema_version": 1,
        "role": "P0R_F3_LOCAL_SCORING_AUDIT",
        "created_at": now,
        "status": "PASS" if not audit_failed else "FAIL",
        "checks": audit_checks,
        "summary": {
            "check_count": len(audit_checks),
            "pass_count": sum(audit_checks.values()),
            "failed_count": len(audit_failed),
            "failed_checks": audit_failed,
        },
        "score_report_path": str(score_path.resolve()),
        "score_report_sha256": score_hash,
        "score_status": overall_status,
        "quantitative_hard_failures": hard_failures,
        "decision": "F3_COMPLETE_PROCEED_TO_INDEPENDENT_F4_AUDIT"
        if not audit_failed
        else "F3_AUDIT_FAIL_STOP",
    }
    audit_path = args.out_dir / "02-F3-audit.json"
    write_json(audit_path, audit)
    audit_hash = sha256_file(audit_path)

    conclusion = f"""# F3 本地评分结论

- 评分执行状态：`{audit['status']}`
- 本地总状态：`{overall_status}`
- MOSS CER：`{moss_cer['cer']:.6f}`，门槛 `<= {rules['cer']['moss_cer_absolute_maximum']}`
- Whisper CER：`{whisper_cer['cer']:.6f}`，仅作同窗口基线
- MOSS 说话人轮次错误率：`{moss_speaker['error_rate']:.6f}`，门槛 `<= {speaker_rules['gate_max_error_rate']}`
- MOSS 说话人时长错误率：`{moss_speaker['duration_error_rate']:.6f}`，门槛 `<= {speaker_rules['gate_max_duration_error_rate']}`
- MOSS 正向术语精确通过：`{moss_positive_terms['exact_occurrence_target_count']}/{moss_positive_terms['target_count']}`
- 数值硬门禁失败数：`{len(hard_failures)}`
- 数值硬门禁失败项：`{', '.join(hard_failures) if hard_failures else '无'}`
- 负向术语门禁：`BLOCKED`
- 严格 S8 CUDA：`BLOCKED`
- 发布判定：`NO-GO`
- 评分报告 SHA-256：`{score_hash}`
- F3 审计 SHA-256：`{audit_hash}`

该结果只代表当前电脑上的 MOSS P1 分块输出与正式 Whisper 在同一 226.440 秒真人真值窗口上的机械评分。它不是 L2 发布通过，也没有把缺失的负向人工声明或跨分块说话人身份拼接伪装成已完成。
"""
    write_text(args.out_dir / "03-F3结论.md", conclusion)

    print(
        json.dumps(
            {
                "audit_status": audit["status"],
                "score_status": overall_status,
                "moss_cer": moss_cer["cer"],
                "whisper_cer": whisper_cer["cer"],
                "moss_speaker_turn_error_rate": moss_speaker["error_rate"],
                "moss_speaker_duration_error_rate": moss_speaker[
                    "duration_error_rate"
                ],
                "moss_positive_exact_targets": moss_positive_terms[
                    "exact_occurrence_target_count"
                ],
                "positive_target_count": moss_positive_terms["target_count"],
                "hard_gate_failures": hard_failures,
                "score_sha256": score_hash,
                "audit_sha256": audit_hash,
            },
            ensure_ascii=False,
        )
    )
    return 0 if not audit_failed else 2


if __name__ == "__main__":
    sys.exit(main())
