#!/usr/bin/env python3
from __future__ import annotations

import copy
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("moss_r3_bind_monolithic_score.py")
SPEC = importlib.util.spec_from_file_location("moss_r3_binding_under_test", MODULE_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("unable_to_import_moss_r3_binding")
binding = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(binding)


def write_json(path: Path, payload: object) -> None:
    path.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")


class R3MonolithicBindingTests(unittest.TestCase):
    def setUp(self) -> None:
        scratch_root = (
            Path.cwd() / "target" / "release" / "scratch" / "moss-r3-test-tmp"
        )
        scratch_root.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=scratch_root)
        self.root = Path(self.temp.name)
        self.file_a = self.root / "a.txt"
        self.file_a.write_text("frozen-a", encoding="utf-8")
        self.old_moss = self.root / "old.json"
        write_json(self.old_moss, {"old": True})
        self.role_files: dict[str, Path] = {}
        for index, role in enumerate(
            sorted(binding.SCORING_REQUIRED_ROLES - {binding.MOSS_ROLE})
        ):
            role_path = self.root / f"scoring-{index}.txt"
            role_path.write_text(f"frozen-{role}", encoding="utf-8")
            self.role_files[role] = role_path
        self.closeout_plan = self.root / "closeout-plan.md"
        self.closeout_plan.write_text("frozen-plan", encoding="utf-8")
        self.new_moss = self.root / "new.json"
        self.adapter = self.make_adapter()
        write_json(self.new_moss, self.adapter)
        self.new_moss_sha256 = binding.sha256_file(self.new_moss)
        self.manifest_path = self.root / "source-manifest.json"
        self.manifest = self.make_manifest()
        write_json(self.manifest_path, self.manifest)

    def tearDown(self) -> None:
        self.temp.cleanup()

    def make_adapter(self) -> dict:
        speakers = ["S01", "S02", "S03", "S04", "S05"]
        turns = []
        for index in range(binding.EXPECTED_TURN_COUNT):
            turns.append(
                {
                    "chunk_index": 0,
                    "global_start_ms": index * 1_000,
                    "global_end_ms": index * 1_000 + 900,
                    "speaker_label": speakers[index % len(speakers)],
                    "text": f"private-{index}",
                }
            )
        return {
            "schema_version": 1,
            "stage": binding.EXPECTED_STAGE,
            "run_state": "COMPLETED",
            "release_go": False,
            "cross_chunk_speaker_identity_proven": True,
            "global_turns": turns,
            "global_gate": {
                "state": "PASS",
                "total_rtf": 0.5,
                "source_structural_status": "PASS",
            },
            "source_binding": {
                "model_sha256": "a" * 64,
                "audio_sha256": "b" * 64,
                "configuration": {"n_ctx": 16_384, "backend": "vulkan"},
            },
        }

    def make_entry(self, role: str, path: Path) -> dict:
        return {
            "role": role,
            "path": str(path),
            "bytes": path.stat().st_size,
            "sha256": binding.sha256_file(path),
            "sensitive": role == binding.MOSS_ROLE,
            "storage": "REFERENCE",
        }

    def make_manifest(self) -> dict:
        entries = [self.make_entry("frozen_a", self.file_a)]
        entries.extend(
            self.make_entry(role, path)
            for role, path in sorted(self.role_files.items())
        )
        entries.append(self.make_entry("closeout_plan", self.closeout_plan))
        entries.append(self.make_entry(binding.MOSS_ROLE, self.old_moss))
        return {
            "schema_version": 1,
            "role": "TEST",
            "entry_count": len(entries),
            "entries": entries,
        }

    def run_binding(self, *, expected_hash: str | None = None) -> dict:
        return binding.derive_binding(
            source_manifest_path=self.manifest_path,
            moss_output_path=self.new_moss,
            expected_moss_sha256=expected_hash or self.new_moss_sha256,
            private_manifest_path=self.root / "private" / "MANIFEST.json",
            private_derivation_path=self.root / "private" / "DERIVATION.json",
            public_binding_path=self.root / "public" / "binding.json",
        )

    def test_good_binding_changes_only_moss_role_and_public_report_has_no_text(self) -> None:
        result = self.run_binding()
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(result["changed_role_count"], 1)
        self.assertEqual(result["unchanged_role_count"], len(self.manifest["entries"]) - 1)

        derived = binding.load_json(self.root / "private" / "MANIFEST.json")
        original_by_role = {item["role"]: item for item in self.manifest["entries"]}
        derived_by_role = {item["role"]: item for item in derived["entries"]}
        for role, original in original_by_role.items():
            if role != binding.MOSS_ROLE:
                self.assertEqual(derived_by_role[role], original)
        self.assertNotEqual(
            derived_by_role[binding.MOSS_ROLE], original_by_role[binding.MOSS_ROLE]
        )
        self.assertEqual(
            derived_by_role[binding.MOSS_ROLE]["sha256"], self.new_moss_sha256
        )

        public_text = (self.root / "public" / "binding.json").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("private-", public_text)
        self.assertNotIn(str(self.root), public_text)
        public = json.loads(public_text)
        self.assertFalse(public["transcript_text_included"])
        self.assertFalse(public["absolute_paths_included"])

    def test_missing_and_duplicate_moss_role_are_rejected(self) -> None:
        missing = copy.deepcopy(self.manifest)
        missing["entries"] = [missing["entries"][0]]
        missing["entry_count"] = 1
        with self.assertRaisesRegex(ValueError, "requires_exactly_one_role"):
            binding.validate_manifest(missing)

        duplicate = copy.deepcopy(self.manifest)
        duplicate["entries"].append(copy.deepcopy(duplicate["entries"][1]))
        duplicate["entry_count"] = len(duplicate["entries"])
        with self.assertRaisesRegex(ValueError, "duplicate_roles"):
            binding.validate_manifest(duplicate)

    def test_wrong_hash_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "moss_output_hash_mismatch"):
            self.run_binding(expected_hash="0" * 64)

    def test_wrong_stage_is_rejected(self) -> None:
        self.adapter["stage"] = "P1_CHUNKED"
        write_json(self.new_moss, self.adapter)
        changed_hash = binding.sha256_file(self.new_moss)
        with self.assertRaisesRegex(ValueError, "moss_output_stage_mismatch"):
            self.run_binding(expected_hash=changed_hash)

    def test_nonzero_chunk_is_rejected(self) -> None:
        self.adapter["global_turns"][0]["chunk_index"] = 1
        write_json(self.new_moss, self.adapter)
        changed_hash = binding.sha256_file(self.new_moss)
        with self.assertRaisesRegex(ValueError, "not_single_chunk_zero"):
            self.run_binding(expected_hash=changed_hash)

    def test_source_manifest_hash_mismatch_is_rejected(self) -> None:
        scoring_path = self.role_files["scoring_rules_draft"]
        scoring_path.write_text("tampered", encoding="utf-8")
        with self.assertRaisesRegex(
            ValueError, "source_manifest_scoring_input_rehash_failed:scoring_rules_draft"
        ):
            self.run_binding()

    def test_closeout_plan_drift_is_recorded_but_does_not_change_score_inputs(self) -> None:
        self.closeout_plan.write_text("later execution results", encoding="utf-8")
        result = self.run_binding()
        self.assertEqual(
            result["status"], "PASS_WITH_RECORDED_NON_SCORING_DOCUMENT_DRIFT"
        )
        public = binding.load_json(self.root / "public" / "binding.json")
        self.assertEqual(
            public["source_manifest_rehash"][
                "recorded_non_scoring_drift_roles"
            ],
            ["closeout_plan"],
        )
        self.assertTrue(
            public["source_manifest_rehash"]["all_f3_f4_scoring_inputs_match"]
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
