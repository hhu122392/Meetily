#!/usr/bin/env python3
"""Validate one isolated D-11 real-time measurement directory.

The validator reads only the directory passed on the command line. It does not
discover application data, recordings, or truth files elsewhere on the host.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


FILES = {
    "contract": "audio-input-contract.json",
    "capture": "capture-pcm.f32le",
    "trace": "realtime-transcription-trace.jsonl",
    "load": "load-timeline.jsonl",
    "stop": "stop-save-timeline.jsonl",
    "summary": "realtime-measurement.json",
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest().upper()


def read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path.name} must contain a JSON object")
    return value


def read_jsonl(path: Path) -> tuple[list[dict[str, Any]], bytes]:
    data = path.read_bytes()
    rows: list[dict[str, Any]] = []
    for line_number, raw_line in enumerate(data.splitlines(), 1):
        if not raw_line.strip():
            continue
        value = json.loads(raw_line)
        if not isinstance(value, dict):
            raise ValueError(f"{path.name}:{line_number} must contain a JSON object")
        rows.append(value)
    return rows, data


def require(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


def is_hash(value: Any) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789ABCDEFabcdef" for character in value)
    )


def monotonic(values: list[int]) -> bool:
    return all(left <= right for left, right in zip(values, values[1:]))


def validate(directory: Path) -> dict[str, Any]:
    root = directory.resolve(strict=True)
    failures: list[str] = []
    require(
        "com.meetily.ai" not in str(root).lower(),
        "formal application data path rejected",
        failures,
    )

    paths = {name: root / filename for name, filename in FILES.items()}
    for name, path in paths.items():
        require(path.is_file(), f"missing {name} file: {path.name}", failures)
    if failures:
        return {"schema_version": 1, "status": "FAIL", "failures": failures}

    contract = read_json(paths["contract"])
    summary = read_json(paths["summary"])
    trace_rows, trace_bytes = read_jsonl(paths["trace"])
    load_rows, load_bytes = read_jsonl(paths["load"])
    stop_rows, stop_bytes = read_jsonl(paths["stop"])
    capture_bytes = paths["capture"].read_bytes()

    require(contract.get("schema_version") == 1, "input contract schema_version must be 1", failures)
    require(summary.get("schema_version") == 1, "summary schema_version must be 1", failures)
    for field in ("audio_input_path_id", "baseline_snapshot_id", "isolated_data_identity"):
        require(
            contract.get(field) == summary.get(field),
            f"contract/summary {field} mismatch",
            failures,
        )
    require(
        all(
            "com.meetily.ai" not in str(contract.get(field, "")).lower()
            for field in ("input_path", "controlled_input_root", "isolated_output_root")
        ),
        "contract contains a formal application data path",
        failures,
    )

    input_hash = contract.get("input_file_sha256")
    canonical_hash = contract.get("canonical_pcm_sha256")
    sample_count = contract.get("canonical_sample_count")
    require(is_hash(input_hash), "input_file_sha256 is invalid", failures)
    require(is_hash(canonical_hash), "canonical_pcm_sha256 is invalid", failures)
    require(
        isinstance(sample_count, int) and sample_count > 0,
        "canonical_sample_count is invalid",
        failures,
    )
    if is_hash(input_hash) and is_hash(canonical_hash) and isinstance(sample_count, int):
        material = (
            "schema_version=1\n"
            f"input_file_sha256={input_hash.upper()}\n"
            f"canonical_pcm_sha256={canonical_hash.upper()}\n"
            "sample_rate_hz=16000\nchannels=1\nsample_format=f32le\n"
            f"sample_count={sample_count}\n"
        ).encode("utf-8")
        expected_id = f"audio_input_v1_{hashlib.sha256(material).hexdigest()}"
        require(
            contract.get("audio_input_path_id") == expected_id,
            "audio_input_path_id is not reproducible",
            failures,
        )

    capture_hash = sha256(capture_bytes)
    capture_sample_count = len(capture_bytes) // 4
    require(len(capture_bytes) % 4 == 0, "capture PCM byte count is not f32-aligned", failures)
    require(summary.get("capture_pcm_sha256") == capture_hash, "capture_pcm_sha256 mismatch", failures)
    require(
        summary.get("capture_pcm_sample_count") == capture_sample_count,
        "capture PCM sample count mismatch",
        failures,
    )
    require(
        summary.get("capture_pcm", {}).get("bytes") == len(capture_bytes),
        "capture file byte record mismatch",
        failures,
    )
    require(
        summary.get("capture_pcm", {}).get("sha256") == capture_hash,
        "capture file hash record mismatch",
        failures,
    )

    for key, data, summary_key in (
        ("trace", trace_bytes, "trace"),
        ("load", load_bytes, "load_timeline"),
        ("stop", stop_bytes, "stop_save_timeline"),
    ):
        record = summary.get(summary_key, {})
        require(record.get("bytes") == len(data), f"{key} byte record mismatch", failures)
        require(record.get("sha256") == sha256(data), f"{key} hash record mismatch", failures)
    require(
        summary.get("load_timeline_sha256") == sha256(load_bytes),
        "load_timeline_sha256 mismatch",
        failures,
    )

    chunks = [row.get("value", {}) for row in trace_rows if row.get("record_type") == "chunk"]
    vad_windows = [
        row.get("value", {}) for row in trace_rows if row.get("record_type") == "vad_window"
    ]
    require(summary.get("chunk_count") == len(chunks), "chunk_count mismatch", failures)
    require(len(chunks) > 0, "chunk trace is empty", failures)
    require(summary.get("vad_window_count") == len(vad_windows), "vad_window_count mismatch", failures)
    require(summary.get("load_sample_count") == len(load_rows), "load_sample_count mismatch", failures)
    require(summary.get("stop_save_event_count") == len(stop_rows), "stop_save_event_count mismatch", failures)
    require(len(load_rows) > 0, "load timeline is empty", failures)
    require(len(stop_rows) > 0, "stop/save timeline is empty", failures)

    seen_chunks: set[int] = set()
    for chunk in chunks:
        chunk_id = chunk.get("chunk_id")
        require(
            isinstance(chunk_id, int) and chunk_id not in seen_chunks,
            f"invalid or duplicate chunk_id: {chunk_id}",
            failures,
        )
        if isinstance(chunk_id, int):
            seen_chunks.add(chunk_id)
        required = (
            "source_sample_start",
            "source_sample_end",
            "sample_rate_hz",
            "vad_decision",
            "enqueued_at_monotonic_ns",
            "dequeued_at_monotonic_ns",
            "inference_started_at_monotonic_ns",
            "inference_finished_at_monotonic_ns",
            "final_writeback_at_monotonic_ns",
            "inference_sample_start",
            "inference_sample_end",
            "inference_sample_count",
            "inference_pcm_sha256",
            "transcription_provider",
            "transcription_model",
            "text_before_dedup",
            "text_after_dedup",
            "text_after_context_normalization",
            "dedup_trace_status",
            "language_applied_to_provider",
            "writeback_result",
        )
        for field in required:
            require(chunk.get(field) is not None, f"chunk {chunk_id} missing {field}", failures)
        times = [
            chunk.get(field)
            for field in (
                "enqueued_at_monotonic_ns",
                "dequeued_at_monotonic_ns",
                "inference_started_at_monotonic_ns",
                "inference_finished_at_monotonic_ns",
                "final_writeback_at_monotonic_ns",
            )
        ]
        if all(isinstance(value, int) for value in times):
            require(monotonic(times), f"chunk {chunk_id} timestamps are not monotonic", failures)
        require(
            is_hash(chunk.get("inference_pcm_sha256")),
            f"chunk {chunk_id} inference hash is invalid",
            failures,
        )
        inference_start = chunk.get("inference_sample_start")
        inference_end = chunk.get("inference_sample_end")
        inference_count = chunk.get("inference_sample_count")
        if all(isinstance(value, int) for value in (inference_start, inference_end, inference_count)):
            require(
                0 <= inference_start <= inference_end <= capture_sample_count,
                f"chunk {chunk_id} inference sample window is invalid",
                failures,
            )
            require(
                inference_end - inference_start == inference_count,
                f"chunk {chunk_id} inference sample count does not match its window",
                failures,
            )
            inference_bytes = capture_bytes[inference_start * 4 : inference_end * 4]
            require(
                sha256(inference_bytes) == chunk.get("inference_pcm_sha256"),
                f"chunk {chunk_id} inference hash does not match capture PCM",
                failures,
            )
        context_id, context_hash = chunk.get("context_version_id"), chunk.get("context_sha256")
        require(
            (context_id is None) == (context_hash is None),
            f"chunk {chunk_id} context identity/hash mismatch",
            failures,
        )

    vad_times = [row.get("observed_at_monotonic_ns") for row in vad_windows]
    if all(isinstance(value, int) for value in vad_times):
        require(monotonic(vad_times), "VAD observations are not monotonic", failures)
    for row in vad_windows:
        require(
            row.get("source_sample_start", -1) <= row.get("source_sample_end", -2),
            "invalid VAD sample window",
            failures,
        )

    load_times = [row.get("monotonic_ns") for row in load_rows]
    if all(isinstance(value, int) for value in load_times):
        require(monotonic(load_times), "load timeline is not monotonic", failures)
    for index, row in enumerate(load_rows):
        require(
            row.get("system_cpu_percent") is not None or row.get("system_unavailable_reason"),
            f"load row {index} hides missing system CPU",
            failures,
        )
        require(
            (
                row.get("process_cpu_percent") is not None
                and row.get("process_memory_used_bytes") is not None
            )
            or row.get("process_unavailable_reason"),
            f"load row {index} hides missing process metrics",
            failures,
        )
        require(
            row.get("gpu_percent") is not None or row.get("gpu_unavailable_reason"),
            f"load row {index} hides missing GPU",
            failures,
        )

    stop_times = [row.get("monotonic_ns") for row in stop_rows]
    if all(isinstance(value, int) for value in stop_times):
        require(monotonic(stop_times), "stop/save timeline is not monotonic", failures)
    stop_stages = {row.get("stage") for row in stop_rows}
    for stage in ("recording_started", "stop_requested", "queue_drained", "measurement_finalized"):
        require(stage in stop_stages, f"stop/save timeline missing {stage}", failures)

    engine = summary.get("transcription_engine")
    require(
        isinstance(engine, dict) and engine.get("provider") and engine.get("model"),
        "transcription engine identity missing",
        failures,
    )
    require(
        summary.get("measurement_started_at_monotonic_ns") == 0,
        "measurement start must be monotonic origin 0",
        failures,
    )
    require(
        isinstance(summary.get("measurement_finalized_at_monotonic_ns"), int),
        "measurement final time missing",
        failures,
    )

    return {
        "schema_version": 1,
        "status": "PASS" if not failures else "FAIL",
        "measurement_directory": str(root),
        "audio_input_path_id": summary.get("audio_input_path_id"),
        "chunk_count": len(chunks),
        "vad_window_count": len(vad_windows),
        "load_sample_count": len(load_rows),
        "stop_save_event_count": len(stop_rows),
        "failure_count": len(failures),
        "failures": failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("measurement_directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = validate(args.measurement_directory)
    except Exception as error:  # fail closed with a machine-readable result
        report = {
            "schema_version": 1,
            "status": "FAIL",
            "failure_count": 1,
            "failures": [str(error)],
        }
    text = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.write_text(text, encoding="utf-8")
    print(text, end="")
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
