from __future__ import annotations

import importlib.util
import csv
import io
import json
from pathlib import Path
import sys
import unittest


SCRIPT = Path(__file__).with_name("moss_v3_p0r_balanced_window.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p0r_balanced_window", SCRIPT)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BalancedWindowTests(unittest.TestCase):
    def test_window_is_continuous_and_policy_sized(self) -> None:
        self.assertAlmostEqual(MODULE.SOURCE_START_SECONDS, 70.370, places=3)
        self.assertAlmostEqual(MODULE.SOURCE_END_SECONDS, 296.810, places=3)
        self.assertAlmostEqual(MODULE.SOURCE_END_SECONDS - MODULE.SOURCE_START_SECONDS, 226.440, places=3)
        self.assertGreaterEqual(MODULE.WINDOW_DURATION_SECONDS, 180.0)
        self.assertLessEqual(MODULE.WINDOW_DURATION_SECONDS, 300.0)
        self.assertAlmostEqual(MODULE.MINIMUM_SPEECH_SECONDS, 192.474, places=3)

    def test_real_candidate_window_exceeds_frozen_speech_gate(self) -> None:
        root = Path(__file__).resolve().parents[2]
        candidate_path = (
            root
            / "target"
            / "release"
            / "docs"
            / "方案"
            / "证据"
            / "MOSS-V3-P0R-FULL-TRUTH-20260830"
            / "03-machine-candidate-full.json"
        )
        payload = json.loads(candidate_path.read_text(encoding="utf-8"))
        clipped = MODULE.clip_machine_segments(payload["segments"])
        speech = MODULE.interval_union_seconds(
            (item["clip_start_seconds"], item["clip_end_seconds"])
            for item in clipped
        )
        self.assertGreaterEqual(speech, MODULE.MINIMUM_SPEECH_SECONDS)
        self.assertEqual(
            {item["reference_speaker_candidate"] for item in clipped},
            {"H01", "H02"},
        )
        self.assertTrue(all(item["is_ground_truth"] is False for item in clipped))

    def test_initial_rows_cover_full_timeline_and_remain_unchecked(self) -> None:
        fixture = [
            {
                "clip_start_seconds": 1.0,
                "clip_end_seconds": 4.0,
                "reference_speaker_candidate": "H01",
                "review_prefill_text": "甲",
                "source_sequence_id": "one",
                "boundary_clipped_start": False,
                "boundary_clipped_end": False,
            },
            {
                "clip_start_seconds": 3.5,
                "clip_end_seconds": 5.0,
                "reference_speaker_candidate": "H02",
                "review_prefill_text": "乙",
                "source_sequence_id": "two",
                "boundary_clipped_start": False,
                "boundary_clipped_end": False,
            },
        ]
        rows = MODULE.build_initial_rows(fixture)
        self.assertEqual(rows[0]["start"], 0.0)
        self.assertEqual(rows[-1]["end"], MODULE.WINDOW_DURATION_SECONDS)
        self.assertTrue(all(row["human_checked"] is False for row in rows))
        speech = [row for row in rows if not row["non_speech"]]
        self.assertTrue(all(row["overlap"] is True for row in speech))

    def test_boundary_clipped_machine_text_is_not_prefilled(self) -> None:
        rows = MODULE.build_initial_rows(
            [
                {
                    "clip_start_seconds": 0.0,
                    "clip_end_seconds": 2.0,
                    "reference_speaker_candidate": "H01",
                    "review_prefill_text": "",
                    "source_sequence_id": "clipped",
                    "boundary_clipped_start": True,
                    "boundary_clipped_end": False,
                }
            ]
        )
        self.assertEqual(rows[0]["text"], "")
        self.assertIn("禁止预填", rows[0]["note"])

    def test_full_playback_coverage_has_zero_maximum_gap(self) -> None:
        root = Path(__file__).resolve().parents[2]
        kit = (
            root
            / "target"
            / "release"
            / "docs"
            / "方案"
            / "证据"
            / "MOSS-V3-P0R-FULL-TRUTH-20260830"
        )
        sys.path.insert(0, str(kit))
        spec = importlib.util.spec_from_file_location(
            "dense_window_human_review_server_coverage", kit / "human_review_server.py"
        )
        assert spec and spec.loader
        server = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = server
        spec.loader.exec_module(server)

        metrics = server.coverage_metrics([(0.0, 180.0)], 180.0)

        self.assertTrue(metrics["complete"])
        self.assertEqual(metrics["maximum_gap_seconds"], 0.0)

    def test_partial_overlap_only_removes_real_intersection(self) -> None:
        root = Path(__file__).resolve().parents[2]
        kit = (
            root
            / "target"
            / "release"
            / "docs"
            / "方案"
            / "证据"
            / "MOSS-V3-P0R-FULL-TRUTH-20260830"
        )
        sys.path.insert(0, str(kit))
        spec = importlib.util.spec_from_file_location(
            "dense_window_human_review_server", kit / "human_review_server.py"
        )
        assert spec and spec.loader
        server = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = server
        spec.loader.exec_module(server)
        rows = [
            {
                "start": 0.0,
                "end": 1.0,
                "speaker": "H01",
                "text": "甲",
                "overlap": True,
                "non_speech": False,
                "note": "真人确认",
            },
            {
                "start": 0.9,
                "end": 10.0,
                "speaker": "H02",
                "text": "乙",
                "overlap": True,
                "non_speech": False,
                "note": "真人确认",
            },
        ]
        _verbatim, turns, counts = server.build_tsv_documents(rows)
        parsed = list(csv.DictReader(io.StringIO(turns.decode("utf-8")), delimiter="\t"))
        self.assertEqual(
            [(row["start_ms"], row["end_ms"], row["reference_speaker_id"]) for row in parsed],
            [("0", "900", "H01"), ("1000", "10000", "H02")],
        )
        self.assertEqual(counts["valid_speaker_turn_count"], 2)


if __name__ == "__main__":
    unittest.main()
