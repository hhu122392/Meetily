#!/usr/bin/env python3
"""Focused tests for the isolated D-11 measurement evidence validator."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest

from validate_d11_realtime_measurement import FILES, sha256, validate


def write_jsonl(path: Path, rows: list[dict]) -> bytes:
    data = b"".join(
        json.dumps(row, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"
        for row in rows
    )
    path.write_bytes(data)
    return data


def file_record(path: Path, data: bytes) -> dict:
    return {"path": path.name, "bytes": len(data), "sha256": sha256(data)}


def make_fixture(root: Path) -> None:
    input_hash = "A" * 64
    canonical_hash = "B" * 64
    sample_count = 16_000
    material = (
        "schema_version=1\n"
        f"input_file_sha256={input_hash}\n"
        f"canonical_pcm_sha256={canonical_hash}\n"
        "sample_rate_hz=16000\nchannels=1\nsample_format=f32le\n"
        f"sample_count={sample_count}\n"
    ).encode()
    audio_input_path_id = f"audio_input_v1_{hashlib.sha256(material).hexdigest()}"
    contract = {
        "schema_version": 1,
        "audio_input_path_id": audio_input_path_id,
        "input_path": str(root / "input.wav"),
        "controlled_input_root": str(root),
        "isolated_output_root": str(root),
        "isolated_data_identity": "d11-test",
        "baseline_snapshot_id": "baseline-test",
        "input_file_sha256": input_hash,
        "canonical_pcm_sha256": canonical_hash,
        "canonical_sample_rate_hz": 16_000,
        "canonical_channels": 1,
        "canonical_sample_format": "f32le",
        "canonical_sample_count": sample_count,
    }
    (root / FILES["contract"]).write_text(json.dumps(contract), encoding="utf-8")

    capture = struct.pack("<4f", 0.25, -0.25, 0.5, -0.5)
    (root / FILES["capture"]).write_bytes(capture)
    inference_hash = sha256(capture)
    trace_rows = [
        {
            "record_type": "vad_window",
            "value": {
                "sequence": 0,
                "source_sample_start": 0,
                "source_sample_end": 4,
                "vad_decision": "speech_emitted",
                "vad_exclusion_reason": None,
                "emitted_chunk_ids": [7],
                "observed_at_monotonic_ns": 1,
            },
        },
        {
            "record_type": "chunk",
            "value": {
                "chunk_id": 7,
                "source_chunk_ids": [7],
                "source_sample_start": 0,
                "source_sample_end": 4,
                "sample_rate_hz": 16_000,
                "overlap_samples": 0,
                "vad_decision": "speech_emitted",
                "vad_exclusion_reason": None,
                "enqueued_at_monotonic_ns": 2,
                "dequeued_at_monotonic_ns": 3,
                "inference_started_at_monotonic_ns": 4,
                "inference_finished_at_monotonic_ns": 5,
                "inference_batch_chunk_id": 7,
                "inference_sample_start": 0,
                "inference_sample_end": 4,
                "inference_sample_count": 4,
                "inference_pcm_sha256": inference_hash,
                "transcription_provider": "whisper",
                "transcription_model": "test-model",
                "final_writeback_at_monotonic_ns": 6,
                "text_before_dedup": "raw",
                "text_after_dedup": "clean",
                "text_after_context_normalization": "clean",
                "dedup_trace_status": "captured_whisper_engine",
                "actual_language": "zh",
                "language_applied_to_provider": True,
                "context_version_id": None,
                "context_sha256": None,
                "coalesced_into_chunk_id": None,
                "writeback_result": "written",
                "error": None,
            },
        },
    ]
    load_rows = [{
        "sequence": 1,
        "monotonic_ns": 1,
        "system_cpu_percent": 10.0,
        "system_memory_used_bytes": 100,
        "system_memory_total_bytes": 200,
        "system_unavailable_reason": None,
        "process_cpu_percent": 5.0,
        "process_memory_used_bytes": 50,
        "process_unavailable_reason": None,
        "gpu_percent": None,
        "gpu_memory_used_bytes": None,
        "gpu_unavailable_reason": "not exposed",
    }]
    stop_rows = [
        {"sequence": index + 2, "stage": stage, "result": "completed", "monotonic_ns": index + 7, "error": None}
        for index, stage in enumerate(("recording_started", "stop_requested", "queue_drained", "measurement_finalized"))
    ]
    trace = write_jsonl(root / FILES["trace"], trace_rows)
    load = write_jsonl(root / FILES["load"], load_rows)
    stop = write_jsonl(root / FILES["stop"], stop_rows)
    summary = {
        "schema_version": 1,
        "recording_id": "recording-test",
        "audio_input_path_id": audio_input_path_id,
        "baseline_snapshot_id": "baseline-test",
        "isolated_data_identity": "d11-test",
        "transcription_engine": {"provider": "whisper", "model": "test-model"},
        "measurement_started_at_monotonic_ns": 0,
        "measurement_finalized_at_monotonic_ns": 10,
        "capture_pcm_sha256": sha256(capture),
        "capture_pcm": file_record(root / FILES["capture"], capture),
        "capture_pcm_sample_count": 4,
        "capture_pcm_sample_rate_hz": 16_000,
        "trace": file_record(root / FILES["trace"], trace),
        "load_timeline": file_record(root / FILES["load"], load),
        "load_timeline_sha256": sha256(load),
        "stop_save_timeline": file_record(root / FILES["stop"], stop),
        "chunk_count": 1,
        "vad_window_count": 1,
        "load_sample_count": 1,
        "stop_save_event_count": 4,
        "errors": [],
    }
    (root / FILES["summary"]).write_text(json.dumps(summary), encoding="utf-8")


class D11MeasurementValidatorTests(unittest.TestCase):
    def test_valid_measurement_passes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="d11-validator-") as value:
            root = Path(value)
            make_fixture(root)
            self.assertEqual(validate(root)["status"], "PASS")

    def test_tampered_load_timeline_fails_hash_check(self) -> None:
        with tempfile.TemporaryDirectory(prefix="d11-validator-") as value:
            root = Path(value)
            make_fixture(root)
            with (root / FILES["load"]).open("ab") as handle:
                handle.write(b"{}\n")
            report = validate(root)
            self.assertEqual(report["status"], "FAIL")
            self.assertIn("load hash record mismatch", report["failures"])

    def test_missing_chunk_field_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="d11-validator-") as value:
            root = Path(value)
            make_fixture(root)
            rows, _ = __import__("validate_d11_realtime_measurement").read_jsonl(root / FILES["trace"])
            del rows[1]["value"]["text_before_dedup"]
            write_jsonl(root / FILES["trace"], rows)
            report = validate(root)
            self.assertEqual(report["status"], "FAIL")
            self.assertTrue(any("missing text_before_dedup" in item for item in report["failures"]))


if __name__ == "__main__":
    unittest.main()
