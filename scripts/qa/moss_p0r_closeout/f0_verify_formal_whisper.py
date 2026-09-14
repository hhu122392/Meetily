from __future__ import annotations

import argparse
import hashlib
import json
import math
import sys
import wave
from datetime import datetime, timezone
from pathlib import Path
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


def wav_duration_seconds(path: Path) -> float:
    with wave.open(str(path), "rb") as audio:
        return audio.getnframes() / float(audio.getframerate())


def close_enough(left: Any, right: Any, tolerance: float = 0.01) -> bool:
    try:
        return math.isclose(float(left), float(right), abs_tol=tolerance)
    except (TypeError, ValueError):
        return False


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Verify the completed formal Whisper run without printing transcript text."
    )
    parser.add_argument("--raw", type=Path, required=True)
    parser.add_argument("--full", type=Path, required=True)
    parser.add_argument("--run-record", type=Path, required=True)
    parser.add_argument("--status", type=Path, required=True)
    parser.add_argument("--full-audio", type=Path, required=True)
    parser.add_argument("--window-audio", type=Path, required=True)
    parser.add_argument("--stable-exe", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--supervisor", type=Path, required=True)
    parser.add_argument("--gate-supervisor", type=Path, required=True)
    parser.add_argument("--scorer", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    inputs = {
        "raw_product_output": args.raw,
        "formal_full_output": args.full,
        "formal_run_record": args.run_record,
        "formal_status": args.status,
        "source_full_audio": args.full_audio,
        "reference_window_audio": args.window_audio,
        "stable_exe": args.stable_exe,
        "whisper_model": args.model,
        "formal_supervisor": args.supervisor,
        "shared_gate_supervisor": args.gate_supervisor,
        "scorer": args.scorer,
    }

    missing = [name for name, path in inputs.items() if not path.is_file()]
    if missing:
        result = {
            "schema_version": 1,
            "role": "P0R_F0_FORMAL_WHISPER_PREFLIGHT_AUDIT",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "status": "FAIL",
            "missing_inputs": missing,
            "checks": {"all_inputs_exist": False},
        }
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(json.dumps({"status": "FAIL", "missing_input_count": len(missing)}))
        return 2

    file_records = {
        name: {
            "path": str(path.resolve()),
            "bytes": path.stat().st_size,
            "sha256": sha256_file(path),
        }
        for name, path in inputs.items()
    }

    raw = load_json(args.raw)
    full = load_json(args.full)
    record = load_json(args.run_record)
    status = load_json(args.status)
    segments = full.get("segments")
    if not isinstance(segments, list):
        segments = []

    valid_segment_times = True
    previous_start = -1.0
    first_start = None
    last_end = None
    for segment in segments:
        if not isinstance(segment, dict):
            valid_segment_times = False
            continue
        try:
            start = float(segment["start"])
            end = float(segment["end"])
        except (KeyError, TypeError, ValueError):
            valid_segment_times = False
            continue
        if start < 0 or end <= start or start < previous_start:
            valid_segment_times = False
        previous_start = start
        if first_start is None:
            first_start = start
        last_end = end

    supervision = record.get("supervision")
    if not isinstance(supervision, dict):
        supervision = {}
    log_audit = supervision.get("log_audit")
    if not isinstance(log_audit, dict):
        log_audit = {}
    firewall = supervision.get("firewall")
    if not isinstance(firewall, dict):
        firewall = {}
    removed = firewall.get("removed")
    if not isinstance(removed, dict):
        removed = {}
    removed_after = removed.get("after")
    if not isinstance(removed_after, dict):
        removed_after = {}
    removed_after_records = removed_after.get("records")
    if not isinstance(removed_after_records, list):
        removed_after_records = ["invalid"]
    job_close = supervision.get("job_close")
    if not isinstance(job_close, dict):
        job_close = {}

    full_audio_duration = wav_duration_seconds(args.full_audio)
    window_audio_duration = wav_duration_seconds(args.window_audio)
    expected_run_id = "9bddab2ace05477dad8376daf73da3c3"
    window_start = 70.370
    window_end = 296.810

    checks: dict[str, bool] = {
        "all_inputs_exist": True,
        "run_id_is_expected": str(record.get("run_id")) == expected_run_id,
        "run_id_matches_full_output": str(full.get("run_id")) == str(record.get("run_id")),
        "run_id_matches_status": str(status.get("run_id")) == str(record.get("run_id")),
        "record_role_is_formal": record.get("role") == "FORMAL_CURRENT_WHISPER_RUN_RECORD",
        "full_role_is_formal": full.get("role") == "FORMAL_CURRENT_WHISPER_FULL_OUTPUT",
        "execution_completed": record.get("execution_status") == "COMPLETED",
        "process_exit_code_zero": record.get("process_exit_code") == 0,
        "backend_is_vulkan": record.get("backend") == "Vulkan",
        "language_is_zh": record.get("language") == "zh",
        "not_timed_out": supervision.get("timed_out") is False,
        "capture_complete": supervision.get("capture_complete") is True,
        "process_started_suspended": supervision.get("process_started_suspended") is True,
        "job_assigned_before_resume": supervision.get("job_assigned_before_resume") is True,
        "job_closed_cleanly": job_close.get("close_handle_succeeded") is True,
        "process_tree_cleanup_complete": supervision.get("process_tree_cleanup_complete") is True,
        "log_audit_clean": log_audit.get("clean") is True,
        "raw_logs_not_written": log_audit.get("raw_logs_written_to_disk") is False,
        "firewall_run_rules_removed": removed.get("removed") is True,
        "firewall_run_rule_records_empty_after": len(removed_after_records) == 0,
        "no_observed_remote_connections": len(supervision.get("observed_remote_connections") or []) == 0,
        "no_observed_listeners": len(supervision.get("observed_listening_sockets") or []) == 0,
        "no_observation_errors": len(supervision.get("observation_errors") or []) == 0,
        "raw_output_hash_bound": str(record.get("raw_product_output_sha256", "")).casefold()
        == file_records["raw_product_output"]["sha256"],
        "formal_output_hash_bound": str(record.get("output_full_sha256", "")).casefold()
        == file_records["formal_full_output"]["sha256"],
        "status_formal_output_hash_bound": str(status.get("formal_output_sha256", "")).casefold()
        == file_records["formal_full_output"]["sha256"],
        "status_run_record_hash_bound": str(status.get("run_record_sha256", "")).casefold()
        == file_records["formal_run_record"]["sha256"],
        "full_audio_hash_bound": str(record.get("source_full_audio_sha256", "")).casefold()
        == file_records["source_full_audio"]["sha256"],
        "window_audio_hash_bound": str(record.get("window_audio_sha256", "")).casefold()
        == file_records["reference_window_audio"]["sha256"],
        "stable_exe_hash_bound": str(record.get("stable_exe_sha256", "")).casefold()
        == file_records["stable_exe"]["sha256"],
        "model_hash_bound": str(record.get("model_file_sha256", "")).casefold()
        == file_records["whisper_model"]["sha256"],
        "supervisor_hash_bound": str(record.get("supervisor_source_sha256", "")).casefold()
        == file_records["formal_supervisor"]["sha256"],
        "gate_supervisor_hash_bound": str(record.get("shared_gate_supervisor_sha256", "")).casefold()
        == file_records["shared_gate_supervisor"]["sha256"],
        "scorer_hash_bound": str(record.get("scorer_sha256", "")).casefold()
        == file_records["scorer"]["sha256"],
        "full_output_source_hash_matches_record": str(full.get("source_full_audio_sha256", "")).casefold()
        == str(record.get("source_full_audio_sha256", "")).casefold(),
        "full_output_model_hash_matches_record": str(full.get("model_file_sha256", "")).casefold()
        == str(record.get("model_file_sha256", "")).casefold(),
        "full_output_exe_hash_matches_record": str(full.get("stable_exe_sha256", "")).casefold()
        == str(record.get("stable_exe_sha256", "")).casefold(),
        "full_output_backend_matches_record": full.get("backend") == record.get("backend"),
        "full_output_language_matches_record": full.get("language") == record.get("language"),
        "full_audio_duration_matches_wave": close_enough(
            full.get("audio_duration_seconds"), full_audio_duration
        ),
        "window_audio_duration_is_226_440": close_enough(window_audio_duration, 226.440),
        "segments_nonempty": len(segments) > 0,
        "segment_times_valid_and_sorted": valid_segment_times,
        "full_output_spans_scoring_window": first_start is not None
        and first_start <= window_start
        and last_end is not None
        and last_end >= window_end,
        "formal_output_transformation_did_not_change_text": (
            isinstance(full.get("transformation"), dict)
            and full["transformation"].get("text_changed") is False
            and full["transformation"].get("timestamps_changed") is False
            and full["transformation"].get("speaker_labels_changed") is False
        ),
        "status_is_completed_not_yet_frozen": status.get("status")
        == "COMPLETED_NOT_YET_FROZEN_IN_SCORING_BUNDLE",
        "status_does_not_allow_go_yet": status.get("go_allowed") is False,
    }

    failed_checks = [name for name, passed in checks.items() if not passed]
    result = {
        "schema_version": 1,
        "role": "P0R_F0_FORMAL_WHISPER_PREFLIGHT_AUDIT",
        "created_at": datetime.now(timezone.utc).isoformat(),
        "run_id": record.get("run_id"),
        "status": "PASS" if not failed_checks else "FAIL",
        "transcript_text_included": False,
        "summary": {
            "check_count": len(checks),
            "passed_count": len(checks) - len(failed_checks),
            "failed_count": len(failed_checks),
            "failed_checks": failed_checks,
            "formal_segment_count": len(segments),
            "formal_first_start_seconds": first_start,
            "formal_last_end_seconds": last_end,
            "source_full_audio_duration_seconds": full_audio_duration,
            "reference_window_duration_seconds": window_audio_duration,
        },
        "checks": checks,
        "inputs": file_records,
        "decision": (
            "F0_PASS_ALLOW_F1_F2_F3F4_PREPARATION"
            if not failed_checks
            else "F0_FAIL_STOP_BEFORE_SCORING_BUNDLE"
        ),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {
                "status": result["status"],
                "check_count": len(checks),
                "failed_count": len(failed_checks),
                "decision": result["decision"],
                "output_sha256": sha256_file(args.out),
            },
            ensure_ascii=False,
        )
    )
    return 0 if not failed_checks else 2


if __name__ == "__main__":
    sys.exit(main())
