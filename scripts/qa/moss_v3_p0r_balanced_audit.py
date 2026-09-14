#!/usr/bin/env python3
"""Independently audit the sealed balanced P0-R human truth package."""

from __future__ import annotations

import argparse
import csv
import hashlib
import importlib
import json
from pathlib import Path
import sys
import tempfile
import wave
from typing import Any, Iterable


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


EXPECTED_FULL_AUDIO_SHA256 = (
    "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
)
EXPECTED_DURATION_SECONDS = 226.440


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def write_json(path: Path, payload: Any) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def interval_union_seconds(intervals: Iterable[tuple[float, float]]) -> float:
    merged: list[list[float]] = []
    for start, end in sorted((float(a), float(b)) for a, b in intervals if b > a):
        if not merged or start > merged[-1][1] + 0.001:
            merged.append([start, end])
        else:
            merged[-1][1] = max(merged[-1][1], end)
    return sum(end - start for start, end in merged)


def file_record(path: Path, root: Path) -> dict[str, Any]:
    return {
        "relative_path": path.relative_to(root).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    args = parser.parse_args()
    package = args.package.resolve(strict=True)
    required = [
        "00-scope-and-truth-boundary.json",
        "01-source-lock.json",
        "04-human-verbatim.tsv",
        "05-human-speaker-turns.tsv",
        "06-human-review.json",
        "08-scoring-rules.json",
        "11-formal-human-truth-audit.json",
        "16-pre-meeting-context.json",
        "17-derived-human-review-provenance.json",
        "human-review-work-in-progress.json",
        "review-config.json",
        "score_s8_m00_r3.py",
        "MANIFEST.json",
    ]
    missing = [name for name in required if not (package / name).is_file()]
    if missing:
        raise FileNotFoundError(missing)

    config = read_json(package / "review-config.json")
    audio = package / str(config["window_audio_name"])
    review = read_json(package / "06-human-review.json")
    rules = read_json(package / "08-scoring-rules.json")
    formal_audit = read_json(package / "11-formal-human-truth-audit.json")
    provenance = read_json(package / "17-derived-human-review-provenance.json")
    wip = read_json(package / "human-review-work-in-progress.json")
    source_lock = read_json(package / "01-source-lock.json")
    manifest = read_json(package / "MANIFEST.json")
    with wave.open(str(audio), "rb") as reader:
        duration = reader.getnframes() / float(reader.getframerate())
        audio_format = {
            "channels": reader.getnchannels(),
            "sample_rate_hz": reader.getframerate(),
            "sample_width_bytes": reader.getsampwidth(),
        }
    audio_hash = sha256(audio)

    manifest_mismatches: list[dict[str, Any]] = []
    for item in manifest.get("files", []):
        path = package / str(item["relative_path"])
        if not path.is_file():
            manifest_mismatches.append({"path": item["relative_path"], "reason": "missing"})
        elif path.stat().st_size != item.get("bytes") or sha256(path) != item.get("sha256"):
            manifest_mismatches.append({"path": item["relative_path"], "reason": "hash_or_size"})

    with (package / "05-human-speaker-turns.tsv").open(
        "r", encoding="utf-8-sig", newline=""
    ) as handle:
        turns = list(csv.DictReader(handle, delimiter="\t"))
    speaker_seconds: dict[str, float] = {}
    speaker_turns: dict[str, int] = {}
    valid_intervals: list[tuple[float, float]] = []
    for row in turns:
        if row["valid_for_scoring"] != "true":
            continue
        speaker = row["reference_speaker_id"]
        start = int(row["start_ms"]) / 1000.0
        end = int(row["end_ms"]) / 1000.0
        speaker_seconds[speaker] = speaker_seconds.get(speaker, 0.0) + end - start
        speaker_turns[speaker] = speaker_turns.get(speaker, 0) + 1
        valid_intervals.append((start, end))

    sys.path.insert(0, str(package))
    scorer = importlib.import_module("score_s8_m00_r3")
    with tempfile.TemporaryDirectory(prefix="meetily-balanced-audit-") as temp_dir:
        preview = Path(temp_dir) / "rules.json"
        preview.write_text(
            json.dumps(
                {
                    "status": "FROZEN_BEFORE_CUDA_OUTPUT",
                    "frozen_at": review["review_completed_at"],
                    "annotation_validation": rules["annotation_validation"],
                },
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        validation = scorer.validate_annotation_kit(
            audio,
            package / "04-human-verbatim.tsv",
            package / "05-human-speaker-turns.tsv",
            package / "06-human-review.json",
            preview,
        )
    failed_validation = [
        check for check in validation.get("checks", []) if check.get("status") != "PASS"
    ]

    source_hashes_ok = True
    provenance_mismatches: list[dict[str, Any]] = []
    for item in provenance.get("sources", []):
        path = Path(str(item.get("wip_path", "")))
        if not path.is_file() or sha256(path) != item.get("wip_sha256"):
            source_hashes_ok = False
            provenance_mismatches.append({"path": str(path), "reason": "missing_or_hash"})

    annotation = rules["annotation_validation"]
    primary_floor = float(annotation["minimum_seconds_per_primary_speaker"])
    turn_floor = int(annotation["minimum_valid_turns_per_speaker"])
    checks = {
        "required_files_present": not missing,
        "manifest_entries_match_before_audit": not manifest_mismatches,
        "source_full_audio_hash_frozen": str(source_lock["source_audio"]["sha256"]).upper()
        == EXPECTED_FULL_AUDIO_SHA256,
        "reference_audio_hash_bound": audio_hash
        == str(config["window_audio_sha256"]).upper()
        == str(review["audio_sha256"]).upper()
        == str(rules["audio_sha256"]).upper(),
        "duration_exact_and_policy_range": abs(duration - EXPECTED_DURATION_SECONDS) <= 0.001
        and 180.0 <= duration <= 300.0,
        "audio_format_exact": audio_format
        == {"channels": 1, "sample_rate_hz": 16000, "sample_width_bytes": 2},
        "review_formally_approved": review.get("review_status") == "HUMAN_VERIFIED"
        and review.get("approved_as_ground_truth") is True,
        "reviewer_identity_fixed": review.get("reviewer_id") == "lili"
        and review.get("reviewer_role") == "会议记录",
        "playback_coverage_complete": review.get("playback_coverage", {}).get("complete") is True
        and float(review["playback_coverage"]["coverage_percent"]) == 100.0
        and float(review["playback_coverage"]["maximum_gap_seconds"]) == 0.0
        and float(review["playback_coverage"]["wall_elapsed_seconds"]) >= duration,
        "all_wip_rows_checked": bool(wip.get("rows"))
        and all(row.get("human_checked") is True for row in wip["rows"]),
        "human_file_hashes_match_review": sha256(package / "04-human-verbatim.tsv")
        == str(review["verbatim_tsv_sha256"]).upper()
        and sha256(package / "05-human-speaker-turns.tsv")
        == str(review["speaker_turns_tsv_sha256"]).upper(),
        "two_primary_speakers_meet_seconds": set(speaker_seconds) == {"H01", "H04"}
        and all(value + 0.001 >= primary_floor for value in speaker_seconds.values()),
        "each_speaker_has_multiple_turns": all(
            value >= turn_floor for value in speaker_turns.values()
        ),
        "speaker_turn_union_meets_frozen_floor": interval_union_seconds(valid_intervals) + 0.001
        >= float(annotation["minimum_speaker_turn_union_seconds"]),
        "formal_audit_agrees": formal_audit.get("status") == "STRUCTURE_PASS_SELF_ATTESTED"
        and formal_audit.get("checks", {}).get("formal_validator_result")
        == "STRUCTURE_PASS_SELF_ATTESTED",
        "derivation_sources_hash_bound": source_hashes_ok,
        "selection_did_not_use_accuracy": provenance.get("selection_guard", {}).get(
            "accuracy_outputs_consulted"
        )
        is False,
        "frozen_rules_not_weakened": provenance.get("selection_guard", {}).get(
            "rules_weakened"
        )
        is False,
        "production_annotation_validator_passes": validation.get("validation_result")
        == "STRUCTURE_PASS_SELF_ATTESTED"
        and not failed_validation,
        "next_gate_is_formal_whisper": formal_audit.get("next_gate")
        == "FORMAL_WHISPER_SUPERVISED_RUN_BEFORE_HOTWORD_FREEZE",
        "full_scoring_rules_remain_pending_whisper_hashes": rules.get("status")
        == "FROZEN_FOR_BALANCED_226S_REFERENCE_HUMAN_TRUTH_PENDING"
        and rules.get("input_lock", {}).get("whisper_full_baseline_sha256") is None,
    }
    passed = all(checks.values())
    audit = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-BALANCED-226S-INDEPENDENT-AUDIT",
        "structural_status": "PASS" if passed else "FAIL",
        "release_status": (
            "P0R_PASS_NEXT_FORMAL_WHISPER" if passed else "P0R_BLOCKED_AUDIT_FAILURE"
        ),
        "checks": checks,
        "details": {
            "audio": {"duration_seconds": duration, "sha256": audio_hash, **audio_format},
            "speaker_seconds": {
                key: round(value, 3) for key, value in sorted(speaker_seconds.items())
            },
            "speaker_turns": dict(sorted(speaker_turns.items())),
            "speaker_turn_union_seconds": round(interval_union_seconds(valid_intervals), 3),
            "production_validator_check_count": len(validation.get("checks", [])),
            "production_validator_failures": failed_validation,
            "manifest_mismatches": manifest_mismatches,
            "provenance_mismatches": provenance_mismatches,
        },
        "truth_boundary": "程序只证明哈希、时长、字段和冻结门槛通过；语义真实性来自lili已明确确认的人工听审。",
        "verdict": (
            "P0-R HUMAN TRUTH STRUCTURE PASS / FORMAL WHISPER REQUIRED"
            if passed
            else "P0-R BLOCKED"
        ),
    }
    audit_path = package / "18-independent-formal-audit.json"
    write_json(audit_path, audit)
    files = sorted(
        path
        for path in package.iterdir()
        if path.is_file() and path.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    write_json(
        package / "MANIFEST.json",
        {
            "schema_version": 1,
            "stage": "MOSS-V3-P0R-BALANCED-226S",
            "status": (
                "HUMAN_TRUTH_STRUCTURE_PASS_NEXT_FORMAL_WHISPER"
                if passed
                else "HUMAN_TRUTH_AUDIT_FAIL"
            ),
            "reference_audio_sha256": audio_hash,
            "files": [file_record(path, package) for path in files],
        },
    )
    print(
        json.dumps(
            {
                "structural_status": audit["structural_status"],
                "release_status": audit["release_status"],
                "failed_checks": [name for name, value in checks.items() if not value],
                "audit": str(audit_path),
            },
            ensure_ascii=False,
        )
    )
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
