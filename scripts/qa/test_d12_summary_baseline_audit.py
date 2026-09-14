import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("d12_summary_baseline_audit.py")
SPEC = importlib.util.spec_from_file_location("d12_summary_baseline_audit", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class D12SummaryBaselineAuditTests(unittest.TestCase):
    def write_isolation_marker(self, root: Path) -> None:
        (root / MODULE.ISOLATION_MARKER).write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "purpose": MODULE.ISOLATION_PURPOSE,
                    "isolated_data_identity": "d12-test",
                }
            ),
            encoding="utf-8",
        )

    def test_peak_ceiling_uses_fixed_formula(self):
        physical = 32 * 1024**3
        commit_limit = 48 * 1024**3
        non_moss_peak = 20 * 1024**3
        expected = min(physical * 70 // 100, commit_limit - non_moss_peak - 2 * 1024**3)
        self.assertEqual(
            MODULE.calculate_peak_commit_ceiling(physical, commit_limit, non_moss_peak),
            expected,
        )

    def test_peak_ceiling_stays_missing_without_real_baseline_peak(self):
        self.assertIsNone(MODULE.calculate_peak_commit_ceiling(32 * 1024**3, 48 * 1024**3, None))

    def test_asset_is_only_frozen_after_existing_bytes_are_hashed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "rule.json"
            path.write_text('{"rule":"fixed"}\n', encoding="utf-8")
            digest = MODULE.sha256_file(path)
            record = MODULE.asset_record(
                role="rule",
                path=path,
                expected_bytes=path.stat().st_size,
                expected_sha256=digest,
                note="test",
            )
            self.assertEqual(record["freeze_status"], "FROZEN_VERIFIED")
            self.assertTrue(record["sha256_match"])

    def test_missing_asset_remains_pending(self):
        record = MODULE.asset_record(role="missing", path=None, note="test")
        self.assertEqual(record["freeze_status"], "PENDING")
        self.assertIsNone(record["actual_sha256"])

    def test_private_asset_requires_isolated_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            asset = root / "asset.txt"
            asset.write_text("private", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "--asset-root is required"):
                MODULE.validate_isolated_asset_boundary(None, root / "evidence", [asset])

    def test_isolated_boundary_accepts_marked_root_and_rejects_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "isolated"
            root.mkdir()
            self.write_isolation_marker(root)
            asset = root / "asset.txt"
            asset.write_text("private", encoding="utf-8")
            boundary = MODULE.validate_isolated_asset_boundary(
                root, root / "evidence", [asset]
            )
            self.assertEqual(boundary["isolated_data_identity"], "d12-test")
            with self.assertRaisesRegex(ValueError, "escapes isolated asset root"):
                MODULE.validate_isolated_asset_boundary(
                    root, root / "evidence", [Path(directory) / "outside.txt"]
                )

    def test_isolated_boundary_requires_valid_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            asset = root / "asset.txt"
            asset.write_text("private", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "isolation marker missing"):
                MODULE.validate_isolated_asset_boundary(root, root / "evidence", [asset])

    def test_protected_user_data_path_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "com.meetily.ai" / "isolated"
            root.mkdir(parents=True)
            self.write_isolation_marker(root)
            asset = root / "asset.txt"
            asset.write_text("private", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "protected com.meetily.ai"):
                MODULE.validate_isolated_asset_boundary(root, root / "evidence", [asset])

    def test_incomplete_baseline_run_cannot_be_counted_as_valid(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "run.json"
            path.write_text(
                json.dumps(
                    {
                        "baseline_snapshot_id": "baseline-1",
                        "generation_id": "gen-1",
                        "timed_endpoint_id": MODULE.TIMED_ENDPOINT_ID,
                        "click_at": "2026-09-06T00:00:00+08:00",
                        "page_display_complete_at": "2026-09-06T00:01:00+08:00",
                        "load_timeline_sha256": "A" * 64,
                        "baseline_non_moss_commit_peak_bytes": 123,
                    }
                ),
                encoding="utf-8",
            )
            result = MODULE.load_baseline_run(path)
            self.assertFalse(result["valid"])
            self.assertIn("missing:stage_timings", result["errors"])
            self.assertIn("missing:page_displayed_body_sha256", result["errors"])

    def test_checked_out_source_has_flows_but_not_required_timing_contract(self):
        repo = SCRIPT.parents[2]
        audit = MODULE.audit_source(repo, "test", "test")
        self.assertTrue(all(item["flow_present"] for item in audit["stage_audit"]))
        self.assertTrue(
            all(
                not item["independent_monotonic_timing_point_present"]
                for item in audit["stage_audit"]
            )
        )
        self.assertEqual(audit["conclusion"]["status"], "BLOCKED")

    def test_generated_report_uses_single_newlines(self):
        manifest = {
            "generated_at": "2026-09-06T00:00:00+08:00",
            "source": {
                "starting_branch": "codex/d10a-evidence-20260906",
                "starting_commit": "4f4737575fff220ed92c2ed8f4ac56994749733e",
            },
            "machine_memory": {
                "physical_memory_bytes": 1,
                "commit_limit_bytes": 2,
                "baseline_non_moss_commit_peak_bytes": None,
                "system_reserve_bytes": MODULE.RESERVE_BYTES,
                "fixed_formula": "fixed",
                "peak_commit_ceiling_bytes": None,
            },
            "assets": [
                {"role": "present", "freeze_status": "FROZEN_VERIFIED"},
                {"role": "missing", "freeze_status": "PENDING"},
            ],
        }
        audit = {
            "stage_audit": [
                {"stage_id": "save", "independent_monotonic_timing_point_present": False}
            ]
        }
        report = MODULE.build_report(manifest, audit)
        self.assertNotIn("\r", report)


if __name__ == "__main__":
    unittest.main()
