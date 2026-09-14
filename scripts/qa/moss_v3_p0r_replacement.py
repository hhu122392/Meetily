#!/usr/bin/env python3
"""Build a replacement P0-R review package without inventing human truth.

The old 244.940-second window contains an intentionally excluded, human-
unresolvable sentence around 198 seconds.  This tool derives a 195-second
reference clip from the same frozen source, keeps machine output separate,
and leaves the formal human transcript and speaker truth empty until a real
listener completes them.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import wave
from typing import Any, Iterable


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


# The machine candidates are relative to the previously frozen review window,
# whose zero point is 415.440 seconds into the full 737.728-second recording.
# Keep these two timelines separate: using the candidate-relative zero as an
# ffmpeg seek position silently pairs the draft with the wrong audio.
CANDIDATE_START_SECONDS = 0.0
CANDIDATE_END_SECONDS = 195.0
SOURCE_AUDIO_START_SECONDS = 415.440
REFERENCE_DURATION_SECONDS = CANDIDATE_END_SECONDS - CANDIDATE_START_SECONDS
SOURCE_AUDIO_END_SECONDS = SOURCE_AUDIO_START_SECONDS + REFERENCE_DURATION_SECONDS
EXPECTED_SOURCE_SHA256 = (
    "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
)
HUMAN_ATTESTATION = (
    "我已从头到尾听完冻结音频，并按实际听到的内容和换人位置完成标注；"
    "机器草稿仅用于定位，没有被当作标准答案。"
)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def write_json(path: Path, payload: Any) -> None:
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def file_record(path: Path, root: Path) -> dict[str, Any]:
    return {
        "relative_path": path.relative_to(root).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise FileNotFoundError(f"{label} does not exist: {path}")


def clip_segments(
    segments: Iterable[dict[str, Any]],
    *,
    start_seconds: float = CANDIDATE_START_SECONDS,
    end_seconds: float = CANDIDATE_END_SECONDS,
) -> list[dict[str, Any]]:
    clipped: list[dict[str, Any]] = []
    for segment in segments:
        if "start_seconds" in segment and "end_seconds" in segment:
            time_fields = ("start_seconds", "end_seconds")
        elif "clip_start_seconds" in segment and "clip_end_seconds" in segment:
            time_fields = ("clip_start_seconds", "clip_end_seconds")
        else:
            time_fields = ("start", "end")
        start = float(segment.get(time_fields[0], 0.0))
        end = float(segment.get(time_fields[1], 0.0))
        if end <= start_seconds or start >= end_seconds:
            continue
        item = dict(segment)
        item[time_fields[0]] = max(start_seconds, start) - start_seconds
        item[time_fields[1]] = min(end_seconds, end) - start_seconds
        item["source_start_seconds"] = start
        item["source_end_seconds"] = end
        item["boundary_clipped"] = start < start_seconds or end > end_seconds
        item["eligible_as_ground_truth"] = False
        clipped.append(item)
    return clipped


def write_tsv_header(path: Path, header: list[str]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        csv.writer(handle, delimiter="\t", lineterminator="\n").writerow(header)


def wav_metadata(path: Path) -> dict[str, Any]:
    with wave.open(str(path), "rb") as handle:
        frames = handle.getnframes()
        rate = handle.getframerate()
        return {
            "channels": handle.getnchannels(),
            "sample_rate_hz": rate,
            "sample_width_bytes": handle.getsampwidth(),
            "frame_count": frames,
            "duration_seconds": frames / rate,
        }


def run_ffmpeg(ffmpeg: Path, source: Path, output: Path) -> None:
    command = [
        str(ffmpeg),
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-ss",
        f"{SOURCE_AUDIO_START_SECONDS:.3f}",
        "-t",
        f"{REFERENCE_DURATION_SECONDS:.3f}",
        "-i",
        str(source),
        "-map_metadata",
        "-1",
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-c:a",
        "pcm_s16le",
        str(output),
    ]
    completed = subprocess.run(command, text=True, capture_output=True, check=False)
    if completed.returncode != 0:
        raise RuntimeError(
            f"ffmpeg failed with exit code {completed.returncode}: {completed.stderr.strip()}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--machine-candidate", type=Path, required=True)
    parser.add_argument("--whisper-draft", type=Path, required=True)
    parser.add_argument("--speaker-anchors", type=Path, required=True)
    parser.add_argument("--term-confirmations", type=Path, required=True)
    parser.add_argument("--pre-meeting-context", type=Path, required=True)
    parser.add_argument("--ffmpeg", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()

    source = args.source.resolve()
    machine_path = args.machine_candidate.resolve()
    whisper_path = args.whisper_draft.resolve()
    anchors_path = args.speaker_anchors.resolve()
    terms_path = args.term_confirmations.resolve()
    context_path = args.pre_meeting_context.resolve()
    ffmpeg = args.ffmpeg.resolve()
    output_dir = args.output_dir.resolve()

    for path, label in [
        (source, "frozen source audio"),
        (machine_path, "MOSS machine candidate"),
        (whisper_path, "Whisper draft"),
        (anchors_path, "human speaker anchors"),
        (terms_path, "human term confirmations"),
        (context_path, "frozen pre-meeting context"),
        (ffmpeg, "frozen ffmpeg"),
    ]:
        require_file(path, label)

    source_hash = sha256(source)
    if source_hash != EXPECTED_SOURCE_SHA256:
        raise ValueError(
            "Frozen source hash mismatch: "
            f"expected {EXPECTED_SOURCE_SHA256}, got {source_hash}"
        )

    output_dir.mkdir(parents=True, exist_ok=True)
    audio_path = output_dir / "01-reference-audio-source-415440-610440ms.wav"
    run_ffmpeg(ffmpeg, source, audio_path)
    audio_meta = wav_metadata(audio_path)
    if audio_meta["channels"] != 1 or audio_meta["sample_rate_hz"] != 16000:
        raise ValueError(f"Unexpected WAV format: {audio_meta}")
    if abs(audio_meta["duration_seconds"] - 195.0) > 0.001:
        raise ValueError(f"Unexpected WAV duration: {audio_meta}")

    machine = read_json(machine_path)
    whisper = read_json(whisper_path)
    anchors = read_json(anchors_path)
    terms = read_json(terms_path)
    context = read_json(context_path)

    source_offsets = {
        round(
            float(item["source_recording_start_seconds"])
            - float(item.get("clip_start_seconds", 0.0)),
            3,
        )
        for item in whisper.get("segments", [])
        if "source_recording_start_seconds" in item
    }
    if source_offsets != {SOURCE_AUDIO_START_SECONDS}:
        raise ValueError(
            "Whisper draft does not map to the frozen source offset: "
            f"expected {SOURCE_AUDIO_START_SECONDS:.3f}, got {sorted(source_offsets)}"
        )
    machine_segments = clip_segments(machine.get("segments", []))

    whisper_segments_source = whisper.get("segments", [])
    whisper_segments = clip_segments(whisper_segments_source)

    excluded_machine_segments = [
        item
        for item in machine.get("segments", [])
        if float(item.get("start_seconds", item.get("start", 0.0))) >= 195.0
    ]
    first_excluded = excluded_machine_segments[0] if excluded_machine_segments else None

    scope = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-REPLACEMENT-195S",
        "status": "REVIEW_PACKAGE_READY_HUMAN_TRUTH_PENDING",
        "truth_boundary": (
            "本包只冻结音频、机器候选和审计规则。04/05 正式人工真值为空；"
            "未经真人逐字听写和签字，不得计算或宣称准确率通过。"
        ),
        "source": {
            "path": str(source),
            "bytes": source.stat().st_size,
            "sha256": source_hash,
            "duration_seconds": 737.728,
        },
        "reference_window": {
            "candidate_start_seconds": CANDIDATE_START_SECONDS,
            "candidate_end_seconds": CANDIDATE_END_SECONDS,
            "source_start_seconds": SOURCE_AUDIO_START_SECONDS,
            "source_end_seconds": SOURCE_AUDIO_END_SECONDS,
            "duration_seconds": REFERENCE_DURATION_SECONDS,
            "reason": (
                "保留已有人名锚点的多人真实业务对话，并在 196.14 秒机器段前截止，"
                "避开约 198 秒真人无法听清的版本号句子。"
            ),
            "audio": {
                "relative_path": audio_path.name,
                "bytes": audio_path.stat().st_size,
                "sha256": sha256(audio_path),
                **audio_meta,
            },
        },
        "known_human_evidence": {
            "reviewer_id": "lili",
            "reviewer_role": "会议记录",
            "speaker_anchor_source": file_record(anchors_path, anchors_path.parent),
            "term_confirmation_source": file_record(terms_path, terms_path.parent),
            "confirmed_names_in_window": ["MeiL", "Ben", "伊犁", "卢真", "Rayson"],
            "confirmed_business_term": "CGS",
        },
        "excluded_boundary": {
            "first_excluded_machine_segment": first_excluded,
            "unresolvable_sentence_included": False,
        },
    }
    write_json(output_dir / "00-scope-and-truth-boundary.json", scope)

    source_lock = {
        "schema_version": 1,
        "stage": "S8-M00-R4",
        "source_audio": {
            "path": str(source),
            "review_path": str(audio_path),
            "sha256": source_hash.casefold(),
            "bytes": source.stat().st_size,
            "duration_seconds": 737.728,
            "format": "pcm_s16le",
            "sample_rate_hz": 16000,
            "channels": 1,
            "review_window_source_start_seconds": SOURCE_AUDIO_START_SECONDS,
            "review_window_source_end_seconds": SOURCE_AUDIO_END_SECONDS,
        },
    }
    write_json(output_dir / "01-source-lock.json", source_lock)

    candidate = {
        "schema_version": 1,
        "role": "MACHINE_CANDIDATE_ONLY",
        "is_ground_truth": False,
        "approved_as_ground_truth": False,
        "reference_audio_sha256": sha256(audio_path),
        "source_machine_file": {
            "path": str(machine_path),
            "sha256": sha256(machine_path),
        },
        "segment_count": len(machine_segments),
        "segments": machine_segments,
    }
    write_json(output_dir / "02-moss-machine-candidate.json", candidate)

    whisper_candidate = {
        "schema_version": 1,
        "role": "WHISPER_DRAFT_ONLY",
        "is_ground_truth": False,
        "approved_as_ground_truth": False,
        "reference_audio_sha256": sha256(audio_path),
        "source_whisper_file": {
            "path": str(whisper_path),
            "sha256": sha256(whisper_path),
        },
        "segment_count": len(whisper_segments),
        "segments": whisper_segments,
    }
    write_json(output_dir / "03-whisper-draft.json", whisper_candidate)
    # Compatibility input for the existing local-only strict review server.
    # The duplicate remains explicitly non-truth and exists only so the
    # reviewer can navigate the 195-second audio without changing the server.
    write_json(output_dir / "02-whisper-draft-raw.json", whisper_candidate)

    write_tsv_header(
        output_dir / "04-human-verbatim.tsv",
        [
            "segment_id",
            "start_ms",
            "end_ms",
            "reference_speaker_id",
            "verbatim_text",
            "overlap",
            "non_speech",
            "valid_for_scoring",
            "reviewer_note",
        ],
    )
    write_tsv_header(
        output_dir / "05-human-speaker-turns.tsv",
        [
            "turn_id",
            "start_ms",
            "end_ms",
            "reference_speaker_id",
            "overlap",
            "valid_for_scoring",
            "note",
        ],
    )

    review = {
        "schema_version": 1,
        "audio_sha256": sha256(audio_path),
        "audio_duration_seconds": 195.0,
        "review_status": "PENDING_HUMAN_REVIEW",
        "reviewer_id": "lili",
        "reviewer_role": "会议记录",
        "review_started_at": None,
        "review_completed_at": None,
        "listened_from_start_to_end": False,
        "valid_verbatim_segment_count": 0,
        "valid_speaker_turn_count": 0,
        "speaker_switch_count": 0,
        "overlap_reviewed": False,
        "names_checked": False,
        "numbers_and_dates_checked": False,
        "business_terms_checked": False,
        "simplified_chinese_checked": False,
        "review_method": None,
        "review_attestation": None,
        "required_attestation": HUMAN_ATTESTATION,
        "approved_as_ground_truth": False,
        "machine_candidate_may_be_used_for_navigation_only": True,
    }
    write_json(output_dir / "06-human-review.json", review)

    term_candidates = {
        "schema_version": 1,
        "status": "ONE_HUMAN_CONFIRMED_TERM_OTHER_ITEMS_PENDING",
        "confirmed_positive_terms": [
            {
                "term": "CGS",
                "type": "business_term",
                "human_confirmed": True,
                "source": str(terms_path),
            }
        ],
        "machine_only_positive_candidates": ["PWA", "YouTube", "QL"],
        "negative_terms": [],
        "gate": {
            "minimum_positive_terms": 3,
            "minimum_negative_terms": 2,
            "ready": False,
        },
    }
    write_json(output_dir / "07-term-candidates.json", term_candidates)

    scoring_rules = {
        "schema_version": 1,
        "status": "FROZEN_FOR_195S_REFERENCE_HUMAN_TRUTH_PENDING",
        "audio_sha256": sha256(audio_path),
        "audio_duration_seconds": 195.0,
        "annotation_validation": {
            "maximum_timeline_edge_gap_seconds": 0.5,
            "maximum_unexplained_gap_seconds": 0.5,
            "maximum_total_unexplained_seconds": 1.0,
            "minimum_verbatim_speech_union_seconds": 166.0,
            "minimum_speaker_turn_union_seconds": 166.0,
            "minimum_valid_verbatim_segments": 8,
            "minimum_valid_speaker_turns": 8,
            "minimum_reference_characters": 300,
            "minimum_speaker_switches": 4,
            "minimum_reference_speakers": 2,
            "minimum_scored_seconds_per_speaker": 10.0,
            "minimum_valid_turns_per_speaker": 2,
            "minimum_primary_speakers": 2,
            "minimum_seconds_per_primary_speaker": 30.0,
            "minimum_speech_turn_alignment_ratio": 0.98,
            "maximum_speech_turn_uncovered_seconds": 1.0,
            "maximum_invalid_row_ratio": 0.02,
            "maximum_invalid_union_seconds": 1.0,
            "minimum_real_review_elapsed_seconds": 195.0,
            "required_review_method": "LISTENED_AND_TRANSCRIBED",
            "required_attestation": HUMAN_ATTESTATION,
        },
        "cer": {
            "formula": (
                "Levenshtein distance(NFKC normalized reference, hypothesis) / "
                "max(1, reference character count)"
            ),
            "moss_cer_absolute_maximum": 0.2,
            "traditional_characters_are_not_converted": True,
        },
        "speaker_scoring": {
            "gate_max_turn_error_rate": 0.1,
            "gate_max_duration_error_rate": 0.1,
            "gate_max_false_alarm_seconds": 2.0,
            "gate_max_false_alarm_rate": 0.02,
            "overlap_audio_excluded_from_denominator_but_must_be_marked": True,
        },
        "term_scoring": {
            "minimum_positive_targets": 3,
            "minimum_negative_targets": 2,
            "maximum_false_insertions": 0,
            "negative_scope": "ENTIRE_195_SECOND_REFERENCE",
        },
        "overall_rule": (
            "任一人工真值、文字、说话人、术语或结构门禁未通过，P0-R 和 L2 均不得放行。"
        ),
    }
    write_json(output_dir / "08-scoring-rules.json", scoring_rules)

    review_config = {
        "schema_version": 1,
        "stage": "S8-M00-R4",
        "window_audio_name": audio_path.name,
        "full_audio_sha256": source_hash.casefold(),
        "window_audio_sha256": sha256(audio_path).casefold(),
        "window_duration_seconds": 195.0,
        "full_duration_seconds": 737.728,
    }
    write_json(output_dir / "review-config.json", review_config)

    if context.get("status") != "FROZEN_BEFORE_TRANSCRIPTION":
        raise ValueError("Pre-meeting context must already be human-frozen")
    if str(context.get("confirmed_by", "")) != "lili":
        raise ValueError("Pre-meeting context reviewer must remain lili")
    if str(context.get("scope_audio_sha256", "")).casefold() != source_hash.casefold():
        raise ValueError("Pre-meeting context is not bound to the frozen source audio")
    write_json(output_dir / "16-pre-meeting-context.json", context)

    overlaps = [
        item.get("row")
        for item in machine_segments
        if item.get("overlaps_previous_model_segment") is True
    ]
    speaker_names = sorted(
        {
            str(item.get("speaker_candidate"))
            for item in machine_segments
            if item.get("speaker_candidate")
        }
    )
    gap_audit = {
        "schema_version": 1,
        "status": "MACHINE_DRAFT_READY_FORMAL_TRUTH_EMPTY",
        "checks": {
            "source_hash_matches": True,
            "reference_duration_is_180_to_300_seconds": 180.0 <= 195.0 <= 300.0,
            "unresolvable_198_second_sentence_excluded": first_excluded is not None
            and float(first_excluded.get("start_seconds", 0.0)) >= 195.0,
            "machine_candidate_is_not_ground_truth": True,
            "formal_verbatim_data_rows": 0,
            "formal_speaker_turn_data_rows": 0,
            "human_approval": False,
        },
        "machine_candidate": {
            "segment_count": len(machine_segments),
            "speaker_candidates": speaker_names,
            "overlap_candidate_rows": overlaps,
            "known_problem": (
                "模型标签会跨人复用，且重叠行必须由真人确认；不能全局把标签替换成人名。"
            ),
        },
        "remaining_release_blockers": [
            "195 秒独立人工逐字稿为空",
            "195 秒人工说话人边界为空",
            "重叠说话未逐处确认",
            "正向术语只有 CGS 经过人工确认，少于 3 个",
            "负向术语尚未由真人确认",
            "人工复核签字尚未完成",
        ],
        "verdict": "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
    }
    write_json(output_dir / "09-gap-audit.json", gap_audit)

    tasks = """# P0-R 195 秒替代基线任务

