#!/usr/bin/env python3
"""Build a dense, continuous P0-R human-review window.

The previously reviewed 195 second window contains only 154.77 seconds of
speech.  The frozen production policy requires at least 85% speech in a
180-300 second continuous window, so that window cannot pass without weakening
the rule or falsifying silence as speech.  This builder selects the real source
recording from 116.810 to 296.810 seconds: a continuous 180 second interval
whose machine navigation draft covers 173.651 seconds.  Machine text and
speaker labels remain explicitly non-truth until the reviewer listens and
attests.
"""

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


SOURCE_START_SECONDS = 116.810
WINDOW_DURATION_SECONDS = 180.0
SOURCE_END_SECONDS = SOURCE_START_SECONDS + WINDOW_DURATION_SECONDS
FULL_DURATION_SECONDS = 737.728
EXPECTED_SOURCE_SHA256 = (
    "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
)
MINIMUM_SPEECH_SECONDS = WINDOW_DURATION_SECONDS * 0.85
HUMAN_ATTESTATION = (
    "我已从头到尾听完冻结音频，并按实际听到的内容和换人位置完成标注；"
    "机器草稿仅用于定位，没有被当作标准答案。"
)

# The source model resets labels at chunk boundaries.  Dialogue continuity
# suggests two anonymous voices across the boundary, but this mapping is only a
# navigation candidate and is deliberately left unchecked for the human.
MODEL_TO_REVIEW_SPEAKER = {
    "C001:S03": "H01",
    "C002:S01": "H01",
    "C001:S02": "H02",
    "C002:S02": "H02",
}


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


def interval_union_seconds(intervals: Iterable[tuple[float, float]]) -> float:
    ordered = sorted((float(start), float(end)) for start, end in intervals if end > start)
    merged: list[list[float]] = []
    for start, end in ordered:
        if not merged or start > merged[-1][1]:
            merged.append([start, end])
        else:
            merged[-1][1] = max(merged[-1][1], end)
    return sum(end - start for start, end in merged)


def extract_pcm_window(source: Path, output: Path) -> dict[str, Any]:
    with wave.open(str(source), "rb") as reader:
        channels = reader.getnchannels()
        sample_width = reader.getsampwidth()
        sample_rate = reader.getframerate()
        compression = reader.getcomptype()
        if (channels, sample_width, sample_rate, compression) != (1, 2, 16000, "NONE"):
            raise ValueError(
                "Frozen source must be mono 16 kHz 16-bit PCM WAV; got "
                f"channels={channels}, width={sample_width}, rate={sample_rate}, compression={compression}"
            )
        start_frame = round(SOURCE_START_SECONDS * sample_rate)
        frame_count = round(WINDOW_DURATION_SECONDS * sample_rate)
        reader.setpos(start_frame)
        frames = reader.readframes(frame_count)
        if len(frames) != frame_count * channels * sample_width:
            raise ValueError("Frozen source ended before the selected review window")
    with wave.open(str(output), "wb") as writer:
        writer.setnchannels(channels)
        writer.setsampwidth(sample_width)
        writer.setframerate(sample_rate)
        writer.writeframes(frames)
    return {
        "channels": channels,
        "sample_rate_hz": sample_rate,
        "sample_width_bytes": sample_width,
        "frame_count": frame_count,
        "duration_seconds": frame_count / sample_rate,
    }


