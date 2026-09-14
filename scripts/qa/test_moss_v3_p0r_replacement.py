#!/usr/bin/env python3

import importlib.util
from pathlib import Path
import unittest


MODULE_PATH = Path(__file__).with_name("moss_v3_p0r_replacement.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p0r_replacement", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ClipSegmentsTests(unittest.TestCase):
    def test_source_audio_offset_is_separate_from_candidate_timeline(self) -> None:
        self.assertEqual(MODULE.CANDIDATE_START_SECONDS, 0.0)
        self.assertEqual(MODULE.CANDIDATE_END_SECONDS, 195.0)
        self.assertEqual(MODULE.SOURCE_AUDIO_START_SECONDS, 415.44)
        self.assertEqual(MODULE.SOURCE_AUDIO_END_SECONDS, 610.44)

    def test_excludes_unresolvable_segment_at_or_after_195_seconds(self) -> None:
        segments = [
            {"row": 1, "start_seconds": 190.0, "end_seconds": 194.0},
            {"row": 2, "start_seconds": 196.14, "end_seconds": 210.74},
        ]
        result = MODULE.clip_segments(segments)
        self.assertEqual([item["row"] for item in result], [1])

    def test_clips_crossing_end_boundary_and_marks_machine_only(self) -> None:
        segments = [
            {
                "row": 1,
                "start_seconds": 194.0,
                "end_seconds": 196.0,
                "eligible_as_ground_truth": True,
            }
        ]
        result = MODULE.clip_segments(segments)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0]["end_seconds"], 195.0)
        self.assertTrue(result[0]["boundary_clipped"])
        self.assertFalse(result[0]["eligible_as_ground_truth"])

    def test_supports_whisper_start_end_fields(self) -> None:
        result = MODULE.clip_segments([{"start": 1.0, "end": 2.0, "text": "x"}])
        self.assertEqual(result[0]["start"], 1.0)
        self.assertEqual(result[0]["end"], 2.0)
        self.assertEqual(result[0]["source_start_seconds"], 1.0)

    def test_supports_frozen_whisper_clip_fields(self) -> None:
        result = MODULE.clip_segments(
            [{"clip_start_seconds": 194.0, "clip_end_seconds": 196.0, "text": "x"}]
        )
        self.assertEqual(result[0]["clip_start_seconds"], 194.0)
        self.assertEqual(result[0]["clip_end_seconds"], 195.0)
        self.assertTrue(result[0]["boundary_clipped"])


if __name__ == "__main__":
    unittest.main()