- [x] P0R-01 锁定原始 737.728 秒真实业务录音及 SHA-256。
- [x] P0R-02 选择原始录音 415.440–610.440 秒窗口（候选稿相对时间 0–195 秒），避开候选稿约 198 秒无法听清的版本号句子。
- [x] P0R-03 生成 16 kHz、单声道、16-bit PCM 冻结音频并校验 195.000 秒。
- [x] P0R-04 分离 MOSS 机器候选、Whisper 草稿和正式人工真值文件。
- [x] P0R-05 固定 reviewer=lili、role=会议记录，但不伪造人工签字。
- [x] P0R-06 建立文字、说话人、术语和重叠说话门禁。
- [ ] P0R-07 真人从头到尾独立听写 195 秒逐字稿。
- [ ] P0R-08 真人标注完整说话人边界和所有重叠说话。
- [ ] P0R-09 真人确认至少 3 个说出术语和 2 个未说术语。
- [ ] P0R-10 完成人工签字后运行 CER、说话人和术语正式评分。

## 当前判定

包结构已完成并审计；人工真值尚未生成，因此 P0-R、准确率和 L2 仍然阻断。
"""
    (output_dir / "TASKS.md").write_text(tasks, encoding="utf-8")

    manifest_files = sorted(
        item
        for item in output_dir.iterdir()
        if item.is_file()
        and item.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    manifest = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-REPLACEMENT-195S",
        "status": "STRUCTURE_PASS_HUMAN_TRUTH_BLOCKED",
        "reference_audio_sha256": sha256(audio_path),
        "files": [file_record(item, output_dir) for item in manifest_files],
    }
    write_json(output_dir / "MANIFEST.json", manifest)

    print(
        json.dumps(
            {
                "status": manifest["status"],
                "output_dir": str(output_dir),
                "audio_sha256": manifest["reference_audio_sha256"],
                "machine_segments": len(machine_segments),
                "whisper_segments": len(whisper_segments),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