def clip_machine_segments(segments: list[dict[str, Any]]) -> list[dict[str, Any]]:
    clipped: list[dict[str, Any]] = []
    for source in segments:
        start = float(source["clip_start_seconds"])
        end = float(source["clip_end_seconds"])
        if end <= SOURCE_START_SECONDS or start >= SOURCE_END_SECONDS:
            continue
        clipped_start = max(start, SOURCE_START_SECONDS)
        clipped_end = min(end, SOURCE_END_SECONDS)
        model_label = str(source.get("model_speaker_label", ""))
        if model_label not in MODEL_TO_REVIEW_SPEAKER:
            raise ValueError(f"Unmapped model label in dense window: {model_label}")
        boundary_start = start < SOURCE_START_SECONDS
        boundary_end = end > SOURCE_END_SECONDS
        text = str(source.get("review_prefill_text") or source.get("text") or "")
        item = dict(source)
        item.update(
            {
                "clip_start_seconds": round(clipped_start - SOURCE_START_SECONDS, 3),
                "clip_end_seconds": round(clipped_end - SOURCE_START_SECONDS, 3),
                "source_recording_start_seconds": start,
                "source_recording_end_seconds": end,
                "boundary_clipped_start": boundary_start,
                "boundary_clipped_end": boundary_end,
                "reference_speaker_candidate": MODEL_TO_REVIEW_SPEAKER[model_label],
                "review_prefill_text": "" if boundary_start or boundary_end else text,
                "human_checked": False,
                "is_ground_truth": False,
                "eligible_as_ground_truth": False,
            }
        )
        clipped.append(item)
    return sorted(clipped, key=lambda item: (item["clip_start_seconds"], item["clip_end_seconds"]))


def build_initial_rows(segments: list[dict[str, Any]]) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    cursor = 0.0
    for index, segment in enumerate(segments, 1):
        start = float(segment["clip_start_seconds"])
        end = float(segment["clip_end_seconds"])
        if start > cursor + 0.005:
            rows.append(
                {
                    "start": round(cursor, 3),
                    "end": round(start, 3),
                    "speaker": "",
                    "text": "[无清晰语音]",
                    "overlap": False,
                    "non_speech": True,
                    "note": "机器候选未覆盖；必须播放确认是否确实没有清晰语音。",
                    "human_checked": False,
                    "machine_source": "gap:dense-window",
                }
            )
        clipped = segment.get("boundary_clipped_start") is True or segment.get("boundary_clipped_end") is True
        rows.append(
            {
                "start": round(start, 3),
                "end": round(end, 3),
                "speaker": str(segment["reference_speaker_candidate"]),
                "text": str(segment.get("review_prefill_text", "")),
                "overlap": False,
                "non_speech": False,
                "note": (
                    "窗口边界从机器段中间截断，禁止预填整段文字；必须只听写窗口内实际内容。"
                    if clipped
                    else "机器候选仅用于定位；必须逐字听审并核对换人。"
                ),
                "human_checked": False,
                "machine_source": f"moss-dense:{segment.get('source_sequence_id', index)}",
            }
        )
        cursor = max(cursor, end)
    if cursor < WINDOW_DURATION_SECONDS - 0.005:
        rows.append(
            {
                "start": round(cursor, 3),
                "end": WINDOW_DURATION_SECONDS,
                "speaker": "",
                "text": "[无清晰语音]",
                "overlap": False,
                "non_speech": True,
                "note": "机器候选未覆盖尾部；必须播放确认。",
                "human_checked": False,
                "machine_source": "gap:dense-window-tail",
            }
        )

    speech_rows = [row for row in rows if not row["non_speech"]]
    for previous_index, previous in enumerate(speech_rows):
        for current in speech_rows[previous_index + 1 :]:
            if current["start"] >= previous["end"]:
                break
            if previous["speaker"] != current["speaker"]:
                previous["overlap"] = True
                current["overlap"] = True
    return sorted(rows, key=lambda row: (row["start"], row["end"]))


