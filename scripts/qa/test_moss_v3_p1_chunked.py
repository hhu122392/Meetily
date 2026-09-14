#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import math
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("moss_v3_p1_chunked.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p1_chunked_under_test", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable_to_import_chunked_runner")
chunked = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(chunked)


class ChunkBoundaryTests(unittest.TestCase):
    def test_business_and_monthly_samples_use_the_measured_safe_bound(self) -> None:
        for duration in (737.728, 900.02):
            minimum = chunked.effective_min_chunk_seconds(duration)
            plan = chunked.choose_chunk_plan(
                duration, [], min_chunk_seconds=minimum
            )
            self.assertEqual(
                len(plan), math.ceil(duration / chunked.MAX_CHUNK_SECONDS)
            )
            self.assertTrue(
                all(
                    minimum <= item["duration_seconds"] <= chunked.MAX_CHUNK_SECONDS
                    for item in plan
                )
            )

    def test_prefers_long_silence_center_inside_feasible_window(self) -> None:
        plan = chunked.choose_chunk_plan(
            700.0,
            [
                {
                    "start_seconds": 344.0,
                    "end_seconds": 356.0,
                    "duration_seconds": 12.0,
                }
            ],
        )
        self.assertEqual(len(plan), 2)
        self.assertEqual(plan[0]["end_boundary"]["source"], "silence_center")
        self.assertAlmostEqual(plan[0]["end_seconds"], 350.0)
        self.assertAlmostEqual(plan[1]["duration_seconds"], 350.0)

    def test_uses_deterministic_balanced_split_without_suitable_silence(self) -> None:
        plan = chunked.choose_chunk_plan(700.0, [])
        self.assertEqual(len(plan), 2)
        self.assertEqual(plan[0]["end_boundary"]["source"], "fixed_balanced")
        self.assertAlmostEqual(plan[0]["end_seconds"], 350.0)
        self.assertAlmostEqual(plan[1]["duration_seconds"], 350.0)


class OffsetAndSpeakerScopeTests(unittest.TestCase):
    @staticmethod
    def output(label: str = "S01") -> dict:
        return {
            "raw_turns": [
                {
                    "start_seconds": 0.1,
                    "end_seconds": 0.9,
                    "speaker_label": label,
                    "text": "private transcript text",
                }
            ]
        }

    def test_offsets_local_timestamps_to_global_timeline(self) -> None:
        turns = chunked.offset_chunk_output(self.output(), 2, 500.0)
        self.assertEqual(turns[0]["local_start_ms"], 100)
        self.assertEqual(turns[0]["local_end_ms"], 900)
        self.assertEqual(turns[0]["global_start_ms"], 500_100)
        self.assertEqual(turns[0]["global_end_ms"], 500_900)

    def test_same_local_label_is_isolated_between_chunks(self) -> None:
        first = chunked.offset_chunk_output(self.output("S01"), 1, 0.0)[0]
        second = chunked.offset_chunk_output(self.output("S01"), 2, 500.0)[0]
        self.assertEqual(first["speaker_label"], "C001:S01")
        self.assertEqual(second["speaker_label"], "C002:S01")
        self.assertNotEqual(first["speaker_label"], second["speaker_label"])
        self.assertEqual(first["speaker_identity_scope"], "chunk_only")
        self.assertEqual(second["speaker_identity_scope"], "chunk_only")


class GlobalValidationTests(unittest.TestCase):
    def test_rejects_global_missing_active_segment(self) -> None:
        chunks = [
            {
                "chunk_index": 1,
                "start_seconds": 0.0,
                "end_seconds": 350.0,
                "state": "PASS",
                "terminal_status": "COMPLETED",
            },
            {
                "chunk_index": 2,
                "start_seconds": 350.0,
                "end_seconds": 700.0,
                "state": "PASS",
                "terminal_status": "COMPLETED",
            },
        ]
        turns = [
            {
                "chunk_index": 1,
                "global_start_ms": 0,
                "global_end_ms": 325_000,
                "speaker_label": "C001:S01",
            },
            {
                "chunk_index": 2,
                "global_start_ms": 375_000,
                "global_end_ms": 700_000,
                "speaker_label": "C002:S01",
            },
        ]
        activity = {
            "significant_active_intervals": [
                {
                    "start_seconds": 0.0,
                    "end_seconds": 700.0,
                    "duration_seconds": 700.0,
                }
            ]
        }
        with self.assertRaisesRegex(
            chunked.GateError, "global_active_audio_gap_exceeds_threshold"
        ):
            chunked.validate_global_output(chunks, turns, activity, 700.0, 140.0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
