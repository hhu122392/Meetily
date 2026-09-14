from __future__ import annotations

import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from moss_v3_p6_common import BLOCKED, FAIL, NOT_RUN, PASS, GateError, read_json, sha256_file
from moss_v3_p6_release import (
    HUMAN_APPROVAL_ATTESTATION,
    REQUIRED_EVIDENCE_FILES,
    REQUIRED_METRIC_EVIDENCE_ROLES,
    REQUIRED_PACKAGE_ROLES,
    command_assets,
    command_build_wiring,
    command_evidence_manifest,
    command_final_audit,
    command_package,
    evaluate_release_asset_binding,
    evaluate_metrics,
    git_head,
    load_bound_report,
    validate_stage_gate,
    validate_release_dependency_chain,
)


REPO = Path(__file__).resolve().parents[2]


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


class ReleaseToolTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.head = git_head(REPO)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_evidence_json_rejects_duplicate_keys_and_nonfinite_numbers(self) -> None:
        malformed = self.root / "malformed.json"
        malformed.write_text('{"status":"PASS","status":"FAIL"}', encoding="utf-8")
        with self.assertRaises(GateError):
            read_json(malformed, "fixture")
        malformed.write_text('{"metric":NaN}', encoding="utf-8")
        with self.assertRaises(GateError):
            read_json(malformed, "fixture")

    def make_assets(self) -> SimpleNamespace:
        data = self.root / "data"
        model = data / "models" / "MOSS-Transcribe-Diarize-Q8_0.gguf"
        model.parent.mkdir(parents=True)
        model.write_bytes(b"model")
        model_license = data / "models" / "LICENSE-MOSS.txt"
        model_license.write_text("model-license", encoding="utf-8")
        source = data / "source"
        (source / "ggml").mkdir(parents=True)
        (source / "src/third_party/miniz").mkdir(parents=True)
        runtime_license = source / "LICENSE"
        runtime_license.write_text("runtime-license", encoding="utf-8")
        (source / "ggml/LICENSE").write_text("ggml-license", encoding="utf-8")
        (source / "src/third_party/miniz/LICENSE").write_text("miniz-license", encoding="utf-8")
        third_party = source / "THIRD-PARTY-LICENSES.md"
        third_party.write_text("third-party-index", encoding="utf-8")

        runtime = data / "runtime"
        (runtime / "licenses/ggml").mkdir(parents=True)
        (runtime / "licenses/src/third_party/miniz").mkdir(parents=True)
        (runtime / "licenses/LICENSE").write_bytes(runtime_license.read_bytes())
        (runtime / "licenses/ggml/LICENSE").write_bytes((source / "ggml/LICENSE").read_bytes())
        (runtime / "licenses/src/third_party/miniz/LICENSE").write_bytes(
            (source / "src/third_party/miniz/LICENSE").read_bytes()
        )
        contract = runtime / "contract.json"
        write_json(contract, {"version": "0.2.2", "backends": ["vulkan", "cpu"], "lane": "cpu-vulkan"})
        runtime_dll = runtime / "transcribe.dll"
        runtime_dll.write_bytes(b"runtime")
        lock = {
            "schema_version": 1,
            "stage": "MOSS_V3_P1",
            "model": {
                "repository": "example/model",
                "revision": "1" * 40,
                "file_name": model.name,
                "bytes": model.stat().st_size,
                "sha256": sha256_file(model),
                "local_license_bytes": model_license.stat().st_size,
                "local_license_sha256": sha256_file(model_license),
            },
            "runtime": {
                "project": "example/runtime",
                "version": "0.2.2",
                "source_commit": "2" * 40,
                "primary_backend": "Intel Arc Vulkan",
                "license_sha256": sha256_file(runtime_license),
                "third_party_licenses_sha256": sha256_file(third_party),
                "runtime_files": {
                    contract.name: sha256_file(contract),
                    runtime_dll.name: sha256_file(runtime_dll),
                },
            },
            "accuracy_status": "NOT_SCORABLE_UNTIL_P0_R_PASS",
        }
        lock_path = self.root / "lock.json"
        write_json(lock_path, lock)
        return SimpleNamespace(
            repo=REPO,
            lock=lock_path,
            data_root=data,
            deployment_root=r"D:\MeetilyData",
            system_drive="C:",
            model=model,
            model_license=model_license,
            runtime=runtime,
            runtime_license=runtime_license,
            runtime_third_party_license=third_party,
            output=self.root / "asset-report.json",
        )

    def test_assets_pass_and_detect_runtime_corruption(self) -> None:
        args = self.make_assets()
        self.assertEqual(command_assets(args), 0)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], PASS)
        self.assertEqual(report["runtime"]["file_count"], 2)
        self.assertEqual(len(report["runtime"]["bundled_licenses"]), 3)

        (args.runtime / "transcribe.dll").write_bytes(b"corrupt")
        self.assertEqual(command_assets(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], FAIL)
        self.assertFalse(report["checks"]["runtime_hashes_match_lock"])

    def make_package_spec(self) -> tuple[Path, dict[str, object]]:
        roots = {
            "package": (self.root / "package", "package", r"D:\Release"),
            "application": (self.root / "application", "application", r"C:\Program Files\Meetily"),
            "moss": (self.root / "moss", "moss_data", r"D:\MeetilyData"),
            "source": (self.root / "source", "source", r"SOURCE"),
            "evidence": (self.root / "evidence", "evidence", r"EVIDENCE"),
        }
        for path, _, _ in roots.values():
            path.mkdir(parents=True)
        role_root = {
            "installer": "package",
            "installer_signature": "package",
            "application": "application",
            "moss_helper": "application",
            "llama_helper": "application",
            "ffmpeg": "application",
            "moss_model": "moss",
            "moss_runtime": "moss",
            "model_license": "moss",
            "runtime_license": "moss",
            "third_party_licenses": "moss",
            "database_migration": "source",
            "release_notes": "evidence",
            "authenticode_report": "evidence",
        }
        entries = []
        for index, role in enumerate(sorted(REQUIRED_PACKAGE_ROLES)):
            root_id = role_root[role]
            suffix = ".gguf" if role == "moss_model" else ".bin"
            name = f"{index:02d}-{role}{suffix}"
            path = roots[root_id][0] / name
            path.write_bytes(f"payload-{role}".encode("utf-8"))
            entries.append(
                {"id": role, "role": role, "root": root_id, "path": name, "required": True}
            )
        spec = {
            "schema_version": 1,
            "release": {
                "version": "1.0.0-test",
                "git_commit": self.head,
                "platform": "windows-x86_64-vulkan",
                "architecture": "x86_64",
                "acceleration": "vulkan",
                "build_command": ["pnpm", "tauri:build:vulkan"],
            },
            "policy": {
                "system_drive": "C:",
                "require_clean_tracked_tree": False,
                "complete_inventory_classifications": ["package", "application", "moss_data"],
            },
            "roots": {
                root_id: {
                    "source": str(path),
                    "classification": classification,
                    "deployment_root": deployment,
                }
                for root_id, (path, classification, deployment) in roots.items()
            },
            "files": entries,
        }
        spec_path = self.root / "package-spec.json"
        write_json(spec_path, spec)
        return spec_path, spec

    def test_package_manifest_requires_roles_and_D_drive_placement(self) -> None:
        spec_path, spec = self.make_package_spec()
        args = SimpleNamespace(repo=REPO, spec=spec_path, output=self.root / "release-manifest.json")
        self.assertEqual(command_package(args), 0)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], PASS)
        self.assertEqual({item["role"] for item in report["files"]}, REQUIRED_PACKAGE_ROLES)

        spec["roots"]["moss"]["deployment_root"] = r"C:\MeetilyData"
        write_json(spec_path, spec)
        self.assertEqual(command_package(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertFalse(report["checks"]["moss_data_deploys_to_D_MeetilyData"])

    def test_package_manifest_rejects_path_traversal(self) -> None:
        spec_path, spec = self.make_package_spec()
        spec["files"][0]["path"] = "../outside.bin"
        write_json(spec_path, spec)
        args = SimpleNamespace(repo=REPO, spec=spec_path, output=self.root / "release-manifest.json")
        self.assertEqual(command_package(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], FAIL)

    def test_package_manifest_rejects_duplicate_targets_and_unlisted_payload(self) -> None:
        spec_path, spec = self.make_package_spec()
        spec["files"][1]["root"] = spec["files"][0]["root"]
        spec["files"][1]["path"] = spec["files"][0]["path"]
        application = Path(spec["roots"]["application"]["source"])
        (application / "unlisted.dll").write_bytes(b"unlisted")
        write_json(spec_path, spec)
        args = SimpleNamespace(repo=REPO, spec=spec_path, output=self.root / "release-manifest.json")
        self.assertEqual(command_package(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], FAIL)
        self.assertFalse(report["checks"]["file_targets_unique"])
        self.assertFalse(report["checks"]["complete_inventory_application"])

    def test_build_wiring_audit_covers_every_tauri_workflow(self) -> None:
        output = self.root / "build-wiring.json"
        self.assertEqual(command_build_wiring(SimpleNamespace(repo=REPO, output=output)), 0)
        report = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], PASS)
        self.assertFalse(report["failures"])
        bound = load_bound_report(
            output,
            label="build wiring",
            expected_stage="MOSS_V3_P6_BUILD_WIRING",
            expected_commit=self.head,
        )
        self.assertEqual(bound["status"], PASS)

        report["checks"] = {}
        write_json(output, report)
        with self.assertRaisesRegex(GateError, "all-true checks map"):
            load_bound_report(
                output,
                label="build wiring",
                expected_stage="MOSS_V3_P6_BUILD_WIRING",
                expected_commit=self.head,
            )

    def test_evidence_manifest_does_not_turn_not_run_into_pass(self) -> None:
        evidence = self.root / "evidence"
        evidence.mkdir()
        for name in REQUIRED_EVIDENCE_FILES:
            (evidence / name).write_text(f"evidence:{name}\n", encoding="utf-8")
        args = SimpleNamespace(
            repo=REPO,
            evidence_dir=evidence,
            output=None,
            p6_status=NOT_RUN,
            final_audit=None,
        )
        self.assertEqual(command_evidence_manifest(args), 0)
        report = json.loads((evidence / "07-evidence-manifest.json").read_text(encoding="utf-8"))
        self.assertEqual(report["manifest_status"], PASS)
        self.assertEqual(report["p6_status"], NOT_RUN)

        (evidence / REQUIRED_EVIDENCE_FILES[0]).unlink()
        self.assertEqual(command_evidence_manifest(args), 1)

    def test_evidence_manifest_rejects_a_superficial_final_pass(self) -> None:
        evidence = self.root / "evidence"
        evidence.mkdir()
        for name in REQUIRED_EVIDENCE_FILES:
            (evidence / name).write_text(f"evidence:{name}\n", encoding="utf-8")
        forged = evidence / "final-audit.json"
        write_json(
            forged,
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P6_FINAL_AUDIT",
                "source_commit": self.head,
                "p6_status": PASS,
            },
        )
        args = SimpleNamespace(
            repo=REPO,
            evidence_dir=evidence,
            output=None,
            p6_status=PASS,
            final_audit=forged,
        )
        with self.assertRaises(GateError):
            command_evidence_manifest(args)

    def test_missing_runtime_contract_is_a_reported_failure(self) -> None:
        args = self.make_assets()
        (args.runtime / "contract.json").unlink()
        self.assertEqual(command_assets(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], FAIL)
        self.assertFalse(report["checks"]["runtime_contract_exists"])
        self.assertFalse(report["checks"]["runtime_contract_lane"])

    def test_stage_gate_verifies_evidence_hash_and_git_ancestry(self) -> None:
        stage_dir = self.root / "stage"
        stage_dir.mkdir()
        evidence = stage_dir / "proof.json"
        write_json(evidence, {"proof": True})
        gate = stage_dir / "gate.json"
        write_json(
            gate,
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P3",
                "status": PASS,
                "source_commit": self.head,
                "files": [
                    {"path": evidence.name, "bytes": evidence.stat().st_size, "sha256": sha256_file(evidence)}
                ],
            },
        )
        result = validate_stage_gate(gate, "P3", REPO, self.head)
        self.assertEqual(result["status"], PASS)
        evidence.write_text("changed", encoding="utf-8")
        with self.assertRaises(GateError):
            validate_stage_gate(gate, "P3", REPO, self.head)

    def test_release_dependency_chain_rejects_parallel_unordered_stages(self) -> None:
        stages = {
            stage: {"status": PASS, "source_commit": str(index) * 40}
            for index, stage in enumerate(("P2", "P3", "P4", "P5"), start=2)
        }
        with patch(
            "moss_v3_p6_release.git_is_ancestor",
            side_effect=[True, False, True, True],
        ):
            valid, errors = validate_release_dependency_chain(stages, REPO, self.head)
        self.assertFalse(valid)
        self.assertEqual(errors, ["P3_NOT_ANCESTOR_OF_P4"])

    def test_metrics_need_human_truth_and_apply_every_threshold(self) -> None:
        lock = {
            "samples": [
                {"name": "business_737s", "sha256": "A" * 64, "source_timeline_duration_seconds": 737.728},
                {"name": "long_3096s", "sha256": "B" * 64, "source_timeline_duration_seconds": 3096.62},
            ]
        }
        lock_path = self.root / "p1-lock.json"
        write_json(lock_path, lock)
        truth = self.root / "human-truth.json"
        write_json(truth, {"human_approved": True})
        approval = self.root / "human-approval.json"
        write_json(
            approval,
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P6_HUMAN_REFERENCE_APPROVAL",
                "status": "APPROVED",
                "reference_bytes": truth.stat().st_size,
                "reference_sha256": sha256_file(truth),
                "business_audio_sha256": "A" * 64,
                "reviewed_duration_seconds": 737.728,
                "reviewer": "fixture-reviewer",
                "approved_at": "2026-08-29T00:00:00Z",
                "attestation": HUMAN_APPROVAL_ATTESTATION,
            },
        )
        metrics_path = self.root / "metrics.json"
        measurement_evidence = []
        for role in sorted(REQUIRED_METRIC_EVIDENCE_ROLES):
            evidence_file = self.root / f"{role}.json"
            write_json(evidence_file, {"role": role, "fixture": True})
            measurement_evidence.append(
                {
                    "role": role,
                    "path": evidence_file.name,
                    "bytes": evidence_file.stat().st_size,
                    "sha256": sha256_file(evidence_file),
                }
            )
        metrics = {
            "schema_version": 1,
            "stage": "MOSS_V3_P6_METRICS",
            "source_commit": self.head,
            "audio": {
                "business_737s": {"sha256": "A" * 64, "duration_seconds": 737.728},
                "long_3096s": {"sha256": "B" * 64, "duration_seconds": 3096.62},
            },
            "ground_truth": {
                "status": "HUMAN_APPROVED_FULL_REFERENCE",
                "path": truth.name,
                "bytes": truth.stat().st_size,
                "sha256": sha256_file(truth),
                "approval_path": approval.name,
                "approval_bytes": approval.stat().st_size,
                "approval_sha256": sha256_file(approval),
            },
            "measurement_evidence": measurement_evidence,
            "metrics": {
                "moss_raw_cer": 0.19,
                "whisper_same_window_cer": 0.20,
                "corrected_cer": 0.14,
                "spoken_term_accuracy": 0.95,
                "unspoken_term_insertions": 0,
                "raw_speaker_segment_error_rate": 0.25,
                "raw_speaker_duration_error_rate": 0.19,
                "claims_automatic_speaker_accuracy": False,
                "corrected_speaker_error_count": 0,
                "timestamp_parse_rate": 1.0,
                "timestamps_monotonic": True,
                "timestamps_in_bounds": True,
                "business_rtf": 1.0,
                "long_audio_complete": True,
                "long_audio_last_timestamp_seconds": 3096.62,
                "moss_residual_process_count": 0,
                "qwen_started_after_moss_exit": True,
            },
        }
        write_json(metrics_path, metrics)
        result = evaluate_metrics(metrics_path, lock_path, self.head)
        self.assertEqual(result["status"], PASS)

        metrics["ground_truth"]["status"] = "MACHINE_GENERATED_NOT_GROUND_TRUTH"
        write_json(metrics_path, metrics)
        self.assertEqual(evaluate_metrics(metrics_path, lock_path, self.head)["status"], BLOCKED)

        metrics["ground_truth"]["status"] = "HUMAN_APPROVED_FULL_REFERENCE"
        metrics["metrics"]["unspoken_term_insertions"] = 1
        write_json(metrics_path, metrics)
        self.assertEqual(evaluate_metrics(metrics_path, lock_path, self.head)["status"], FAIL)

        metrics["metrics"]["unspoken_term_insertions"] = False
        write_json(metrics_path, metrics)
        with self.assertRaisesRegex(GateError, "non-negative integer"):
            evaluate_metrics(metrics_path, lock_path, self.head)

        metrics["metrics"]["unspoken_term_insertions"] = 0
        metrics["metrics"]["moss_raw_cer"] = -0.01
        write_json(metrics_path, metrics)
        with self.assertRaisesRegex(GateError, "at least 0.0"):
            evaluate_metrics(metrics_path, lock_path, self.head)

        metrics["metrics"]["moss_raw_cer"] = 0.19
        write_json(metrics_path, metrics)
        (self.root / measurement_evidence[0]["path"]).write_text(
            "changed\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(GateError, "measurement evidence changed"):
            evaluate_metrics(metrics_path, lock_path, self.head)

    def test_final_release_inventory_is_bound_to_frozen_assets(self) -> None:
        hashes = {
            "model": "1" * 64,
            "model_license": "2" * 64,
            "contract": "3" * 64,
            "runtime": "4" * 64,
            "runtime_license": "5" * 64,
            "third_party": "6" * 64,
            "bundled": "7" * 64,
        }
        release_path = self.root / "release.json"
        asset_path = self.root / "asset.json"
        release_files = [
            {"root": "moss", "role": "moss_model", "path": "model.gguf", "sha256": hashes["model"]},
            {"root": "moss", "role": "model_license", "path": "MODEL-LICENSE", "sha256": hashes["model_license"]},
            {"root": "moss", "role": "moss_runtime", "path": "runtime/contract.json", "sha256": hashes["contract"]},
            {"root": "moss", "role": "runtime_component", "path": "runtime/transcribe.dll", "sha256": hashes["runtime"]},
            {"root": "moss", "role": "runtime_license", "path": "runtime/licenses/LICENSE", "sha256": hashes["runtime_license"]},
            {"root": "moss", "role": "third_party_licenses", "path": "THIRD-PARTY-LICENSES.md", "sha256": hashes["third_party"]},
            {"root": "moss", "role": "runtime_component", "path": "runtime/licenses/ggml/LICENSE", "sha256": hashes["bundled"]},
        ]
        write_json(
            release_path,
            {
                "stage": "MOSS_V3_P6_RELEASE_MANIFEST",
                "status": PASS,
                "source_commit": self.head,
                "deployment_roots": {"moss": {"classification": "moss_data"}},
                "files": release_files,
            },
        )
        write_json(
            asset_path,
            {
                "stage": "MOSS_V3_P6_ASSET_INTEGRITY",
                "status": PASS,
                "source_commit": self.head,
                "model": {"sha256": hashes["model"]},
                "model_license": {"sha256": hashes["model_license"]},
                "runtime": {
                    "files": [
                        {"path": "contract.json", "sha256": hashes["contract"]},
                        {"path": "transcribe.dll", "sha256": hashes["runtime"]},
                    ],
                    "bundled_licenses": [
                        {"path": "ggml/LICENSE", "sha256": hashes["bundled"]}
                    ],
                },
                "runtime_license": {"sha256": hashes["runtime_license"]},
                "third_party_licenses": {"sha256": hashes["third_party"]},
            },
        )
        self.assertEqual(
            evaluate_release_asset_binding(release_path, asset_path, self.head)["status"], PASS
        )

        release_files[3]["sha256"] = "8" * 64
        write_json(
            release_path,
            {
                "stage": "MOSS_V3_P6_RELEASE_MANIFEST",
                "status": PASS,
                "source_commit": self.head,
                "deployment_roots": {"moss": {"classification": "moss_data"}},
                "files": release_files,
            },
        )
        result = evaluate_release_asset_binding(release_path, asset_path, self.head)
        self.assertEqual(result["status"], FAIL)
        self.assertIn("transcribe.dll", result["missing_runtime_files"])

    def test_final_audit_stays_not_run_without_p3_p5_or_real_reports(self) -> None:
        output = self.root / "final-audit.json"
        args = SimpleNamespace(
            repo=REPO,
            stage_gate=[],
            release_manifest=None,
            asset_report=None,
            build_wiring_report=None,
            lifecycle_report=None,
            acceptance_report=None,
            metrics_report=None,
            output=output,
        )
        self.assertEqual(command_final_audit(args), 2)
        report = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(report["p6_status"], NOT_RUN)
        self.assertEqual(report["l2_release_status"], BLOCKED)
        self.assertFalse(report["p3_p5_merged_and_passed"])
        self.assertIn("P3_P4_P5_NOT_ALL_MERGED_AND_PASS", report["open_gates"])


if __name__ == "__main__":
    unittest.main()