def write_tsv_header(path: Path, header: list[str]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        csv.writer(handle, delimiter="\t", lineterminator="\n").writerow(header)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--full-machine-candidate", type=Path, required=True)
    parser.add_argument("--pre-meeting-context", type=Path, required=True)
    parser.add_argument("--review-kit", type=Path, required=True)
    parser.add_argument("--prior-review-package", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()

    source = args.source.resolve(strict=True)
    machine_path = args.full_machine_candidate.resolve(strict=True)
    context_path = args.pre_meeting_context.resolve(strict=True)
    review_kit = args.review_kit.resolve(strict=True)
    prior_package = args.prior_review_package.resolve(strict=True)
    output = args.output_dir.resolve()
    if output.exists() and any(output.iterdir()):
        raise FileExistsError(f"Output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)

    source_hash = sha256(source)
    if source_hash != EXPECTED_SOURCE_SHA256:
        raise ValueError(f"Frozen source hash mismatch: {source_hash}")

    audio_name = "01-reference-audio-source-116810-296810ms.wav"
    audio_path = output / audio_name
    audio_metadata = extract_pcm_window(source, audio_path)
    audio_hash = sha256(audio_path)

    machine = read_json(machine_path)
    segments = clip_machine_segments(machine.get("segments", []))
    machine_speech_seconds = interval_union_seconds(
        (item["clip_start_seconds"], item["clip_end_seconds"]) for item in segments
    )
    if machine_speech_seconds < MINIMUM_SPEECH_SECONDS:
        raise ValueError(
            f"Dense window precheck failed: {machine_speech_seconds:.3f} < {MINIMUM_SPEECH_SECONDS:.3f}"
        )
    rows = build_initial_rows(segments)

    scope = {
        "schema_version": 1,
        "stage": "MOSS-V3-P0R-DENSE-180S",
        "status": "REVIEW_PACKAGE_READY_HUMAN_TRUTH_PENDING",
        "truth_boundary": (
            "音频是真实连续录音；机器文字和匿名说话人只用于导航。"
            "正式人工文件保持为空，必须由真人完整听审后生成。"
        ),
        "source": {
            "path": str(source),
            "bytes": source.stat().st_size,
            "sha256": source_hash,
            "duration_seconds": FULL_DURATION_SECONDS,
        },
        "reference_window": {
            "source_start_seconds": SOURCE_START_SECONDS,
            "source_end_seconds": SOURCE_END_SECONDS,
            "duration_seconds": WINDOW_DURATION_SECONDS,
            "continuous": True,
            "machine_navigation_speech_union_seconds": round(machine_speech_seconds, 3),
            "minimum_required_speech_union_seconds": MINIMUM_SPEECH_SECONDS,
            "selection_reason": (
                "原195秒窗口只有154.77秒语音，低于85%冻结门槛且无法通过连续扩展修复；"
                "本窗口为连续180秒，机器导航段覆盖173.651秒，先满足送审资格，最终仍以真人标注为准。"
            ),
            "audio": {
                "relative_path": audio_name,
                "bytes": audio_path.stat().st_size,
                "sha256": audio_hash,
                **audio_metadata,
            },
        },
        "prior_review_preserved": {
            "path": str(prior_package),
            "wip_sha256": sha256(prior_package / "human-review-work-in-progress.json"),
            "reuse_as_truth": False,
            "reason": "旧窗口与新窗口不相同，保留为审计证据但不冒充新窗口人工真值。",
        },
    }
    write_json(output / "00-scope-and-truth-boundary.json", scope)

    write_json(
        output / "01-source-lock.json",
        {
            "schema_version": 1,
            "stage": "S8-M00-R5",
            "source_audio": {
                "path": str(source),
                "review_path": str(audio_path),
                "sha256": source_hash.casefold(),
                "bytes": source.stat().st_size,
                "duration_seconds": FULL_DURATION_SECONDS,
                "format": "pcm_s16le",
                "sample_rate_hz": 16000,
                "channels": 1,
                "review_window_source_start_seconds": SOURCE_START_SECONDS,
                "review_window_source_end_seconds": SOURCE_END_SECONDS,
            },
        },
    )

    candidate = {
        "schema_version": 1,
        "role": "MACHINE_CANDIDATE_ONLY",
        "is_ground_truth": False,
        "approved_as_ground_truth": False,
        "reference_audio_sha256": audio_hash,
        "source_machine_file": {"path": str(machine_path), "sha256": sha256(machine_path)},
        "segment_count": len(segments),
        "machine_navigation_speech_union_seconds": round(machine_speech_seconds, 3),
        "speaker_mapping_status": "ANONYMOUS_MACHINE_PREFILL_REQUIRES_HUMAN_CONFIRMATION",
        "segments": segments,
    }
    write_json(output / "02-moss-machine-candidate.json", candidate)
    write_json(output / "02-whisper-draft-raw.json", candidate)
    write_json(output / "03-whisper-draft.json", candidate)

    write_tsv_header(
        output / "04-human-verbatim.tsv",
        ["segment_id", "start_ms", "end_ms", "reference_speaker_id", "verbatim_text", "overlap", "non_speech", "valid_for_scoring", "reviewer_note"],
    )
    write_tsv_header(
        output / "05-human-speaker-turns.tsv",
        ["turn_id", "start_ms", "end_ms", "reference_speaker_id", "overlap", "valid_for_scoring", "note"],
    )
    write_json(
        output / "06-human-review.json",
        {
            "schema_version": 1,
            "audio_sha256": audio_hash,
            "audio_duration_seconds": WINDOW_DURATION_SECONDS,
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
        },
    )

    write_json(
        output / "07-term-candidates.json",
        {
            "schema_version": 1,
            "status": "MACHINE_CANDIDATES_PENDING_WINDOW_HUMAN_CONFIRMATION",
            "confirmed_positive_terms": [],
            "machine_only_positive_candidates": ["PWA", "YouTube", "AI", "H5", "CGS"],
            "negative_terms": [],
            "gate": {"minimum_positive_terms": 3, "minimum_negative_terms": 2, "ready": False},
            "truth_boundary": "旧窗口的术语确认不证明新窗口说出了同一术语，必须在本窗口重新听审。",
        },
    )

    base_rules = read_json(review_kit / "08-scoring-rules.json")
    rules = dict(base_rules)
    rules.update(
        {
            "schema_version": 1,
            "status": "FROZEN_FOR_DENSE_180S_REFERENCE_HUMAN_TRUTH_PENDING",
            "audio_sha256": audio_hash,
            "audio_duration_seconds": WINDOW_DURATION_SECONDS,
        }
    )
    annotation = dict(base_rules["annotation_validation"])
    annotation.update(
        {
            "minimum_verbatim_speech_union_seconds": MINIMUM_SPEECH_SECONDS,
            "minimum_speaker_turn_union_seconds": MINIMUM_SPEECH_SECONDS,
            "minimum_real_review_elapsed_seconds": WINDOW_DURATION_SECONDS,
            "required_attestation": HUMAN_ATTESTATION,
        }
    )
    rules["annotation_validation"] = annotation
    if "term_scoring" in rules:
        rules["term_scoring"] = dict(rules["term_scoring"])
        rules["term_scoring"]["negative_scope"] = "ENTIRE_180_SECOND_REFERENCE"
    write_json(output / "08-scoring-rules.json", rules)

    context = read_json(context_path)
    if context.get("status") != "FROZEN_BEFORE_TRANSCRIPTION" or context.get("confirmed_by") != "lili":
        raise ValueError("Pre-meeting context must remain frozen by lili")
    if str(context.get("scope_audio_sha256", "")).casefold() != source_hash.casefold():
        raise ValueError("Pre-meeting context is not bound to the frozen full source")
    write_json(output / "16-pre-meeting-context.json", context)

    write_json(
        output / "09-gap-audit.json",
        {
            "schema_version": 1,
            "status": "DENSE_MACHINE_DRAFT_READY_FORMAL_TRUTH_EMPTY",
            "checks": {
                "source_hash_matches": True,
                "reference_duration_is_180_to_300_seconds": True,
                "reference_window_is_continuous": True,
                "machine_navigation_speech_union_seconds": round(machine_speech_seconds, 3),
                "minimum_required_speech_union_seconds": MINIMUM_SPEECH_SECONDS,
                "machine_density_precheck_passes": machine_speech_seconds >= MINIMUM_SPEECH_SECONDS,
                "machine_candidate_is_not_ground_truth": True,
                "formal_verbatim_data_rows": 0,
                "formal_speaker_turn_data_rows": 0,
                "human_approval": False,
            },
            "remaining_release_blockers": [
                "180秒连续音频尚未由真人1倍速从头听到尾",
                "18行机器预填尚未逐行核对正文和换人",
                "抢话、数字、日期和业务术语尚未由真人确认",
                "正式人工逐字稿和说话人边界仍为空",
                "人工复核签字尚未完成",
            ],
            "verdict": "P0-R BLOCKED / ACCURACY NOT_SCORABLE / L2 BLOCKED",
        },
    )

    write_json(
        output / "review-config.json",
        {
            "schema_version": 1,
            "stage": "S8-M00-R5",
            "window_audio_name": audio_name,
            "full_audio_sha256": source_hash.casefold(),
            "window_audio_sha256": audio_hash.casefold(),
            "window_duration_seconds": WINDOW_DURATION_SECONDS,
            "full_duration_seconds": FULL_DURATION_SECONDS,
        },
    )
    write_json(
        output / "human-review-work-in-progress.json",
        {
            "schema_version": 1,
            "status": "WORK_IN_PROGRESS_NOT_GROUND_TRUTH",
            "saved_at": None,
            "reviewer_id": "lili",
            "reviewer_role": "会议记录",
            "rows": rows,
            "window_progress": {
                "duration_seconds": WINDOW_DURATION_SECONDS,
                "covered_seconds": 0.0,
                "coverage_percent": 0.0,
                "first_covered_seconds": None,
                "last_covered_seconds": None,
                "maximum_gap_seconds": WINDOW_DURATION_SECONDS,
                "complete": False,
                "review_started_at": None,
                "wall_elapsed_seconds": 0.0,
            },
            "truth_boundary": "机器预填的可恢复草稿，不是人工真值，不参与评分。",
        },
    )

    for name in ("human_review_server.py", "score_s8_m00_r3.py", "human-review.html"):
        (output / name).write_bytes((review_kit / name).read_bytes())

    tasks = """# P0-R 高语音密度连续窗口任务

- [x] DENSE-01 证明原195秒窗口仅154.77秒语音，不能满足冻结的85%门槛。
- [x] DENSE-02 从同一冻结原录音选择116.810–296.810秒连续窗口。
- [x] DENSE-03 锁定180秒PCM音频及SHA-256。
- [x] DENSE-04 机器导航段语音覆盖预检达到173.651秒，超过153秒送审门槛。
- [x] DENSE-05 修正局部重叠只排除真实交叉区间，不再丢弃整行。
- [ ] DENSE-06 真人以1倍速连续听完180秒，逐行核对正文和H01/H02换人。
- [ ] DENSE-07 真人确认抢话、数字、日期、业务术语和简体中文。
- [ ] DENSE-08 正式封存后运行生产结构校验和独立审计。
- [ ] DENSE-09 结构通过后运行MOSS/Whisper正式准确率评分。

旧195秒草稿完整保留，只作为此前工作的审计证据；因窗口不同，不冒充新窗口真值。
"""
    (output / "TASKS.md").write_text(tasks, encoding="utf-8")

    manifest_files = sorted(
        item for item in output.iterdir()
        if item.is_file() and item.name not in {"MANIFEST.json", "human-review-work-in-progress.json"}
    )
    write_json(
        output / "MANIFEST.json",
        {
            "schema_version": 1,
            "stage": "MOSS-V3-P0R-DENSE-180S",
            "status": "STRUCTURE_READY_HUMAN_TRUTH_PENDING",
            "reference_audio_sha256": audio_hash,
            "files": [file_record(item, output) for item in manifest_files],
        },
    )

    print(
        json.dumps(
            {
                "status": "STRUCTURE_READY_HUMAN_TRUTH_PENDING",
                "output_dir": str(output),
                "audio_sha256": audio_hash,
                "audio_duration_seconds": WINDOW_DURATION_SECONDS,
                "machine_navigation_speech_union_seconds": round(machine_speech_seconds, 3),
                "minimum_required_seconds": MINIMUM_SPEECH_SECONDS,
                "initial_rows": len(rows),
                "initial_checked_rows": sum(row["human_checked"] for row in rows),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
