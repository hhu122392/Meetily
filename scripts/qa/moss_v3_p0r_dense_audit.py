#!/usr/bin/env python3
"""Independently audit a dense continuous P0-R review package."""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
from pathlib import Path
import sys
import wave
from typing import Any, Iterable


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


EXPECTED_SOURCE_SHA256 = (
    "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
)


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
        return max(0, len(list(csv.reader(handle, delimiter="\t"))) - 1)


def interval_union_seconds(intervals: Iterable[tuple[float, float]]) -> float:
    ordered = sorted((float(start), float(end)) for start, end in intervals if end > start)
    merged: list[list[float]] = []
    for start, end in ordered:
        if not merged or start > merged[-1][1]:
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
        "02-moss-machine-candidate.json",
        "02-whisper-draft-raw.json",
        "03-whisper-draft.json",
        "04-human-verbatim.tsv",
        "05-human-speaker-turns.tsv",
        "06-human-review.json",
        "07-term-candidates.json",
        "08-scoring-rules.json",
        "09-gap-audit.json",
        "16-pre-meeting-context.json",
        "human-review-work-in-progress.json",
        "human_review_server.py",
        "score_s8_m00_r3.py",
        "human-review.html",
        "review-config.json",
        "TASKS.md",
        "MANIFEST.json",
    ]
    missing = [name for name in required if not (package / name).is_file()]
    if missing:
        raise FileNotFoundError(f"Missing package files: {missing}")

    config = read_json(package / "review-config.json")
    audio_name = str(config["window_audio_name"])
    audio = package / audio_name
    if not audio.is_file() or Path(audio_name).name != audio_name:
        raise FileNotFoundError("Configured review audio is missing or unsafe")

    scope = read_json(package / "00-scope-and-truth-boundary.json")
    source_lock = read_json(package / "01-source-lock.json")
    machine = read_json(package / "02-moss-machine-candidate.json")
    draft_raw = read_json(package / "02-whisper-draft-raw.json")
    draft = read_json(package / "03-whisper-draft.json")
    review = read_json(package / "06-human-review.json")
    terms = read_json(package / "07-term-candidates.json")
    rules = read_json(package / "08-scoring-rules.json")
    gap = read_json(package / "09-gap-audit.json")
    context = read_json(package / "16-pre-meeting-context.json")
    wip = read_json(package / "human-review-work-in-progress.json")
    manifest = read_json(package / "MANIFEST.json")

    with wave.open(str(audio), "rb") as reader:
        duration = reader.getnframes() / reader.getframerate()
        audio_format = {
            "channels": reader.getnchannels(),
            "sample_rate_hz": reader.getframerate(),
            "sample_width_bytes": reader.getsampwidth(),
        }
    audio_hash = sha256(audio)
    segments = machine.get("segments", [])
    speech_union = interval_union_seconds(
        (float(item["clip_start_seconds"]), float(item["clip_end_seconds"]))
        for item in segments
    )
    required_speech = float(rules["annotation_validation"]["minimum_verbatim_speech_union_seconds"])

    illegal_times = [
        item.get("source_sequence_id")
        for item in segments
        if not (
            0.0 <= float(item["clip_start_seconds"])
            < float(item["clip_end_seconds"])
            <= duration + 0.001
        )
    ]
    manifest_mismatches: list[dict[str, Any]] = []
    for item in manifest.get("files", []):
        path = package / item["relative_path"]
        if not path.is_file():
            manifest_mismatches.append({"path": item["relative_path"], "reason": "missing"})
            continue
        if path.stat().st_size != item.get("bytes") or sha256(path) != item.get("sha256"):
            manifest_mismatches.append({"path": item["relative_path"], "reason": "hash_or_size"})

    prior = scope["prior_review_preserved"]
    prior_path = Path(prior["path"])
    prior_wip = prior_path / "human-review-work-in-progress.json"
    server_text = (package / "human_review_server.py").read_text(encoding="utf-8")
    scorer_text = (package / "score_s8_m00_r3.py").read_text(encoding="utf-8")
    rows = wip.get("rows", [])
    checks = {
        "required_files_present": not missing,
        "manifest_entries_match_before_audit": not manifest_mismatches,
        "source_hash_frozen": scope["source"]["sha256"] == EXPECTED_SOURCE_SHA256,
        "source_lock_matches": str(source_lock["source_audio"]["sha256"]).upper() == EXPECTED_SOURCE_SHA256,
        "audio_hash_bound_everywhere": audio_hash
        == scope["reference_window"]["audio"]["sha256"]
        == machine["reference_audio_sha256"]
        == draft_raw["reference_audio_sha256"]
        == draft["reference_audio_sha256"]
        == review["audio_sha256"]
        == rules["audio_sha256"]
        == str(config["window_audio_sha256"]).upper(),
        "duration_exact_and_policy_range": abs(duration - 180.0) <= 0.001 and 180.0 <= duration <= 300.0,
        "audio_format_exact": audio_format == {"channels": 1, "sample_rate_hz": 16000, "sample_width_bytes": 2},
        "window_is_continuous_source_slice": scope["reference_window"].get("continuous") is True
        and abs(scope["reference_window"]["source_end_seconds"] - scope["reference_window"]["source_start_seconds"] - duration) <= 0.001,
        "machine_density_precheck_passes": speech_union >= required_speech,
        "machine_density_matches_scope": abs(speech_union - float(scope["reference_window"]["machine_navigation_speech_union_seconds"])) <= 0.001,
        "machine_timestamps_legal": not illegal_times,
        "machine_is_explicitly_non_truth": machine.get("is_ground_truth") is False
        and machine.get("approved_as_ground_truth") is False
        and all(item.get("is_ground_truth") is False for item in segments),
        "anonymous_speaker_prefill_is_bounded": {item["reference_speaker_candidate"] for item in segments} == {"H01", "H02"},
        "formal_truth_files_empty": data_row_count(package / "04-human-verbatim.tsv") == 0
        and data_row_count(package / "05-human-speaker-turns.tsv") == 0,
        "human_review_not_forged": review.get("approved_as_ground_truth") is False
        and review.get("listened_from_start_to_end") is False,
        "wip_rows_all_unchecked": bool(rows) and all(item.get("human_checked") is False for item in rows),
        "wip_has_no_ground_truth_status": wip.get("status") == "WORK_IN_PROGRESS_NOT_GROUND_TRUTH",
        "context_remains_frozen_by_lili": context.get("status") == "FROZEN_BEFORE_TRANSCRIPTION"
        and context.get("confirmed_by") == "lili",
        "prior_review_preserved_by_hash": prior_wip.is_file() and sha256(prior_wip) == prior["wip_sha256"],
        "term_gate_closed": terms.get("gate", {}).get("ready") is False,
        "gap_verdict_blocked": gap.get("verdict") == "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
        "partial_overlap_fix_packaged": "only the intersections where two different speakers are actually active" in server_text
        and "cross_speaker_overlap_masks" in scorer_text,
    }
    structural_pass = all(checks.values())

    audit = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-DENSE-180S-INDEPENDENT-AUDIT",
        "structural_status": "PASS" if structural_pass else "FAIL",
        "release_status": "BLOCKED_HUMAN_TRUTH_PENDING",
        "checks": checks,
        "details": {
            "audio": {"duration_seconds": duration, "sha256": audio_hash, **audio_format},
            "machine_segment_count": len(segments),
            "machine_navigation_speech_union_seconds": speech_union,
            "minimum_required_speech_union_seconds": required_speech,
            "initial_review_rows": len(rows),
            "initial_checked_rows": sum(item.get("human_checked") is True for item in rows),
            "manifest_mismatches": manifest_mismatches,
            "illegal_machine_times": illegal_times,
        },
        "verdict": "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
        "truth_rule": "结构和语音密度送审条件已通过；没有真人逐字听审、换人确认和签字，不得计算准确率。",
    }
    audit_path = package / "10-independent-audit.json"
    audit_path.write_text(json.dumps(audit, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    files = sorted(
        path for path in package.iterdir()
        if path.is_file() and path.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    rebuilt = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-DENSE-180S",
        "status": "STRUCTURE_PASS_HUMAN_TRUTH_BLOCKED" if structural_pass else "STRUCTURE_FAIL",
        "reference_audio_sha256": audio_hash,
        "files": [file_record(path, package) for path in files],
    }
    (package / "MANIFEST.json").write_text(json.dumps(rebuilt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"structural_status": audit["structural_status"], "release_status": audit["release_status"], "checks": checks, "audit": str(audit_path)}, ensure_ascii=False))
    return 0 if structural_pass else 1


if __name__ == "__main__":
    raise SystemExit(main())
