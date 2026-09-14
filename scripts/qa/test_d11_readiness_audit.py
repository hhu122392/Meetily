#!/usr/bin/env python3
"""Contract tests for the D-11 read-only readiness audit."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("d11_readiness_audit.py")
SPEC = importlib.util.spec_from_file_location("d11_readiness_audit", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load d11_readiness_audit")
AUDIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AUDIT)
REPO = Path(__file__).resolve().parents[2]


class D11ReadinessAuditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.result = AUDIT.collect_audit(REPO)

    def test_frozen_checked_in_records_match_q00_constants(self) -> None:
        frozen = self.result["frozen_q00_registration"]
        self.assertTrue(all(frozen["checks"].values()))
        self.assertEqual(frozen["window"]["duration_seconds"], 226.440)
        self.assertEqual(
            frozen["window"]["sha256"],
            "5128D52F41A6FA58BBD9387C6C68D09E076E9E782C8C13B8BF0CF5BB8655AD22",
        )
        self.assertEqual(
            frozen["positive_terms_declared_by_q00_code"],
            ["YouTube", "PWA", "Google"],
        )
        self.assertEqual(
            frozen["negative_terms_rehashed_from_checked_in_truth"],
            ["M100", "H5", "VIP", "A/B Test", "TG"],
        )

    def test_q00_is_import_not_formal_realtime_injection(self) -> None:
        contract = self.result["formal_realtime_input_contract"]
        self.assertEqual(contract["status"], "FAIL_MISSING")
        self.assertTrue(contract["q00_uses_import_audio"])
        self.assertFalse(contract["q00_uses_live_recording_input"])
        self.assertEqual(contract["audio_input_path_id_occurrences_outside_plans"], [])

    def test_required_trace_is_not_overclaimed(self) -> None:
        trace = self.result["traceability"]
        self.assertEqual(trace["status"], "FAIL_INCOMPLETE")
        self.assertTrue(trace["audio_chunk_has_chunk_id"])
        self.assertFalse(trace["transcript_update_has_chunk_id"])
        self.assertEqual(trace["fully_available_field_count"], 0)
        self.assertTrue(all(row["availability"].endswith("FAIL") for row in trace["fields"]))

    def test_no_baseline_or_attribution_is_fabricated(self) -> None:
        baseline = self.result["baseline_execution"]
        self.assertEqual(baseline["status"], "NOT_RUN_CONTRACT_BLOCKED")
        self.assertEqual(baseline["cold_run_count"], 0)
        self.assertEqual(baseline["warm_run_count"], 0)
        self.assertEqual(baseline["attribution_claims_made"], 0)
        self.assertFalse(self.result["ready_for_three_run_baseline"])

    def test_output_guard_rejects_path_outside_evidence_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            outside = Path(directory) / "audit.json"
            with self.assertRaises(AUDIT.AuditError):
                AUDIT._validated_output_path(REPO, outside)

    def test_result_is_json_serializable(self) -> None:
        payload = json.dumps(self.result, ensure_ascii=False, sort_keys=True)
        self.assertIn("D11_REALTIME_BASELINE_READINESS_AUDIT", payload)


if __name__ == "__main__":
    unittest.main()
