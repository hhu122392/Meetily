import importlib.util
import hashlib
import json
import os
import shutil
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).with_name("moss_v3_p2c_acceptance.py")
SPEC = importlib.util.spec_from_file_location("moss_v3_p2c_acceptance", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def fixed_hash(label: str) -> str:
    return hashlib.sha256(label.encode("utf-8")).hexdigest().upper()


def helper_run(index: int, offset: int, duration: int, process_id: int) -> dict:
    return {
        "chunkIndex": index,
        "cleanTextSha256": fixed_hash(f"clean-{offset}-{duration}"),
        "inputDurationMs": duration,
        "inputOffsetMs": offset,
        "lastTimestampMs": duration - 1_000,
        "nativeRtf": 0.25,
        "peakJobMemoryBytes": 100_000,
        "processId": process_id,
        "rawTextSha256": fixed_hash(f"raw-{offset}-{duration}"),
        "residualProcessCount": 0,
        "terminalCount": 1,
        "totalProcesses": 1,
    }


def create_complete_evidence(directory: Path) -> dict[str, Path]:
    commit = "a" * 40
    artifacts = {
        name: {"bytes": index + 1, "sha256": fixed_hash(name)}
        for index, name in enumerate(sorted(MODULE.BINDING_ARTIFACT_NAMES))
    }
    binding_path = directory / "binding.json"
    MODULE.atomic_json(
        binding_path,
        {
            "schema_version": 1,
            "stage": "MOSS_V3_P2D_BINDING",
            "status": "PASS",
            "commit": commit,
            "tree_clean": True,
            "artifacts": artifacts,
        },
    )
    binding_sha256 = MODULE.sha256_file(binding_path)

    common = {
        "schema_version": 1,
        "status": "PASS",
        "binding_sha256": binding_sha256,
        "source_commit": commit,
        "source_tree_clean": True,
        "cargo_lock_sha256": artifacts["cargo_lock"]["sha256"],
        "test_executable_sha256": artifacts["test_executable"]["sha256"],
    }
    support: dict[str, Path] = {}
    support_specs = {
        "single_first": ("performance-300-1.json", "MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm", [helper_run(0, 0, 300_000, 101)]),
        "single_second": ("performance-300-2.json", "MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm", [helper_run(0, 0, 300_000, 102)]),
        "single_third": ("performance-300-3.json", "MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm", [helper_run(0, 0, 300_000, 103)]),
        "four_eighty": ("performance-480.json", "MOSS_V3_P2C_480S_PERFORMANCE", "four_eighty_pcm", [helper_run(0, 0, 300_000, 104), helper_run(1, 300_000, 180_000, 105)]),
        "six_hundred": ("performance-600.json", "MOSS_V3_P2C_600S_PERFORMANCE", "six_hundred_pcm", [helper_run(0, 0, 300_000, 106), helper_run(1, 300_000, 300_000, 107)]),
    }
    for name, (filename, stage, fixture_name, runs) in support_specs.items():
        path = directory / filename
        MODULE.atomic_json(
            path,
            {
                **common,
                "stage": stage,
                "helper_binary_sha256": artifacts["moss_helper"]["sha256"],
                "runtime_manifest_sha256": artifacts["runtime_manifest"]["sha256"],
                "model_sha256": artifacts["q8_model"]["sha256"],
                "fixture_sha256": artifacts[fixture_name]["sha256"],
                "helper_runs": runs,
                "helper_process_id": runs[0]["processId"] if len(runs) == 1 else 0,
                "residual_process_count": 0,
            },
        )
        support[name] = path

    stage_values = {
        "code_validation": {},
        "ten_sequential": {"run_count": 10, "cross_run_hash_isolation": True},
        "chunk_control": {},
        "real_control": {},
        "performance": {
            "identical_300_second_run_count": 3,
            "identical_output_hashes": True,
            "unique_terminal_per_run": True,
            "new_windows_error_1000_1001_count": 0,
            "residual_process_count": 0,
            "source_evidence_sha256": {
                name: MODULE.sha256_file(path) for name, path in support.items()
            },
            "source_sidecar_sha256": {
                name: MODULE.sha256_file(path.with_name(path.name + ".sha256"))
                for name, path in support.items()
            },
        },
        "process_faults": {"case_count": 6},
        "truncation": {},
        "offline": {
            "terminal_type": "completed",
            "terminal_count": 1,
            "token_is_appcontainer": True,
            "capability_count": 0,
            "network_before_blocked": True,
            "network_after_blocked": True,
            "residual_process_count": 0,
        },
        "parent_kill": {"residual_process_count": 0},
        "duplicate_request": {
            "second_helper_started": False,
            "duplicate_error": "MOSS_DUPLICATE_REQUEST",
            "residual_process_count": 0,
        },
        "second_chunk_failure": {
            "partial_success_returned": False,
            "retry_terminal": "completed",
            "residual_process_count": 0,
        },
        "qwen": {
            "health_probe": "pong",
            "model_generate_health": "response_without_error",
            "residual_process_count": 0,
        },
    }
    gates: dict[str, Path] = {}
    for name, stage in MODULE.FINAL_GATE_STAGES.items():
        value = {**common, "stage": stage, **stage_values[name]}
        for field, artifact_name in MODULE.FINAL_GATE_ARTIFACT_FIELDS.get(name, {}).items():
            value[field] = artifacts[artifact_name]["sha256"]
        path = directory / f"gate-{name}.json"
        MODULE.atomic_json(path, value)
        gates[name] = path

    final_path = directory / "final-validation.json"
    MODULE.atomic_json(
        final_path,
        {
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_FINAL_VALIDATION",
            "status": "PASS",
            "source_commit": commit,
            "code_validation": "PASS",
            "real_gates": {
                name: "PASS" for name in MODULE.FINAL_GATE_STAGES if name != "code_validation"
            },
            "traceability": {
                name: {
                    "json_sha256": MODULE.sha256_file(path),
                    "sidecar_sha256": MODULE.sha256_file(path.with_name(path.name + ".sha256")),
                }
                for name, path in gates.items()
            },
            "blocked_gates": [],
            "binding_sha256": binding_sha256,
        },
    )
    return {"binding": binding_path, "final": final_path, **gates, **support}


class AcceptanceHarnessTests(unittest.TestCase):
    def test_environment_removes_every_native_override_prefix(self):
        names = ["VK_TEST_OVERRIDE", "VK_LOADER_DEBUG", "TRANSCRIBE_X", "GGML_X", "VULKAN_SDK"]
        old = {name: os.environ.get(name) for name in names}
        try:
            for name in names:
                os.environ[name] = "private"
            clean = MODULE.native_safe_environment()
            for name in names:
                self.assertNotIn(name, clean)
        finally:
            for name, value in old.items():
                if value is None:
                    os.environ.pop(name, None)
                else:
                    os.environ[name] = value

    def test_public_evidence_rejects_paths_and_text(self):
        with self.assertRaises(AssertionError):
            MODULE.assert_public_safe({"schema_version": 1, "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE", "status": "PASS", "path": r"D:\\private\\fixture"})
        with self.assertRaises(AssertionError):
            MODULE.assert_public_safe({"schema_version": 1, "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE", "status": "PASS", "text": "redacted"})
        with self.assertRaises(AssertionError):
            MODULE.assert_public_safe({"schema_version": 1, "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE", "status": "PASS", "unknown": 1})
        with self.assertRaises(AssertionError):
            MODULE.assert_public_safe(
                {
                    "schema_version": 1,
                    "stage": "MOSS_V3_P2C_QWEN_RELEASE",
                    "status": "PASS",
                    "health_probe": "a free form diagnostic sentence",
                }
            )

    def test_public_evidence_recursively_rejects_nested_identity_and_free_text(self):
        unsafe_values = [
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_TEN_SEQUENTIAL_MANAGER",
                "status": "PASS",
                "runs": [{"participant_name": "Alice"}],
            },
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_PROCESS_FAULTS",
                "status": "PASS",
                "cases": [{"speaker": "gerry", "content_sha256": "A" * 64}],
            },
            {
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_PROCESS_FAULTS",
                "status": "PASS",
                "cases": [{"case": "invalid_json", "note": "free form"}],
            },
        ]
        for value in unsafe_values:
            with self.subTest(value=value), self.assertRaises(AssertionError):
                MODULE.assert_public_safe(value)

    def test_public_evidence_rejects_nonfinite_values_at_every_depth(self):
        for nonfinite in (float("nan"), float("inf"), float("-inf")):
            value = {
                "schema_version": 1,
                "stage": "MOSS_V3_P2C_CHUNKED_PERFORMANCE",
                "status": "PASS",
                "four_hundred_eighty": {
                    "child_durations_ms": [300_000, 180_000],
                    "last_timestamp_ms": 479_000,
                    "supervisor_wall_rtf": nonfinite,
                },
            }
            with self.subTest(nonfinite=nonfinite), self.assertRaises(AssertionError):
                MODULE.assert_public_safe(value)

    def test_atomic_json_has_matching_sha_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "evidence.json"
            MODULE.atomic_json(path, {"status": "PASS"})
            self.assertEqual(json.loads(path.read_text(encoding="utf-8"))["status"], "PASS")
            expected = MODULE.sha256_file(path)
            sidecar = path.with_name(path.name + ".sha256")
            sidecar_bytes = sidecar.read_bytes()
            actual = sidecar_bytes.decode("ascii").split()[0]
            self.assertEqual(actual, expected)
            self.assertTrue(sidecar_bytes.endswith(b"\n"))
            self.assertNotIn(b"\r", sidecar_bytes)

    def test_publish_safe_keeps_rust_sidecar_traceability_byte_identical(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "private" / "rust-gate.json"
            source.parent.mkdir(parents=True)
            encoded = json.dumps(
                {
                    "schema_version": 1,
                    "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE",
                    "status": "PASS",
                    "test_name": "bound_rust_unit_test",
                    "test_executable_sha256": "A" * 64,
                    "exit_code": 0,
                    "log_sha256": "B" * 64,
                    "log_bytes": 1,
                },
                indent=2,
                sort_keys=True,
            ).encode("utf-8") + b"\n"
            source.write_bytes(encoded)
            source_sidecar = source.with_name(source.name + ".sha256")
            source_sidecar.write_bytes(
                f"{MODULE.sha256_file(source)}  {source.name}\n".encode("ascii")
            )
            published = root / "public" / source.name

            MODULE.command_publish_safe(Namespace(source=source, public=published))

            MODULE.verify_sha_sidecar(published)
            self.assertEqual(MODULE.sha256_file(source), MODULE.sha256_file(published))
            self.assertEqual(
                MODULE.sha256_file(source_sidecar),
                MODULE.sha256_file(published.with_name(published.name + ".sha256")),
            )

    def test_publish_and_both_manifests_are_atomic_and_safe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            private = root / "private"
            public = root / "public"
            paths = create_complete_evidence(private)
            shutil.copytree(private, public)
            published = root / "published" / "offline.json"
            MODULE.command_publish_safe(Namespace(source=paths["offline"], public=published))
            MODULE.verify_sha_sidecar(published)
            MODULE.command_private_manifest(Namespace(private_dir=private))
            MODULE.command_manifest(Namespace(public_dir=public))
            self.assertEqual(
                json.loads((private / "manifest.json").read_text(encoding="utf-8"))["status"],
                "PASS",
            )
            self.assertEqual(
                json.loads((public / "manifest.json").read_text(encoding="utf-8"))["status"],
                "PASS",
            )

    def test_manifest_fails_closed_on_missing_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            public = Path(directory)
            (public / "gate.json").write_text(
                json.dumps({"schema_version": 1, "stage": "MOSS_V3_P2C_FINAL_VALIDATION", "status": "PASS"}),
                encoding="utf-8",
            )
            with self.assertRaises(AssertionError):
                MODULE.command_manifest(Namespace(public_dir=public))
            manifest = json.loads((public / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["status"], "BLOCKED")
            self.assertEqual(manifest["integrity_status"], "FAIL")

    def test_nonempty_manifest_without_final_acceptance_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            public = Path(directory)
            MODULE.atomic_json(
                public / "gate.json",
                {
                    "schema_version": 1,
                    "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE",
                    "status": "PASS",
                    "test_name": "native::imp::binding_tests::native_output_truncated_status_maps_to_stable_terminal_code",
                    "test_executable_sha256": "A" * 64,
                    "exit_code": 0,
                    "log_sha256": "B" * 64,
                    "log_bytes": 1,
                },
            )
            with self.assertRaises(AssertionError):
                MODULE.command_manifest(Namespace(public_dir=public))
            manifest = json.loads((public / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["integrity_status"], "FAIL")
            self.assertEqual(manifest["acceptance_status"], "BLOCKED")
            self.assertEqual(manifest["status"], "BLOCKED")

    def test_manifest_rejects_current_gate_tamper_even_with_new_sidecar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = create_complete_evidence(root)
            offline = json.loads(paths["offline"].read_text(encoding="utf-8"))
            offline["status"] = "BLOCKED"
            MODULE.atomic_json(paths["offline"], offline)
            with self.assertRaises(AssertionError):
                MODULE.command_manifest(Namespace(public_dir=root))
            manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["status"], "BLOCKED")

    def test_manifest_rejects_stale_final_traceability(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = create_complete_evidence(root)
            offline = json.loads(paths["offline"].read_text(encoding="utf-8"))
            offline["wall_elapsed_ms"] = 123
            MODULE.atomic_json(paths["offline"], offline)
            with self.assertRaises(AssertionError):
                MODULE.command_manifest(Namespace(public_dir=root))
            self.assertEqual(
                json.loads((root / "manifest.json").read_text(encoding="utf-8"))["status"],
                "BLOCKED",
            )

    def test_manifest_rejects_missing_duplicate_and_unknown_stages(self):
        mutations = (
            "missing",
            "duplicate",
            "unknown",
            "extra_file",
            "orphan_sidecar",
            "extra_directory",
        )
        for mutation in mutations:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                paths = create_complete_evidence(root)
                if mutation == "missing":
                    paths["offline"].unlink()
                    paths["offline"].with_name(paths["offline"].name + ".sha256").unlink()
                elif mutation == "duplicate":
                    duplicate = json.loads(paths["offline"].read_text(encoding="utf-8"))
                    MODULE.atomic_json(root / "duplicate-stage.json", duplicate)
                else:
                    if mutation == "unknown":
                        MODULE.atomic_json(
                            root / "unknown-stage.json",
                            {"schema_version": 1, "stage": "UNAPPROVED_STAGE", "status": "PASS"},
                        )
                    elif mutation == "extra_file":
                        (root / "transcript.txt").write_text("private speech", encoding="utf-8")
                    elif mutation == "orphan_sidecar":
                        (root / "orphan.json.sha256").write_text("A" * 64, encoding="ascii")
                    else:
                        (root / "unexpected-directory").mkdir()
                with self.assertRaises(AssertionError):
                    MODULE.command_manifest(Namespace(public_dir=root))
                self.assertEqual(
                    json.loads((root / "manifest.json").read_text(encoding="utf-8"))["status"],
                    "BLOCKED",
                )

    def test_bound_source_rejects_missing_identity_and_nonfinite_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "source.json"
            MODULE.atomic_json(
                path,
                {"schema_version": 1, "stage": "MOSS_V3_P2C_300S_SINGLE_CHILD", "status": "PASS"},
            )
            binding = {"commit": "a" * 40, "artifacts": {}}
            with self.assertRaises(AssertionError):
                MODULE.load_bound_source(path, "MOSS_V3_P2C_300S_SINGLE_CHILD", binding, "B" * 64, "three_hundred_pcm")
            with self.assertRaises(AssertionError):
                MODULE.validate_binding(
                    {
                        "schema_version": 1,
                        "stage": "MOSS_V3_P2D_BINDING",
                        "status": "PASS",
                        "commit": "a" * 40,
                        "tree_clean": True,
                        "artifacts": {},
                    }
                )
            self.assertFalse(MODULE.finite_nonnegative(float("nan")))
            self.assertFalse(MODULE.finite_nonnegative(float("inf")))

    def test_offline_nonzero_child_always_writes_structured_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "app"
            app.mkdir()
            helper = root / "helper.exe"
            helper.write_bytes(b"MZ")
            runtime = root / "runtime"
            runtime.mkdir()
            (runtime / "contract.json").write_text("{}", encoding="utf-8")
            model = root / "model.gguf"
            model.write_bytes(b"model")
            audio = root / "audio.f32le"
            audio.write_bytes(b"audio")
            python = root / "python.exe"
            python.write_bytes(b"MZ")
            private = root / "private.json"
            public = root / "public.json"
            arguments = Namespace(
                profile="test",
                binding=root / "binding.json",
                python=python,
                helper=helper,
                runtime=runtime,
                model=model,
                audio=audio,
                private=private,
                public=public,
            )
            with mock.patch.object(
                MODULE,
                "load_acceptance_binding",
                return_value=({"commit": "A" * 40, "tree_clean": True, "artifacts": {}}, "B" * 64),
            ), mock.patch.object(
                MODULE, "require_bound_artifact", return_value="C" * 64
            ), mock.patch.object(
                MODULE, "binding_fields", return_value={}
            ), mock.patch.object(
                MODULE, "appcontainer_private_root", return_value=app
            ), mock.patch.object(
                MODULE, "copy_portable_python", return_value=python
            ), mock.patch.object(
                MODULE, "run_in_zero_capability_appcontainer", return_value=7
            ):
                with self.assertRaises(AssertionError):
                    MODULE.command_offline(arguments)
            value = json.loads(public.read_text(encoding="utf-8"))
            self.assertEqual(value["status"], "FAIL")
            self.assertEqual(value["failure_code"], "APPCONTAINER_CHILD_FAILED")
            self.assertFalse(value["result_present"])

    def test_performance_missing_source_writes_blocked_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            arguments = Namespace(
                binding=root / "missing-binding.json",
                private=root / "private.json",
                public=root / "public.json",
            )
            with self.assertRaises(AssertionError):
                MODULE.command_performance(arguments)
            value = json.loads(arguments.public.read_text(encoding="utf-8"))
            self.assertEqual(value["status"], "BLOCKED")
            self.assertEqual(value["failure_code"], "PERFORMANCE_SOURCE_INVALID")

    def test_finalizer_missing_gate_writes_blocked_acceptance(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            arguments = Namespace(
                binding=root / "binding.json",
                private=root / "private.json",
                public=root / "public.json",
                **{name: root / f"missing-{name}.json" for name in MODULE.FINAL_GATE_STAGES},
            )
            binding = {
                "commit": "a" * 40,
                "tree_clean": True,
                "artifacts": {"cargo_lock": {"sha256": "B" * 64}},
            }
            with mock.patch.object(
                MODULE, "load_acceptance_binding", return_value=(binding, "C" * 64)
            ):
                with self.assertRaises(AssertionError):
                    MODULE.command_finalize(arguments)
            value = json.loads(arguments.public.read_text(encoding="utf-8"))
            self.assertEqual(value["status"], "BLOCKED")
            self.assertEqual(value["blocked_gates"], ["P2D_ACCEPTANCE_SOURCE_INVALID"])


if __name__ == "__main__":
    unittest.main()
