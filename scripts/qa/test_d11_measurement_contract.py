#!/usr/bin/env python3
"""Source-level contract tests for the D-11 measurement capability checkpoint."""

from __future__ import annotations

from pathlib import Path
import re
import unittest


REPO = Path(__file__).resolve().parents[2]
MEASUREMENT = REPO / "frontend/src-tauri/src/audio/measurement.rs"
MOD = REPO / "frontend/src-tauri/src/audio/mod.rs"
LIB = REPO / "frontend/src-tauri/src/lib.rs"
PIPELINE = REPO / "frontend/src-tauri/src/audio/pipeline.rs"
WORKER = REPO / "frontend/src-tauri/src/audio/transcription/worker.rs"
COMMANDS = REPO / "frontend/src-tauri/src/audio/recording_commands.rs"
SAVER = REPO / "frontend/src-tauri/src/audio/recording_saver.rs"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


class D11MeasurementContractTests(unittest.TestCase):
    def test_measurement_module_defines_reproducible_isolated_input_contract(self) -> None:
        source = read(MEASUREMENT)
        for token in (
            "pub struct RealtimeAudioInputContract",
            "pub audio_input_path_id: String",
            "pub input_file_sha256: String",
            "pub canonical_pcm_sha256: String",
            "pub isolated_data_identity: String",
            "pub transcription_engine: Option<TranscriptionEngineIdentity>",
            "pub measurement_started_at_monotonic_ns: u64",
            "pub measurement_finalized_at_monotonic_ns: u64",
            "ISOLATION_MARKER_FILE",
            "refusing formal application data root",
        ):
            self.assertIn(token, source)

    def test_formal_controlled_realtime_command_is_registered(self) -> None:
        self.assertIn("pub mod measurement;", read(MOD))
        commands = read(COMMANDS)
        self.assertIn("pub async fn start_controlled_realtime_input", commands)
        self.assertIn("RealtimeAudioInputRequest", commands)
        self.assertIn("start_controlled_realtime_input", read(LIB))

    def test_chunk_trace_contract_covers_all_required_fields(self) -> None:
        combined = "\n".join((read(MEASUREMENT), read(PIPELINE), read(WORKER), read(COMMANDS)))
        for token in (
            "chunk_id",
            "source_sample_start",
            "source_sample_end",
            "overlap_samples",
            "vad_decision",
            "vad_exclusion_reason",
            "enqueued_at_monotonic_ns",
            "dequeued_at_monotonic_ns",
            "inference_started_at_monotonic_ns",
            "inference_finished_at_monotonic_ns",
            "inference_sample_start",
            "inference_sample_end",
            "final_writeback_at_monotonic_ns",
            "text_before_dedup",
            "text_after_dedup",
            "dedup_trace_status",
            "text_after_context_normalization",
            "actual_language",
            "context_version_id",
            "context_sha256",
            "inference_pcm_sha256",
            "transcription_provider",
            "transcription_model",
        ):
            self.assertIn(token, combined)

    def test_capture_hash_load_timeline_and_stop_timeline_are_persisted(self) -> None:
        combined = "\n".join((read(MEASUREMENT), read(WORKER), read(COMMANDS), read(SAVER)))
        for token in (
            "capture_pcm_sha256",
            "capture-pcm.f32le",
            "load_timeline_sha256",
            "process_cpu_percent",
            "process_memory_used_bytes",
            "gpu_unavailable_reason",
            "load-timeline.jsonl",
            "realtime-transcription-trace.jsonl",
            "stop_requested",
            "queue_drained",
            "checkpoint_merge_started",
            "checkpoint_merge_finished",
            "final_transcript_write_started",
            "final_transcript_write_finished",
            "metadata_completed",
            "recording_stopped_event_emitted",
        ):
            self.assertIn(token, combined)

    def test_measurement_stays_out_of_transcription_decisions(self) -> None:
        pipeline = read(PIPELINE)
        worker = read(WORKER)
        self.assertRegex(pipeline, r"fn live_segment_durations_ms\(\) -> \(u32, u32\) \{\s*\(3_500, 5_000\)")
        self.assertIn("let redemption_time = 1_200;", pipeline)
        self.assertIn("const MAX_WHISPER_BATCH_DURATION_SECONDS: f64 = 9.0;", worker)
        self.assertIn("const MAX_WHISPER_BATCH_GAP_SECONDS: f64 = 2.0;", worker)
        self.assertRegex(
            worker,
            re.compile(
                r"TranscriptionEngine::Whisper\(_\)\s*\|\s*TranscriptionEngine::Provider\(_\)\s*=>\s*0\.3",
                re.MULTILINE,
            ),
        )
        self.assertIn("TranscriptionEngine::Parakeet(_) => 0.0", worker)


if __name__ == "__main__":
    unittest.main()
