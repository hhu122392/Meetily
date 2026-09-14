#!/usr/bin/env python3
"""Independently compare the final MOSS timestamp with PCM tail activity."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
import wave
from datetime import datetime, timezone
from pathlib import Path


FRAME_MS = 20
THRESHOLDS_DBFS = (-55.0, -50.0, -45.0, -40.0, -35.0)
SELECTED_THRESHOLD_DBFS = -50.0
MAXIMUM_TAIL_ERROR_MS = 250


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def frame_dbfs(frame: bytes) -> float:
    count = len(frame) // 2
    if count == 0:
        return float("-inf")
    sum_squares = 0
    for (sample,) in struct.iter_unpack("<h", frame):
        sum_squares += sample * sample
    rms = math.sqrt(sum_squares / count)
    if rms == 0:
        return float("-inf")
    return 20.0 * math.log10(rms / 32768.0)


def inspect_wav(path: Path) -> dict[str, object]:
    resolved = path.resolve(strict=True)
    with wave.open(str(resolved), "rb") as audio:
        channels = audio.getnchannels()
        sample_width = audio.getsampwidth()
        sample_rate = audio.getframerate()
        frame_count = audio.getnframes()
        compression = audio.getcomptype()
        if channels != 1 or sample_width != 2 or sample_rate != 16_000:
            raise ValueError("expected mono 16 kHz PCM16 WAV")
        if compression != "NONE":
            raise ValueError("expected uncompressed PCM WAV")
        samples_per_window = sample_rate * FRAME_MS // 1000
        last_active_frame = {threshold: None for threshold in THRESHOLDS_DBFS}
        frame_index = 0
        while True:
            frame = audio.readframes(samples_per_window)
            if not frame:
                break
            dbfs = frame_dbfs(frame)
            for threshold in THRESHOLDS_DBFS:
                if dbfs >= threshold:
                    last_active_frame[threshold] = frame_index
            frame_index += 1

    duration_ms = frame_count * 1000.0 / sample_rate
    tail_by_threshold: dict[str, float | None] = {}
    for threshold, index in last_active_frame.items():
        key = f"{threshold:.0f}"
        tail_by_threshold[key] = (
            None
            if index is None
            else min((index + 1) * FRAME_MS, duration_ms)
        )
    return {
        "bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
        "channels": channels,
        "sample_width_bytes": sample_width,
        "sample_rate_hz": sample_rate,
        "frames": frame_count,
        "duration_ms": duration_ms,
        "last_active_end_ms_by_threshold_dbfs": tail_by_threshold,
    }


def audit(
    source: Path,
    expected_sha256: str,
    model_last_ms: int,
) -> dict[str, object]:
    audio = inspect_wav(source)
    selected_tail_value = audio["last_active_end_ms_by_threshold_dbfs"]["-50"]
    if not isinstance(selected_tail_value, (int, float)):
        raise ValueError("no audio activity found at the selected threshold")
    selected_tail = float(selected_tail_value)
    error_ms = abs(float(model_last_ms) - selected_tail)
    checks = {
        "source_hash_matches": audio["sha256"].lower() == expected_sha256.lower(),
        "model_timestamp_is_non_negative": model_last_ms >= 0,
        "model_timestamp_not_after_audio": model_last_ms <= audio["duration_ms"],
        "tail_error_within_250_ms": error_ms <= MAXIMUM_TAIL_ERROR_MS,
    }
    status = "PASS" if all(checks.values()) else "FAIL"
    return {
        "schema_version": 1,
        "role": "MOSS_R2_PCM_TAIL_ACTIVITY_AUDIT",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "status": status,
        "method": {
            "frame_ms": FRAME_MS,
            "measure": "PCM16_FRAME_RMS_DBFS",
            "thresholds_dbfs": THRESHOLDS_DBFS,
            "selected_threshold_dbfs": SELECTED_THRESHOLD_DBFS,
            "maximum_tail_error_ms": MAXIMUM_TAIL_ERROR_MS,
        },
        "audio": audio,
        "model_last_timestamp_ms": model_last_ms,
        "selected_last_active_end_ms": selected_tail,
        "absolute_tail_error_ms": error_ms,
        "checks": checks,
        "conclusion": {
            "audio_tail_gate_status": status,
            "hard_extend_model_timestamp_into_silence": False,
        },
        "absolute_paths_in_evidence": False,
        "transcript_in_evidence": False,
    }


def write_new_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        raise FileExistsError(f"refusing to overwrite evidence: {path}")
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--model-last-ms", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = audit(args.source, args.expected_sha256, args.model_last_ms)
    write_new_json(args.output, payload)
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
