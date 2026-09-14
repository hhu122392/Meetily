#!/usr/bin/env python3
"""Independently audit a generated 195-second P0-R review package."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
from pathlib import Path
import sys
import wave
from typing import Any


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def data_row_count(path: Path) -> int:
    with path.open("r", encoding="utf-8-sig", newline="") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    return max(0, len(rows) - 1)


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
    package = args.package.resolve()

    config_path = package / "review-config.json"
    if not config_path.is_file():
        raise FileNotFoundError("Missing package file: review-config.json")
    config = read_json(config_path)
    audio_name = str(config.get("window_audio_name", "")).strip()
    if not audio_name or Path(audio_name).name != audio_name:
        raise ValueError("review-config.json has an invalid window_audio_name")

    required_names = [
        "00-scope-and-truth-boundary.json",
        audio_name,
        "02-moss-machine-candidate.json",
        "03-whisper-draft.json",
        "04-human-verbatim.tsv",
        "05-human-speaker-turns.tsv",
        "06-human-review.json",
        "07-term-candidates.json",
        "08-scoring-rules.json",
        "09-gap-audit.json",
        "TASKS.md",
        "MANIFEST.json",
    ]
    missing = [name for name in required_names if not (package / name).is_file()]
    if missing:
        raise FileNotFoundError(f"Missing package files: {missing}")

    scope = read_json(package / required_names[0])
    machine = read_json(package / required_names[2])
    whisper = read_json(package / required_names[3])
    review = read_json(package / required_names[6])
    terms = read_json(package / required_names[7])
    rules = read_json(package / required_names[8])
    gap = read_json(package / required_names[9])
    manifest = read_json(package / "MANIFEST.json")
    audio_path = package / required_names[1]

    with wave.open(str(audio_path), "rb") as handle:
        audio_duration = handle.getnframes() / handle.getframerate()
        audio_format = {
            "channels": handle.getnchannels(),
            "sample_rate_hz": handle.getframerate(),
            "sample_width_bytes": handle.getsampwidth(),
        }

    manifest_mismatches: list[dict[str, Any]] = []
    for item in manifest.get("files", []):
        path = package / item["relative_path"]
        if not path.is_file():
            manifest_mismatches.append(
                {"relative_path": item["relative_path"], "reason": "missing"}
            )
            continue
        actual_hash = sha256(path)
        actual_bytes = path.stat().st_size
        if actual_hash != item.get("sha256") or actual_bytes != item.get("bytes"):
            manifest_mismatches.append(
                {
                    "relative_path": item["relative_path"],
                    "reason": "hash_or_size_mismatch",
                    "expected_sha256": item.get("sha256"),
                    "actual_sha256": actual_hash,
                    "expected_bytes": item.get("bytes"),
                    "actual_bytes": actual_bytes,
                }
            )

    machine_segments = machine.get("segments", [])
    illegal_machine_times = []
    for item in machine_segments:
        start = float(item.get("start_seconds", 0.0))
        end = float(item.get("end_seconds", 0.0))
        if start < 0.0 or end <= start or end > 195.0:
            illegal_machine_times.append(
                {"row": item.get("row"), "start": start, "end": end}
            )

    verbatim_rows = data_row_count(package / "04-human-verbatim.tsv")
    speaker_rows = data_row_count(package / "05-human-speaker-turns.tsv")
    audio_hash = sha256(audio_path)
    scope_audio = scope.get("reference_window", {}).get("audio", {})

    checks = {
        "manifest_entries_match": len(manifest_mismatches) == 0,
        "source_hash_is_frozen": scope.get("source", {}).get("sha256")
        == "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB",
        "reference_audio_hash_bound": audio_hash == scope_audio.get("sha256")
        == machine.get("reference_audio_sha256")
        == whisper.get("reference_audio_sha256")
        == review.get("audio_sha256")
        == rules.get("audio_sha256"),
        "reference_duration_exact": abs(audio_duration - 195.0) <= 0.001,
        "reference_format_exact": audio_format
        == {"channels": 1, "sample_rate_hz": 16000, "sample_width_bytes": 2},
        "reference_is_in_180_to_300_second_range": 180.0 <= audio_duration <= 300.0,
        "unresolvable_sentence_excluded": scope.get("excluded_boundary", {}).get(
            "unresolvable_sentence_included"
        )
        is False,
        "machine_candidate_marked_non_truth": machine.get("is_ground_truth") is False
        and machine.get("approved_as_ground_truth") is False,
        "whisper_draft_marked_non_truth": whisper.get("is_ground_truth") is False
        and whisper.get("approved_as_ground_truth") is False,
        "machine_timestamps_legal": len(illegal_machine_times) == 0,
        "machine_candidate_has_multiple_people": len(
            {
                item.get("speaker_candidate")
                for item in machine_segments
                if item.get("speaker_candidate")
            }
        )
        >= 2,
        "formal_verbatim_is_empty": verbatim_rows == 0,
        "formal_speaker_truth_is_empty": speaker_rows == 0,
        "human_review_not_forged": review.get("approved_as_ground_truth") is False
        and review.get("listened_from_start_to_end") is False,
        "term_gate_remains_closed": terms.get("gate", {}).get("ready") is False,
        "gap_verdict_remains_blocked": gap.get("verdict")
        == "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
    }
    structural_pass = all(checks.values())

    audit = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-REPLACEMENT-195S-INDEPENDENT-AUDIT",
        "structural_status": "PASS" if structural_pass else "FAIL",
        "release_status": "BLOCKED_HUMAN_TRUTH_PENDING",
        "checks": checks,
        "details": {
            "audio": {
                "duration_seconds": audio_duration,
                "sha256": audio_hash,
                **audio_format,
            },
            "machine_segment_count": len(machine_segments),
            "whisper_segment_count": whisper.get("segment_count"),
            "formal_verbatim_rows": verbatim_rows,
            "formal_speaker_turn_rows": speaker_rows,
            "manifest_mismatches": manifest_mismatches,
            "illegal_machine_times": illegal_machine_times,
        },
        "metrics": {
            "cer": "NOT_SCORABLE_NO_HUMAN_VERBATIM",
            "speaker_turn_error_rate": "NOT_SCORABLE_NO_HUMAN_SPEAKER_TURNS",
            "speaker_duration_error_rate": "NOT_SCORABLE_NO_HUMAN_SPEAKER_TURNS",
            "positive_term_accuracy": "NOT_SCORABLE_ONLY_ONE_HUMAN_CONFIRMED_TERM",
            "negative_term_false_insertion": "NOT_SCORABLE_NO_HUMAN_NEGATIVE_TERMS",
        },
        "verdict": "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
        "truth_rule": (
            "结构通过不等于准确率通过。没有真人独立逐字稿、说话人边界、术语确认和签字，"
            "不得生成准确率数字或发布结论。"
        ),
    }
    audit_path = package / "10-independent-audit.json"
    audit_path.write_text(
        json.dumps(audit, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    manifest_files = sorted(
        path
        for path in package.iterdir()
        if path.is_file()
        and path.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    rebuilt_manifest = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-REPLACEMENT-195S",
        "status": (
            "STRUCTURE_PASS_HUMAN_TRUTH_BLOCKED"
            if structural_pass
            else "STRUCTURE_FAIL"
        ),
        "reference_audio_sha256": audio_hash,
        "files": [file_record(path, package) for path in manifest_files],
    }
    (package / "MANIFEST.json").write_text(
        json.dumps(rebuilt_manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )

    print(
        json.dumps(
            {
                "structural_status": audit["structural_status"],
                "release_status": audit["release_status"],
                "audit": str(audit_path),
            },
            ensure_ascii=False,
        )
    )
    return 0 if structural_pass else 1


if __name__ == "__main__":
    sys.exit(main())
