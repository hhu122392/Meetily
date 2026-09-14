#!/usr/bin/env python3
"""Strict chunked MOSS V3/P1 runner for frozen long-form samples.

This runner deliberately delegates the frozen model/runtime/device/audio checks,
raw-turn parser, activity probe, and evidence primitives to
``moss_v3_p1_run.py``.  It adds one safety property: no inference session is
allowed to own more than a 360 second slice of the locked 16 kHz mono WAV.
This host completed a 368.864 second slice but the native process failed fast
on a 450 second slice, so 360 seconds is the frozen upper bound with a small
measured margin.

Speaker labels are scoped to a chunk.  ``C001:S01`` and ``C002:S01`` are not a
claim that the same person spoke in both chunks.
"""

from __future__ import annotations

import argparse
import dataclasses
import gc
import hashlib
import importlib.util
import json
import math
import os
import re
import subprocess
import sys
import time
import traceback
import wave
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence


SCRIPT_PATH = Path(__file__).resolve()
SCRIPT_DIR = SCRIPT_PATH.parent
BASE_RUNNER_PATH = SCRIPT_DIR / "moss_v3_p1_run.py"
TEST_PATH = SCRIPT_DIR / "test_moss_v3_p1_chunked.py"


def _load_base_runner() -> Any:
    spec = importlib.util.spec_from_file_location("moss_v3_p1_run_base", BASE_RUNNER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("base_runner_import_spec_unavailable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


base = _load_base_runner()
GateError = base.GateError

DEFAULT_SAMPLE_NAME = "long_3096s"
ALLOWED_CHUNKED_SAMPLE_NAMES = frozenset(
    {"business_737s", "monthly_900s", "long_3096s"}
)
MIN_CHUNK_SECONDS = 300.0
MAX_CHUNK_SECONDS = 360.0
MIN_BOUNDARY_SILENCE_SECONDS = 2.0
CHUNK_N_CTX = 32_768
CHUNK_N_THREADS = 0
CHUNK_KV_TYPE = "auto"
SYSTEM_MEMORY_MARGIN_BYTES = 2 * 1024**3
DEVICE_MEMORY_MARGIN_BYTES = 512 * 1024**2
BOUNDARY_TOLERANCE_SECONDS = 1.0 / 16_000.0 + 1e-9
PUBLIC_ABSOLUTE_PATH_RE = re.compile(r"(?i)(?:^|[\s\"'])([a-z]:[\\/][^\s\"']*)")


def _as_float(value: Any, name: str) -> float:
    try:
        number = float(value)
    except (TypeError, ValueError) as exc:
        raise GateError(f"{name}_not_numeric") from exc
    if not math.isfinite(number):
        raise GateError(f"{name}_not_finite")
    return number


def _silence_record(item: Mapping[str, Any]) -> tuple[float, float, float]:
    start = _as_float(item.get("start_seconds"), "silence_start")
    end = _as_float(item.get("end_seconds"), "silence_end")
    duration = _as_float(item.get("duration_seconds", end - start), "silence_duration")
    if start < 0.0 or end <= start or duration <= 0.0:
        raise GateError("invalid_silence_interval")
    if abs((end - start) - duration) > 0.1:
        raise GateError("inconsistent_silence_interval")
    return start, end, duration


def effective_min_chunk_seconds(duration_seconds: float) -> float:
    """Keep chunks balanced when a 7.5-15 minute input needs two sessions."""

    duration = _as_float(duration_seconds, "audio_duration")
    if duration <= 0.0:
        raise GateError("invalid_audio_duration_for_chunk_limit")
    chunk_count = max(1, int(math.ceil(duration / MAX_CHUNK_SECONDS)))
    return min(MIN_CHUNK_SECONDS, duration / chunk_count)


def choose_chunk_plan(
    duration_seconds: float,
    silence_intervals: Sequence[Mapping[str, Any]],
    *,
    min_chunk_seconds: float = MIN_CHUNK_SECONDS,
    max_chunk_seconds: float = MAX_CHUNK_SECONDS,
    min_silence_seconds: float = MIN_BOUNDARY_SILENCE_SECONDS,
) -> list[dict[str, Any]]:
    """Build a deterministic, balanced plan and prefer long-silence centres.

    The smallest feasible number of chunks is selected first.  Every interior
    boundary has a lower and upper bound derived from all remaining chunks, so
    selecting a good boundary now cannot strand an undersized final slice.
    """

    duration = _as_float(duration_seconds, "audio_duration")
    minimum = _as_float(min_chunk_seconds, "minimum_chunk_duration")
    maximum = _as_float(max_chunk_seconds, "maximum_chunk_duration")
    minimum_silence = _as_float(min_silence_seconds, "minimum_boundary_silence")
    if duration <= 0.0 or minimum <= 0.0 or maximum < minimum or minimum_silence <= 0.0:
        raise GateError("invalid_chunk_plan_limits")

    if duration <= maximum:
        chunk_count = 1
    else:
        chunk_count = int(math.ceil(duration / maximum))
        if duration / chunk_count < minimum:
            raise GateError("audio_duration_cannot_be_partitioned_with_locked_chunk_limits")

    silences: list[dict[str, float]] = []
    for raw in silence_intervals:
        start, end, silence_duration = _silence_record(raw)
        if silence_duration + 1e-9 < minimum_silence:
            continue
        centre = (start + end) / 2.0
        if 0.0 < centre < duration:
            silences.append(
                {
                    "start_seconds": start,
                    "end_seconds": end,
                    "duration_seconds": silence_duration,
                    "centre_seconds": centre,
                }
            )

    boundaries = [0.0]
    boundary_details: list[dict[str, Any]] = []
    current = 0.0
    for boundary_index in range(1, chunk_count):
        chunks_left = chunk_count - boundary_index
        lower = max(current + minimum, duration - chunks_left * maximum)
        upper = min(current + maximum, duration - chunks_left * minimum)
        if lower > upper + 1e-7:
            raise GateError("chunk_boundary_feasibility_window_empty")

        balanced_target = duration * boundary_index / chunk_count
        target = min(max(balanced_target, lower), upper)
        candidates = [
            silence
            for silence in silences
            if lower - 1e-9 <= silence["centre_seconds"] <= upper + 1e-9
        ]
        if candidates:
            selected = min(
                candidates,
                key=lambda item: (
                    abs(item["centre_seconds"] - target),
                    -item["duration_seconds"],
                    item["centre_seconds"],
                ),
            )
            boundary = selected["centre_seconds"]
            detail: dict[str, Any] = {
                "source": "silence_center",
                "target_seconds": target,
                "window_start_seconds": lower,
                "window_end_seconds": upper,
                "silence": dict(selected),
            }
        else:
            boundary = target
            detail = {
                "source": "fixed_balanced",
                "target_seconds": target,
                "window_start_seconds": lower,
                "window_end_seconds": upper,
                "silence": None,
            }
        if boundary <= current or boundary >= duration:
            raise GateError("invalid_selected_chunk_boundary")
        boundaries.append(boundary)
        boundary_details.append(detail)
        current = boundary

    boundaries.append(duration)
    plan: list[dict[str, Any]] = []
    for index in range(chunk_count):
        start = boundaries[index]
        end = boundaries[index + 1]
        chunk_duration = end - start
        if chunk_count > 1 and not (
            minimum - 1e-7 <= chunk_duration <= maximum + 1e-7
        ):
            raise GateError("selected_chunk_duration_outside_locked_range")
        end_boundary = (
            boundary_details[index]
            if index < len(boundary_details)
            else {
                "source": "audio_end",
                "target_seconds": duration,
                "window_start_seconds": duration,
                "window_end_seconds": duration,
                "silence": None,
            }
        )
        plan.append(
            {
                "chunk_index": index + 1,
                "start_seconds": start,
                "end_seconds": end,
                "duration_seconds": chunk_duration,
                "end_boundary": end_boundary,
            }
        )
    return plan


def quantize_chunk_plan(
    plan: Sequence[Mapping[str, Any]], total_frames: int, sample_rate: int
) -> list[dict[str, Any]]:
    if total_frames <= 0 or sample_rate != 16_000 or not plan:
        raise GateError("invalid_chunk_plan_quantization_input")
    boundary_frames = [0]
    for item in plan[:-1]:
        boundary_frames.append(int(round(_as_float(item["end_seconds"], "chunk_end") * sample_rate)))
    boundary_frames.append(total_frames)
    if boundary_frames != sorted(boundary_frames) or len(set(boundary_frames)) != len(boundary_frames):
        raise GateError("quantized_chunk_boundaries_not_strictly_increasing")

    quantized: list[dict[str, Any]] = []
    for index, raw in enumerate(plan):
        start_frame = boundary_frames[index]
        end_frame = boundary_frames[index + 1]
        if start_frame < 0 or end_frame > total_frames or end_frame <= start_frame:
            raise GateError("quantized_chunk_frame_range_invalid")
        item = dict(raw)
        item.update(
            {
                "chunk_index": index + 1,
                "start_frame": start_frame,
                "end_frame": end_frame,
                "frame_count": end_frame - start_frame,
                "start_seconds": start_frame / sample_rate,
                "end_seconds": end_frame / sample_rate,
                "duration_seconds": (end_frame - start_frame) / sample_rate,
            }
        )
        quantized.append(item)
    return quantized


def _canonical_json_bytes(payload: Any) -> bytes:
    return json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )


def _chunk_identity(
    prepared_record: Mapping[str, Any],
    plan: Sequence[Mapping[str, Any]],
    sample_name: str = DEFAULT_SAMPLE_NAME,
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "sample_name": sample_name,
        "prepared_audio": {
            "bytes": int(prepared_record["bytes"]),
            "sha256": str(prepared_record["sha256"]),
            "sample_rate_hz": int(prepared_record["sample_rate_hz"]),
            "channels": int(prepared_record["channels"]),
            "sample_width_bytes": int(prepared_record["sample_width_bytes"]),
            "frames": int(prepared_record["frames"]),
        },
        "chunk_limits_seconds": {
            "minimum": effective_min_chunk_seconds(
                _as_float(prepared_record["duration_seconds"], "prepared_duration")
            ),
            "maximum": MAX_CHUNK_SECONDS,
            "minimum_boundary_silence": MIN_BOUNDARY_SILENCE_SECONDS,
        },
        "chunks": [
            {
                "chunk_index": int(item["chunk_index"]),
                "start_frame": int(item["start_frame"]),
                "end_frame": int(item["end_frame"]),
                "end_boundary_source": str(item["end_boundary"]["source"]),
            }
            for item in plan
        ],
    }


def _write_wave_slice(
    source_path: Path, target_path: Path, start_frame: int, frame_count: int
) -> None:
    temp_path = target_path.with_name(
        f".{target_path.name}.{os.getpid()}.{time.time_ns()}.tmp"
    )
    try:
        with wave.open(str(source_path), "rb") as source:
            if (
                source.getframerate() != 16_000
                or source.getnchannels() != 1
                or source.getsampwidth() != 2
                or source.getcomptype() != "NONE"
            ):
                raise GateError("prepared_wave_format_changed_before_chunking")
            if start_frame < 0 or start_frame + frame_count > source.getnframes():
                raise GateError("wave_slice_outside_prepared_audio")
            source.setpos(start_frame)
            frames = source.readframes(frame_count)
        if len(frames) != frame_count * 2:
            raise GateError("wave_slice_read_length_mismatch")
        with wave.open(str(temp_path), "wb") as target:
            target.setnchannels(1)
            target.setsampwidth(2)
            target.setframerate(16_000)
            target.setcomptype("NONE", "not compressed")
            target.writeframes(frames)
        try:
            temp_path.rename(target_path)
        except FileExistsError:
            temp_path.unlink(missing_ok=True)
    finally:
        temp_path.unlink(missing_ok=True)


def _wave_pcm_s16le_sha256(path: Path) -> str:
    with wave.open(str(path), "rb") as handle:
        frames = handle.readframes(handle.getnframes())
    return hashlib.sha256(frames).hexdigest().upper()


def materialize_chunks(
    prepared_record: Mapping[str, Any],
    plan: Sequence[Mapping[str, Any]],
    sample_name: str = DEFAULT_SAMPLE_NAME,
) -> dict[str, Any]:
    prepared_path = Path(str(prepared_record["path"]))
    identity = _chunk_identity(prepared_record, plan, sample_name)
    identity_sha256 = hashlib.sha256(_canonical_json_bytes(identity)).hexdigest().upper()
    root = base.INPUT_ROOT / (
        f"{sample_name}-chunks-{str(prepared_record['sha256'])[:16]}-{identity_sha256[:16]}"
    )
    root.mkdir(parents=True, exist_ok=True)
    manifest_path = root / "manifest.json"

    records: list[dict[str, Any]] = []
    expected_names = {"manifest.json"}
    for item in plan:
        index = int(item["chunk_index"])
        start_frame = int(item["start_frame"])
        end_frame = int(item["end_frame"])
        file_name = f"chunk-{index:03d}-{start_frame:010d}-{end_frame:010d}.wav"
        expected_names.add(file_name)
        path = root / file_name
        if not path.exists():
            _write_wave_slice(prepared_path, path, start_frame, end_frame - start_frame)
        record = base.wav_record(path)
        if (
            record["sample_rate_hz"] != 16_000
            or record["channels"] != 1
            or record["sample_width_bytes"] != 2
            or record["format"] != "pcm_s16le"
            or record["frames"] != end_frame - start_frame
        ):
            raise GateError("materialized_chunk_wave_invariant_failed")
        record.update(
            {
                "chunk_index": index,
                "start_frame": start_frame,
                "end_frame": end_frame,
                "global_start_seconds": start_frame / 16_000.0,
                "global_end_seconds": end_frame / 16_000.0,
                "pcm_s16le_sha256": _wave_pcm_s16le_sha256(path),
            }
        )
        records.append(record)

    manifest = {
        "schema_version": 1,
        "identity_sha256": identity_sha256,
        "identity": identity,
        "chunks": [
            {
                key: value
                for key, value in record.items()
                if key != "path"
            }
            for record in records
        ],
    }
    if manifest_path.exists():
        existing = base.read_json(manifest_path, "locked chunk manifest")
        if existing != manifest:
            raise GateError("existing_chunk_manifest_mismatch")
    else:
        base.atomic_write_text_exclusive(
            manifest_path, json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
        )

    unexpected = sorted(path.name for path in root.iterdir() if path.name not in expected_names)
    if unexpected:
        raise GateError("unexpected_file_in_locked_chunk_directory")
    return {
        "identity_sha256": identity_sha256,
        "path": str(root.resolve()),
        "manifest": base.file_record(manifest_path),
        "chunks": records,
    }


def scoped_speaker_label(chunk_index: int, local_label: str) -> str:
    if chunk_index <= 0 or not isinstance(local_label, str) or not local_label:
        raise GateError("invalid_chunk_speaker_label")
    return f"C{chunk_index:03d}:{local_label}"


def offset_chunk_output(
    output: Mapping[str, Any], chunk_index: int, offset_seconds: float
) -> list[dict[str, Any]]:
    offset = _as_float(offset_seconds, "chunk_offset")
    if chunk_index <= 0 or offset < 0.0:
        raise GateError("invalid_chunk_output_offset")
    raw_turns = output.get("raw_turns")
    if not isinstance(raw_turns, list) or not raw_turns:
        raise GateError("chunk_output_has_no_strict_raw_turns")
    global_turns: list[dict[str, Any]] = []
    previous_start_ms = -1
    for turn_index, turn in enumerate(raw_turns, start=1):
        local_start_ms = int(round(_as_float(turn["start_seconds"], "turn_start") * 1000.0))
        local_end_ms = int(round(_as_float(turn["end_seconds"], "turn_end") * 1000.0))
        if local_start_ms < previous_start_ms or local_end_ms < local_start_ms:
            raise GateError("chunk_raw_turn_timestamps_not_monotonic")
        previous_start_ms = local_start_ms
        local_label = str(turn["speaker_label"])
        global_turns.append(
            {
                "chunk_index": chunk_index,
                "turn_index": turn_index,
                "local_start_ms": local_start_ms,
                "local_end_ms": local_end_ms,
                "global_start_ms": int(round(offset * 1000.0)) + local_start_ms,
                "global_end_ms": int(round(offset * 1000.0)) + local_end_ms,
                "local_speaker_label": local_label,
                "speaker_label": scoped_speaker_label(chunk_index, local_label),
                "speaker_identity_scope": "chunk_only",
                "text": str(turn["text"]),
            }
        )
    return global_turns


def _active_intervals(activity: Mapping[str, Any]) -> list[Mapping[str, Any]]:
    intervals = activity.get("significant_active_intervals")
    if not isinstance(intervals, list) or not intervals:
        raise GateError("audio_activity_has_no_significant_intervals")
    return intervals


def validate_global_output(
    chunk_records: Sequence[Mapping[str, Any]],
    global_turns: Sequence[Mapping[str, Any]],
    global_activity: Mapping[str, Any],
    duration_seconds: float,
    total_inference_seconds: float,
    sample_name: str = DEFAULT_SAMPLE_NAME,
) -> dict[str, Any]:
    duration = _as_float(duration_seconds, "global_duration")
    inference = _as_float(total_inference_seconds, "total_inference_duration")
    if duration <= 0.0 or inference < 0.0 or not chunk_records:
        raise GateError("invalid_global_validation_input")

    effective_minimum = effective_min_chunk_seconds(duration)
    previous_end = 0.0
    for expected_index, chunk in enumerate(chunk_records, start=1):
        if int(chunk.get("chunk_index", -1)) != expected_index:
            raise GateError("chunk_sequence_not_contiguous")
        if chunk.get("state") != "PASS" or chunk.get("terminal_status") != "COMPLETED":
            raise GateError("chunk_did_not_reach_required_terminal_state")
        start = _as_float(chunk.get("start_seconds"), "chunk_start")
        end = _as_float(chunk.get("end_seconds"), "chunk_end")
        if abs(start - previous_end) > BOUNDARY_TOLERANCE_SECONDS:
            raise GateError("global_chunk_plan_has_gap_or_overlap")
        chunk_duration = end - start
        if len(chunk_records) > 1 and not (
            effective_minimum - BOUNDARY_TOLERANCE_SECONDS
            <= chunk_duration
            <= MAX_CHUNK_SECONDS + BOUNDARY_TOLERANCE_SECONDS
        ):
            raise GateError("completed_chunk_duration_outside_locked_range")
        previous_end = end
    if abs(previous_end - duration) > BOUNDARY_TOLERANCE_SECONDS:
        raise GateError("global_chunk_plan_does_not_reach_audio_end")

    if not global_turns:
        raise GateError("global_output_has_no_turns")
    intervals: list[dict[str, float]] = []
    previous_turn_start_ms = -1
    seen_chunks: set[int] = set()
    seen_labels: set[str] = set()
    for turn in global_turns:
        start_ms = int(turn["global_start_ms"])
        end_ms = int(turn["global_end_ms"])
        chunk_index = int(turn["chunk_index"])
        label = str(turn["speaker_label"])
        if start_ms < previous_turn_start_ms or end_ms < start_ms:
            raise GateError("global_turn_timestamps_not_monotonic")
        if start_ms < 0 or end_ms > int(
            round((duration + base.OUTPUT_END_TOLERANCE_SECONDS) * 1000.0)
        ):
            raise GateError("global_turn_timestamp_outside_audio")
        if not label.startswith(f"C{chunk_index:03d}:"):
            raise GateError("global_speaker_label_not_chunk_scoped")
        previous_turn_start_ms = start_ms
        seen_chunks.add(chunk_index)
        seen_labels.add(label)
        intervals.append({"start": start_ms / 1000.0, "end": end_ms / 1000.0})
    expected_chunks = set(range(1, len(chunk_records) + 1))
    if seen_chunks != expected_chunks:
        raise GateError("one_or_more_chunks_missing_from_global_turns")

    activity_intervals = _active_intervals(global_activity)
    first_activity = _as_float(activity_intervals[0]["start_seconds"], "first_activity")
    last_activity = _as_float(activity_intervals[-1]["end_seconds"], "last_activity")
    if intervals[0]["start"] > first_activity + base.OUTPUT_START_TOLERANCE_SECONDS:
        raise GateError("global_output_missing_audio_start")
    if intervals[-1]["end"] < last_activity - base.OUTPUT_END_TOLERANCE_SECONDS:
        raise GateError("global_output_missing_audio_end")

    transcript_interval_pairs = [(item["start"], item["end"]) for item in intervals]
    activity_interval_pairs = [
        (
            _as_float(item["start_seconds"], "activity_start"),
            _as_float(item["end_seconds"], "activity_end"),
        )
        for item in activity_intervals
    ]
    coverage = base.activity_coverage_metrics(transcript_interval_pairs, activity_interval_pairs)
    if coverage["activity_coverage_ratio"] < base.MIN_ACTIVITY_COVERAGE_RATIO:
        raise GateError("global_active_audio_coverage_below_threshold")
    if (
        coverage["max_uncovered_activity_gap_seconds"]
        > base.MAX_UNCOVERED_ACTIVITY_GAP_SECONDS
    ):
        raise GateError("global_active_audio_gap_exceeds_threshold")
    total_rtf = inference / duration
    if not math.isfinite(total_rtf) or total_rtf < 0.0:
        raise GateError("global_total_rtf_invalid")
    maximum_allowed_rtf = base.validate_rtf(sample_name, total_rtf)
    return {
        "state": "PASS",
        "chunk_count": len(chunk_records),
        "turn_count": len(global_turns),
        "scoped_speaker_label_count": len(seen_labels),
        "cross_chunk_speaker_identity_proven": False,
        "speaker_identity_statement": (
            "Speaker labels are isolated per chunk; equal local labels across chunks do not "
            "prove the same person."
        ),
        "first_turn_seconds": intervals[0]["start"],
        "last_turn_seconds": intervals[-1]["end"],
        "first_activity_seconds": first_activity,
        "last_activity_seconds": last_activity,
        "activity_coverage": coverage,
        "total_inference_seconds": inference,
        "total_audio_seconds": duration,
        "total_rtf": total_rtf,
        "maximum_allowed_rtf": maximum_allowed_rtf,
        "completeness_scope": (
            "structural timestamp/activity coverage only; semantic omissions require frozen "
            "human truth"
        ),
    }


def validate_memory_record(memory: Mapping[str, Any]) -> dict[str, Any]:
    import psutil

    required = (
        "sample_count",
        "process_peak_rss_bytes",
        "process_tree_peak_rss_bytes",
        "system_min_available_bytes",
    )
    for key in required:
        if key not in memory:
            raise GateError("peak_memory_record_incomplete")
    if int(memory["sample_count"]) <= 0:
        raise GateError("peak_memory_monitor_has_no_samples")
    if (
        int(memory["process_peak_rss_bytes"]) <= 0
        or int(memory["process_tree_peak_rss_bytes"]) <= 0
        or int(memory["system_min_available_bytes"]) <= 0
    ):
        raise GateError("peak_memory_record_invalid")
    record = dict(memory)
    system_total = int(psutil.virtual_memory().total)
    record["system_total_bytes"] = system_total
    record["peak_system_used_bytes"] = system_total - int(memory["system_min_available_bytes"])
    return record


def validate_memory_admission(
    session_limits: Mapping[str, Any], device: Any, chunk_duration_seconds: float
) -> dict[str, Any]:
    import psutil

    duration = _as_float(chunk_duration_seconds, "chunk_duration")
    if duration > MAX_CHUNK_SECONDS + BOUNDARY_TOLERANCE_SECONDS:
        raise GateError("chunk_exceeds_safe_duration_before_inference")
    max_kv_bytes = int(session_limits.get("max_kv_bytes", 0))
    max_audio_ms = int(session_limits.get("effective_max_audio_ms", 0))
    if max_kv_bytes <= 0 or max_audio_ms <= 0:
        raise GateError("session_limits_do_not_provide_memory_and_audio_bounds")
    if int(round(duration * 1000.0)) > max_audio_ms:
        raise GateError("chunk_exceeds_session_audio_limit")
    available_system = int(psutil.virtual_memory().available)
    required_system = max_kv_bytes + SYSTEM_MEMORY_MARGIN_BYTES
    if available_system < required_system:
        raise GateError("insufficient_system_memory_for_bounded_chunk_session")
    device_free = int(getattr(device, "memory_free", 0) or 0)
    required_device = max_kv_bytes + DEVICE_MEMORY_MARGIN_BYTES
    if device_free > 0 and device_free < required_device:
        raise GateError("insufficient_device_memory_for_bounded_chunk_session")
    return {
        "chunk_duration_seconds": duration,
        "session_max_audio_ms": max_audio_ms,
        "session_max_kv_bytes": max_kv_bytes,
        "system_available_bytes": available_system,
        "system_required_bytes": required_system,
        "device_free_bytes": device_free,
        "device_required_bytes": required_device,
        "state": "PASS",
    }


def _git(args: list[str]) -> subprocess.CompletedProcess[str]:
    return base.frozen_git_run(args)


def validate_chunked_toolchain(
    sample_name: str = DEFAULT_SAMPLE_NAME,
) -> dict[str, Any]:
    if sample_name not in ALLOWED_CHUNKED_SAMPLE_NAMES:
        raise GateError("sample_is_not_approved_for_bounded_chunk_execution")
    frozen = base.validate_test_toolchain()
    own_records: list[dict[str, Any]] = []
    for path in (SCRIPT_PATH, TEST_PATH):
        relative = path.relative_to(base.REPO_ROOT).as_posix()
        tracked = _git(["ls-files", "--error-unmatch", "--", relative])
        if tracked.returncode != 0:
            raise GateError("chunked_runner_toolchain_file_not_git_tracked")
        status = _git(["status", "--porcelain", "--untracked-files=all", "--", relative])
        if status.returncode != 0 or status.stdout.strip():
            raise GateError("chunked_runner_toolchain_file_not_clean")
        own_records.append(base.file_record(path))

    lock = base.read_json(base.LOCK_PATH, "P1 lock file")
    if lock.get("stage") != "MOSS_V3_P1" or int(lock.get("schema_version", 0)) != 1:
        raise GateError("frozen_lock_identity_mismatch_for_chunked_runner")
    sample_records = lock.get("samples")
    if not isinstance(sample_records, list):
        raise GateError("frozen_lock_sample_list_missing")
    matching_samples = [
        item
        for item in sample_records
        if item == sample_name
        or (isinstance(item, dict) and item.get("name") == sample_name)
    ]
    if len(matching_samples) != 1:
        raise GateError("frozen_chunked_sample_not_unique_in_lock")
    expected = base.EXPECTED_SAMPLES[sample_name]
    if int(expected.get("bytes", -1)) <= 0 or not base.SHA256_RE.fullmatch(
        str(expected.get("sha256", ""))
    ):
        raise GateError("frozen_chunked_sample_code_identity_invalid")
    locked_sample = matching_samples[0]
    if isinstance(locked_sample, dict) and (
        int(locked_sample.get("bytes", -1)) != int(expected["bytes"])
        or str(locked_sample.get("sha256", "")).upper() != str(expected["sha256"])
    ):
        raise GateError("frozen_chunked_sample_lock_identity_mismatch")
    return {"base_toolchain": frozen, "chunked_toolchain": own_records, "state": "PASS"}


def _strict_public_scrub(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: _strict_public_scrub(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_strict_public_scrub(item) for item in value]
    if isinstance(value, str) and PUBLIC_ABSOLUTE_PATH_RE.search(value):
        return {
            "redacted": True,
            "sha256": hashlib.sha256(value.encode("utf-8")).hexdigest().upper(),
            "length": len(value),
        }
    return value


def make_public_evidence(payload: Mapping[str, Any]) -> dict[str, Any]:
    public = _strict_public_scrub(base.make_public_evidence(dict(payload)))
    base.assert_public_evidence_safe(public)
    encoded = json.dumps(public, ensure_ascii=False)
    if PUBLIC_ABSOLUTE_PATH_RE.search(encoded):
        raise GateError("public_evidence_absolute_path_redaction_failed")
    return public


def _record_post_integrity(
    model_record: Mapping[str, Any],
    source_record: Mapping[str, Any],
    prepared_record: Mapping[str, Any],
    chunk_set: Mapping[str, Any],
) -> dict[str, Any]:
    model_post = base.validate_locked_file(
        Path(str(model_record["path"])),
        int(model_record["bytes"]),
        str(model_record["sha256"]),
        "model_post",
    )
    source_post = base.validate_locked_file(
        Path(str(source_record["path"])),
        int(source_record["bytes"]),
        str(source_record["sha256"]),
        "source_post",
    )
    prepared_post = base.validate_locked_file(
        Path(str(prepared_record["path"])),
        int(prepared_record["bytes"]),
        str(prepared_record["sha256"]),
        "prepared_audio_post",
    )
    chunk_posts = []
    for chunk in chunk_set["chunks"]:
        chunk_posts.append(
            base.validate_locked_file(
                Path(str(chunk["path"])),
                int(chunk["bytes"]),
                str(chunk["sha256"]),
                f"chunk_{int(chunk['chunk_index']):03d}_post",
            )
        )
    manifest = chunk_set["manifest"]
    manifest_post = base.validate_locked_file(
        Path(str(manifest["path"])),
        int(manifest["bytes"]),
        str(manifest["sha256"]),
        "chunk_manifest_post",
    )
    return {
        "model": model_post,
        "source": source_post,
        "prepared_audio": prepared_post,
        "chunks": chunk_posts,
        "chunk_manifest": manifest_post,
        "loaded_native_modules": base.capture_loaded_native_modules(phase="inference"),
        "runtime_and_binding": base.validate_frozen_files(),
        "test_toolchain": base.validate_test_toolchain(),
        "state": "PASS",
    }


def run_chunked(
    model_path: Path,
    sample_path: Path,
    sample_name: str = DEFAULT_SAMPLE_NAME,
) -> dict[str, Any]:
    if sample_name not in ALLOWED_CHUNKED_SAMPLE_NAMES:
        raise GateError("sample_is_not_approved_for_bounded_chunk_execution")
    started_at = base.now_iso()
    wall_started = time.perf_counter()
    context: dict[str, Any] = {"sample_name": sample_name, "completed_chunks": []}
    run_chunked._last_context = context
    monitor: Any = None
    memory: dict[str, Any] | None = None
    try:
        inference_environment = base.validate_inference_environment()
        context["inference_environment"] = inference_environment
        toolchain = validate_chunked_toolchain(sample_name)
        context["toolchain"] = toolchain
        host = base.capture_and_validate_host()
        context["host"] = host
        module, binding = base.load_transcribe_cpp()
        context["binding"] = binding
        device, device_inventory = base.select_exact_vulkan_device(module)
        context["device_inventory"] = device_inventory

        model_record = base.validate_model(
            model_path,
            base.EXPECTED_MODEL_BYTES,
            base.EXPECTED_MODEL_SHA256,
        )
        context["model"] = model_record
        expected_sample = base.EXPECTED_SAMPLES[sample_name]
        sample_args = argparse.Namespace(
            sample_name=sample_name,
            sample=sample_path,
            sample_bytes=expected_sample["bytes"],
            sample_sha256=expected_sample["sha256"],
        )
        base.validate_sample_lock(sample_args)
        source_record = base.validate_locked_file(
            sample_path,
            expected_sample["bytes"],
            expected_sample["sha256"],
            "frozen long-form source sample",
        )
        context["source_sample"] = source_record

        monitor = base.MemoryMonitor()
        monitor.start()
        prepared_record, conversion = base.prepare_audio(sample_name, source_record)
        context["prepared_audio"] = prepared_record
        context["conversion"] = conversion
        full_activity = base.probe_audio_activity(
            Path(str(prepared_record["path"])), float(prepared_record["duration_seconds"])
        )
        context["global_activity"] = full_activity
        prepared_duration = float(prepared_record["duration_seconds"])
        plan = choose_chunk_plan(
            prepared_duration,
            full_activity["silence_intervals"],
            min_chunk_seconds=effective_min_chunk_seconds(prepared_duration),
        )
        plan = quantize_chunk_plan(plan, int(prepared_record["frames"]), 16_000)
        context["chunk_plan"] = plan
        context["chunking_source_pre"] = base.validate_locked_file(
            Path(str(prepared_record["path"])),
            int(prepared_record["bytes"]),
            str(prepared_record["sha256"]),
            "prepared audio immediately before chunking",
        )
        chunk_set = materialize_chunks(prepared_record, plan, sample_name)
        context["chunk_set"] = chunk_set
        context["chunking_source_post"] = base.validate_locked_file(
            Path(str(prepared_record["path"])),
            int(prepared_record["bytes"]),
            str(prepared_record["sha256"]),
            "prepared audio immediately after chunking",
        )

        global_turns: list[dict[str, Any]] = []
        chunk_results: list[dict[str, Any]] = []
        total_inference_seconds = 0.0
        model_load_started = time.perf_counter()
        with module.Model(
            model_record["path"], backend="vulkan", device=device
        ) as model:
            model_load_seconds = time.perf_counter() - model_load_started
            resolved_device = model.device
            if resolved_device != device:
                raise GateError("model_resolved_device_differs_from_exact_selected_device")
            resolved_device_record = base.device_record(resolved_device)
            if resolved_device_record["kind"] != base.EXPECTED_DEVICE_KIND:
                raise GateError("model_resolved_device_kind_mismatch")
            if resolved_device_record["description"] != base.EXPECTED_DEVICE_DESCRIPTION:
                raise GateError("model_resolved_device_description_mismatch")
            backend_name = str(model.backend)
            allowed_backend_names = {
                base.EXPECTED_DEVICE_KIND.casefold(),
                str(device.name).casefold(),
            }
            if backend_name.casefold() not in allowed_backend_names:
                raise GateError("model_backend_is_not_vulkan")
            context["model_runtime"] = {
                "backend": backend_name,
                "device": resolved_device_record,
                "capabilities": dataclasses.asdict(model.capabilities),
                "load_seconds": model_load_seconds,
            }
            context["loaded_native_modules"] = base.capture_loaded_native_modules(
                phase="inference"
            )

            for plan_item, chunk_file in zip(plan, chunk_set["chunks"], strict=True):
                chunk_index = int(plan_item["chunk_index"])
                context["current_chunk_index"] = chunk_index
                chunk_duration = float(plan_item["duration_seconds"])
                chunk_path = Path(str(chunk_file["path"]))
                chunk_activity = base.probe_audio_activity(chunk_path, chunk_duration)
                context["current_chunk_activity"] = chunk_activity
                pcm, pcm_record = base.load_pcm_float32(chunk_path)
                if int(pcm_record["samples"]) != int(chunk_file["frames"]):
                    raise GateError("chunk_pcm_sample_count_mismatch")
                if (
                    str(pcm_record["source_pcm_s16le_sha256"])
                    != str(chunk_file["pcm_s16le_sha256"])
                ):
                    raise GateError("chunk_pcm_payload_hash_mismatch")
                result: Any = None
                session_started = time.perf_counter()
                try:
                    with model.session(
                        n_threads=CHUNK_N_THREADS,
                        kv_type=CHUNK_KV_TYPE,
                        n_ctx=CHUNK_N_CTX,
                    ) as session:
                        session_limits = dataclasses.asdict(session.limits)
                        admission = validate_memory_admission(
                            session_limits, resolved_device, chunk_duration
                        )
                        inference_started = time.perf_counter()
                        result = session.run(
                            pcm,
                            language="zh",
                            timestamps="segment",
                            diarize="on",
                        )
                        inference_seconds = time.perf_counter() - inference_started
                except Exception as exc:
                    context["current_chunk_failure"] = {
                        "chunk_index": chunk_index,
                        "error_code": base.stable_error_code(exc),
                        "error_type": type(exc).__name__,
                        "message": str(exc),
                        "partial_result": base.partial_result_record(exc),
                    }
                    raise
                finally:
                    del pcm
                    gc.collect()

                output = base.result_record(result)
                del result
                context["current_chunk_output"] = output
                local_gate = base.validate_output(
                    sample_name, output, chunk_duration, chunk_activity
                )
                turns = offset_chunk_output(
                    output, chunk_index, float(plan_item["start_seconds"])
                )
                global_turns.extend(turns)
                total_inference_seconds += inference_seconds
                chunk_record = {
                    "chunk_index": chunk_index,
                    "start_seconds": float(plan_item["start_seconds"]),
                    "end_seconds": float(plan_item["end_seconds"]),
                    "duration_seconds": chunk_duration,
                    "end_boundary": plan_item["end_boundary"],
                    "audio": chunk_file,
                    "pcm_load": pcm_record,
                    "activity": chunk_activity,
                    "session": {
                        "n_threads": CHUNK_N_THREADS,
                        "kv_type": CHUNK_KV_TYPE,
                        "n_ctx": CHUNK_N_CTX,
                        "limits": session_limits,
                        "memory_admission": admission,
                    },
                    "output": output,
                    "global_turns": turns,
                    "local_gate": local_gate,
                    "inference_seconds": inference_seconds,
                    "session_wall_seconds": time.perf_counter() - session_started,
                    "rtf": inference_seconds / chunk_duration,
                    "terminal_status": "COMPLETED",
                    "state": "PASS",
                }
                chunk_results.append(chunk_record)
                context["completed_chunks"] = chunk_results
                context.pop("current_chunk_index", None)
                context.pop("current_chunk_activity", None)
                context.pop("current_chunk_output", None)

        post_integrity = _record_post_integrity(
            model_record, source_record, prepared_record, chunk_set
        )
        context["post_integrity"] = post_integrity
        memory = monitor.stop()
        monitor = None
        memory = validate_memory_record(memory)
        context["memory"] = memory
        global_gate = validate_global_output(
            chunk_results,
            global_turns,
            full_activity,
            float(prepared_record["duration_seconds"]),
            total_inference_seconds,
            sample_name,
        )
        global_gate["peak_memory"] = memory
        context["global_gate"] = global_gate
        finished_at = base.now_iso()
        payload = {
            "schema_version": 1,
            "stage": "MOSS_V3_P1_CHUNKED",
            "run_state": "STRUCTURAL_RUNTIME_PASS",
            "accuracy_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "simplified_chinese_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "semantic_completeness_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "diarization_accuracy_status": "NOT_SCORABLE_P0_R_BLOCKED",
            "p1_verdict": "NOT_EVALUATED_P0_R_BLOCKED",
            "release_go": False,
            "completeness_scope": (
                "structural timestamp/activity coverage only; semantic omissions require frozen "
                "human truth"
            ),
            "started_at": started_at,
            "finished_at": finished_at,
            "wall_seconds": time.perf_counter() - wall_started,
            "toolchain": toolchain,
            "inference_environment": inference_environment,
            "host": host,
            "binding": binding,
            "model": model_record,
            "source_sample": source_record,
            "prepared_audio": prepared_record,
            "conversion": conversion,
            "global_activity": full_activity,
            "chunk_plan": plan,
            "chunk_set": chunk_set,
            "model_runtime": context["model_runtime"],
            "loaded_native_modules": context["loaded_native_modules"],
            "chunks": chunk_results,
            "global_turns": global_turns,
            "global_gate": global_gate,
            "post_integrity": post_integrity,
            "memory": memory,
            "cross_chunk_speaker_identity_proven": False,
        }
        run_chunked._last_context = payload
        return payload
    except Exception:
        if monitor is not None:
            try:
                memory = validate_memory_record(monitor.stop())
                context["memory"] = memory
            except Exception as memory_exc:
                context["memory_monitor_error"] = {
                    "error_code": base.stable_error_code(memory_exc),
                    "error_type": type(memory_exc).__name__,
                    "message": str(memory_exc),
                }
        context["started_at"] = started_at
        context["failed_at"] = base.now_iso()
        context["wall_seconds"] = time.perf_counter() - wall_started
        run_chunked._last_context = context
        raise


run_chunked._last_context = {}


def _write_evidence_pair(
    public_dir: Path, private_dir: Path, stem: str, private_payload: Mapping[str, Any]
) -> tuple[Path, Path]:
    private_path = private_dir / f"{stem}.private.json"
    private_hash = base.write_evidence_atomic(private_path, dict(private_payload))
    private_record = base.file_record(private_path)
    public_payload = make_public_evidence(private_payload)
    public_payload["private_evidence"] = {
        "bytes": private_record["bytes"],
        "sha256": private_record["sha256"],
        "restricted": True,
    }
    public_payload["restricted_attestation_required"] = True
    base.assert_public_evidence_safe(public_payload)
    public_path = public_dir / f"{stem}.json"
    public_hash = base.write_evidence_atomic(public_path, public_payload)
    base.write_public_attestation_atomic(
        private_dir,
        public_path,
        public_hash,
        private_path,
        private_hash,
    )
    return public_path, private_path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Strict bounded-session MOSS V3/P1 run for a frozen long-form sample"
    )
    parser.add_argument(
        "--sample-name",
        choices=sorted(ALLOWED_CHUNKED_SAMPLE_NAMES),
        default=DEFAULT_SAMPLE_NAME,
        help="Frozen sample identity bound to the P1 lock.",
    )
    parser.add_argument(
        "--model",
        type=Path,
        default=base.MODEL_ROOT / base.EXPECTED_MODEL_NAME,
        help="Frozen MOSS GGUF model; byte length and SHA-256 remain mandatory.",
    )
    parser.add_argument(
        "--sample",
        type=Path,
        required=True,
        help="Frozen long-form source sample; byte length and SHA-256 remain mandatory.",
    )
    parser.add_argument(
        "--evidence",
        type=Path,
        required=True,
        help="New public evidence directory whose name begins with MOSS-V3-P1-.",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    public_dir: Path | None = None
    private_dir: Path | None = None
    run_id = f"{time.strftime('%Y%m%dT%H%M%S')}-{os.getpid()}"
    stem = f"moss-p1-{args.sample_name}-chunked-{run_id}"
    try:
        # Authenticate the fixed toolchain, including PowerShell, before the
        # shared ACL helper is allowed to mutate evidence directories.
        toolchain = base.validate_test_toolchain()
        public_dir = base.ensure_evidence_directory(
            args.evidence, str(toolchain["git_commit"])
        )
        private_dir = base.ensure_private_evidence_directory(public_dir)
        payload = run_chunked(args.model, args.sample, args.sample_name)
        public_path, _ = _write_evidence_pair(public_dir, private_dir, stem, payload)
        print(
            json.dumps(
                {
                    "run_state": "STRUCTURAL_RUNTIME_PASS",
                    "evidence_file": public_path.name,
                    "evidence_sha256": base.sha256(public_path),
                },
                ensure_ascii=False,
            )
        )
        return 0
    except Exception as exc:
        failure = {
            "schema_version": 1,
            "stage": "MOSS_V3_P1_CHUNKED",
            "run_state": "FAIL",
            "failed_at": base.now_iso(),
            "error": {
                "error_code": base.stable_error_code(exc),
                "error_type": type(exc).__name__,
                "message": str(exc),
                "traceback": traceback.format_exc(),
                "partial_result": base.partial_result_record(exc),
            },
            "context": getattr(run_chunked, "_last_context", {}),
            "cross_chunk_speaker_identity_proven": False,
        }
        if public_dir is not None and private_dir is not None:
            try:
                public_path, _ = _write_evidence_pair(public_dir, private_dir, stem, failure)
                print(
                    json.dumps(
                        {
                            "run_state": "FAIL",
                            "evidence_file": public_path.name,
                            "evidence_sha256": base.sha256(public_path),
                            "error_code": base.stable_error_code(exc),
                        },
                        ensure_ascii=False,
                    )
                )
            except Exception as evidence_exc:
                print(
                    json.dumps(
                        {
                            "run_state": "FAIL",
                            "error_code": base.stable_error_code(exc),
                            "evidence_error_code": base.stable_error_code(evidence_exc),
                        },
                        ensure_ascii=False,
                    ),
                    file=sys.stderr,
                )
        else:
            print(
                json.dumps(
                    {"run_state": "FAIL", "error_code": base.stable_error_code(exc)},
                    ensure_ascii=False,
                ),
                file=sys.stderr,
            )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
