#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("moss_v3_r1_monolithic_32k.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_r1_monolithic_under_test", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable_to_import_r1_monolithic_runner")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class R1BoundedSessionTests(unittest.TestCase):
    def test_context_is_fixed_to_stable_chunked_value(self) -> None:
        self.assertEqual(runner.N_CTX, 32_768)
        self.assertNotEqual(runner.N_CTX, 131_072)
        self.assertEqual(runner.KV_TYPE, "auto")

    def test_private_output_must_stay_under_fixed_d_drive_root(self) -> None:
        accepted = runner.PRIVATE_ROOT / "case" / "output.private.json"
        self.assertEqual(runner.validate_private_output_path(accepted), accepted.resolve())
        with self.assertRaisesRegex(RuntimeError, "outside_fixed_root"):
            runner.validate_private_output_path(Path(r"D:\MeetilyData\outside.private.json"))

    def test_public_payload_never_contains_transcript_text(self) -> None:
        result = {
            "text": "SENSITIVE_TRANSCRIPT_SENTINEL",
            "raw_turns": [
                {
                    "start_seconds": 1.0,
                    "end_seconds": 2.0,
                    "speaker_label": "S01",
                    "text": "SENSITIVE_TRANSCRIPT_SENTINEL",
                }
            ],
        }
        payload = runner.make_public_payload(
            status="COMPLETED",
            started_at="2026-08-31T00:00:00+00:00",
            finished_at="2026-08-31T00:01:00+00:00",
            wall_seconds=60.0,
            model={"bytes": 1, "sha256": "a" * 64},
            source={"bytes": 2, "sha256": "b" * 64},
            prepared={"duration_seconds": 10.0},
            session_limits={"effective_n_ctx": 32768},
            admission={"status": "PASS"},
            result_record=result,
            output_gate={"state": "PASS"},
            memory=None,
            private_path=None,
            private_sha256=None,
            error=None,
        )
        rendered = json.dumps(payload, ensure_ascii=False)
        self.assertNotIn("SENSITIVE_TRANSCRIPT_SENTINEL", rendered)
        self.assertFalse(payload["transcript_text_included"])
        self.assertEqual(payload["output_summary"]["turn_count"], 1)
        self.assertEqual(payload["output_summary"]["speaker_label_count"], 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
