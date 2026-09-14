#!/usr/bin/env python3
"""V-06 tests for the functional-fix acceptance configuration and reports."""

from __future__ import annotations

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

QA_SCRIPT_DIR = Path(__file__).resolve().parent
if str(QA_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(QA_SCRIPT_DIR))

import moss_functional_fix_acceptance as acceptance
from moss_v3_p6_common import GateError, atomic_write_json, read_json, sha256_file


PRODUCER_CODE = r"""
import json
from pathlib import Path
import sys

public_root = Path(sys.argv[1])
private_root = Path(sys.argv[2])
run_id = sys.argv[3]
source_commit = sys.argv[4]
candidate_sha256 = sys.argv[5]
case_ids = sys.argv[6].split(',')
behavior = sys.argv[7]
if behavior == 'fail':
    raise SystemExit(7)
for case_id in case_ids:
    public = public_root / case_id / 'result.public.json'
    private = private_root / case_id / 'result.private.json'
    public.parent.mkdir(parents=True, exist_ok=True)
    private.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        'verdict': 'FAIL' if behavior == 'partial' and case_id == case_ids[-1] else 'PASS',
        'run_id': run_id,
        'source_commit': source_commit,
        'candidate_sha256': candidate_sha256,
    }
    public.write_text(json.dumps(payload), encoding='utf-8')
    private.write_text(json.dumps({**payload, 'raw': 'private body'}), encoding='utf-8')
"""


CLEANUP_CODE = r"""
import json
from pathlib import Path
import sys

public_path = Path(sys.argv[1])
private_path = Path(sys.argv[2])
behavior = sys.argv[3]
public_path.parent.mkdir(parents=True, exist_ok=True)
private_path.parent.mkdir(parents=True, exist_ok=True)
if behavior == 'tamper':
    business_path = Path(sys.argv[4])
    business_path.write_text(json.dumps({'verdict': 'PASS', 'tampered_by_cleanup': True}), encoding='utf-8')
payload = {
    'status': 'PASS' if behavior in {'pass', 'tamper'} else 'FAIL',
    'residual_process_count': 0 if behavior in {'pass', 'tamper'} else 1,
}
public_path.write_text(json.dumps(payload), encoding='utf-8')
private_path.write_text(json.dumps({**payload, 'raw': 'cleanup private body'}), encoding='utf-8')
raise SystemExit(0 if behavior in {'pass', 'tamper'} else 9)
"""


class AcceptanceTest(unittest.TestCase):
    maxDiff = None

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="moss-acceptance-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.external = self.root / "external"
        self.public_root = self.root / "public"
        self.private_root = self.root / "private"
        for path in (self.repo, self.external, self.public_root, self.private_root):
            path.mkdir(parents=True)
        (self.repo / "tracked.txt").write_text("frozen source\n", encoding="utf-8")
        self._git("init", "-q")
        self._git("config", "user.email", "qa@example.invalid")
        self._git("config", "user.name", "QA")
        self._git("add", "tracked.txt")
        self._git("commit", "-q", "-m", "fixture")
        self.head = self._git("rev-parse", "HEAD").strip()
        self.candidate = self.external / "candidate.exe"
        self.manifest = self.external / "build-manifest.json"
        self.input_file = self.external / "input.wav"
        self.candidate.write_bytes(b"candidate-v2")
        self.manifest.write_text(
            json.dumps({"source_commit": self.head, "kind": "test build"}), encoding="utf-8"
        )
        self.input_file.write_bytes(b"RIFF-test-input")
        self.config_path = self.external / "acceptance.json"
        self.ids = ("FT-01", "FT-02")
        self.names = {"FT-01": "live-bubbles", "FT-02": "stop-convergence"}
        self.run_keys = {"FT-01": "group-one", "FT-02": "group-two"}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _git(self, *args: str) -> str:
        completed = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        return completed.stdout

    @staticmethod
    def _record(path: Path) -> dict[str, object]:
        return {"path": str(path), "bytes": path.stat().st_size, "sha256": sha256_file(path)}

    def make_document(
        self,
        *,
        same_group: bool = False,
        producer_behavior: str = "pass",
        cleanup_behavior: str = "pass",
    ) -> dict[str, object]:
        run_keys = (
            {"FT-01": "group-one", "FT-02": "group-one"}
            if same_group
            else dict(self.run_keys)
        )
        groups: dict[str, list[str]] = {}
        for case_id in self.ids:
            groups.setdefault(run_keys[case_id], []).append(case_id)
        scenarios: list[dict[str, object]] = []
        for case_id in self.ids:
            run_key = run_keys[case_id]
            member_ids = groups[run_key]
            public_result = f"{case_id}/result.public.json"
            private_result = f"{case_id}/result.private.json"
            cleanup_public = f"cleanup/{run_key}.public.json"
            cleanup_private = f"cleanup/{run_key}.private.json"
            dependencies = [] if case_id == "FT-01" or same_group else ["FT-01"]
            producer_argv = [
                sys.executable,
                "-c",
                PRODUCER_CODE,
                str(self.public_root),
                str(self.private_root),
                "FIX-TEST-20260902-02",
                self.head,
                sha256_file(self.candidate),
                ",".join(member_ids),
                producer_behavior,
            ]
            cleanup_argv = [
                sys.executable,
                "-c",
                CLEANUP_CODE,
                str(self.public_root / cleanup_public),
                str(self.private_root / cleanup_private),
                cleanup_behavior,
            ]
            scenarios.append(
                {
                    "id": case_id,
                    "name": self.names[case_id],
                    "run_once_key": run_key,
                    "source_commit": self.head,
                    "candidate_sha256": sha256_file(self.candidate),
                    "argv": producer_argv,
                    "cwd": str(self.repo),
                    "expected_exit_codes": [0],
                    "inputs": [self._record(self.input_file)],
                    "timeout_seconds": 10,
                    "force_stop_seconds": 1,
                    "dependencies": dependencies,
                    "pass_assertions": [
                        {
                            "id": "verdict-pass",
                            "source": "public",
                            "path": public_result,
                            "pointer": "/verdict",
                            "op": "eq",
                            "value": "PASS",
                        },
                        {
                            "id": "run-bound",
                            "source": "public",
                            "path": public_result,
                            "pointer": "/run_id",
                            "op": "eq",
                            "value": "FIX-TEST-20260902-02",
                        },
                    ],
                    "fail_assertions": [
                        {
                            "id": "no-fail",
                            "source": "public",
                            "path": public_result,
                            "pointer": "/verdict",
                            "op": "in",
                            "value": ["FAIL", "BLOCKED", "NOT RUN"],
                        }
                    ],
                    "evidence": {
                        "public": [public_result, cleanup_public],
                        "private": [private_result, cleanup_private],
                    },
                    "cleanup": {
                        "argv": cleanup_argv,
                        "cwd": str(self.repo),
                        "expected_exit_codes": [0],
                        "timeout_seconds": 10,
                        "force_stop_seconds": 1,
                        "checks": [
                            {
                                "id": "cleanup-pass",
                                "source": "public",
                                "path": cleanup_public,
                                "pointer": "/status",
                                "op": "eq",
                                "value": "PASS",
                            },
                            {
                                "id": "zero-residual",
                                "source": "public",
                                "path": cleanup_public,
                                "pointer": "/residual_process_count",
                                "op": "eq",
                                "value": 0,
                            },
                        ],
                    },
                }
            )
        return {
            "schema_version": 1,
            "stage": acceptance.CONFIG_STAGE,
            "template_only": False,
            "source_commit": self.head,
            "run_id": "FIX-TEST-20260902-02",
            "machine_run_id": "FIX-TEST-20260902-02",
            "execution_contract": copy.deepcopy(acceptance.FORMAL_EXECUTION_CONTRACT),
            "candidate": self._record(self.candidate),
            "build_manifest": self._record(self.manifest),
            "evidence_roots": {
                "public": str(self.public_root),
                "private": str(self.private_root),
            },
            "statistics": {
                "expected_total": len(self.ids),
                "run_once_total": len(set(run_keys.values())),
            },
            "scenarios": scenarios,
        }

    def write_document(self, document: dict[str, object]) -> None:
        atomic_write_json(self.config_path, document)

    def validate(self, *, same_group: bool = False) -> dict[str, object]:
        keys = (
            {"FT-01": "group-one", "FT-02": "group-one"}
            if same_group
            else self.run_keys
        )
        return acceptance.validate_config(
            self.config_path,
            self.repo,
            required_ids=self.ids,
            required_names=self.names,
            expected_run_keys=keys,
        )

    def invalid(self, mutate, *, same_group: bool = False) -> None:
        document = self.make_document(same_group=same_group)
        mutate(document)
        self.write_document(document)
        with self.assertRaises(GateError):
            self.validate(same_group=same_group)

    def run_fixture(
        self,
        document: dict[str, object] | None = None,
        *,
        same_group: bool = False,
        suffix: str = "",
    ):
        self.write_document(document or self.make_document(same_group=same_group))
        keys = (
            {"FT-01": "group-one", "FT-02": "group-one"}
            if same_group
            else self.run_keys
        )
        public_report = self.public_root / f"acceptance{suffix}.public.json"
        private_report = self.private_root / f"acceptance{suffix}.private.json"
        result = acceptance.run_acceptance(
            self.config_path,
            self.repo,
            public_report,
            private_report,
            required_ids=self.ids,
            required_names=self.names,
            expected_run_keys=keys,
        )
        return result, public_report, private_report, keys

    def test_01_formal_template_has_exact_28_cases_and_seven_groups(self) -> None:
        template_path = Path(__file__).resolve().parents[2] / (
            "target/release/docs/方案/MOSS功能修复计划-20260902/acceptance.template.json"
        )
        document = read_json(template_path, "formal template")
        scenarios = document["scenarios"]
        self.assertEqual([item["id"] for item in scenarios], list(acceptance.FORMAL_CASE_IDS))
        self.assertEqual(
            [item["name"] for item in scenarios],
            [acceptance.FORMAL_CASE_NAMES[item] for item in acceptance.FORMAL_CASE_IDS],
        )
        self.assertEqual(len({item["run_once_key"] for item in scenarios}), 7)
        self.assertEqual(document["machine_run_id"], document["run_id"])
        self.assertEqual(document["execution_contract"], acceptance.FORMAL_EXECUTION_CONTRACT)
        self.assertEqual(
            [item["task_id"] for item in document["cua_contract"]["records"]],
            list(acceptance.CUA_TASK_IDS),
        )
        self.assertEqual(document["cua_contract"], acceptance.FORMAL_CUA_CONTRACT)
        self.assertEqual(
            list(document["producer_nonces"]), list(acceptance.FORMAL_RUN_KEYS)
        )
        self.assertEqual(
            document["schema_bindings"], acceptance.formal_schema_template_bindings()
        )
        self.assertEqual(
            next(item for item in scenarios if item["id"] == "FT-26")["dependencies"],
            ["FT-22", "FT-23", "FT-24", "FT-25"],
        )
        acceptance._template_shape(
            document,
            acceptance.FORMAL_CASE_IDS,
            acceptance.FORMAL_CASE_NAMES,
            acceptance.FORMAL_RUN_KEY_BY_ID,
        )

    def test_02_valid_reduced_config_passes_internal_validator(self) -> None:
        self.write_document(self.make_document())
        context = self.validate()
        self.assertEqual(len(context["scenarios"]), 2)

    def test_03_cli_contract_never_accepts_reduced_case_set(self) -> None:
        self.write_document(self.make_document())
        with self.assertRaises(GateError):
            acceptance.validate_config(self.config_path, self.repo)

    def test_04_missing_case_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"].pop())

    def test_05_duplicate_case_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"].__setitem__(1, copy.deepcopy(d["scenarios"][0])))

    def test_06_case_order_must_match_ft_numbering(self) -> None:
        self.invalid(lambda d: d["scenarios"].reverse())

    def test_07_required_name_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0].__setitem__("name", "wrong-name"))

    def test_08_deep_placeholder_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0]["pass_assertions"][0].__setitem__("value", "__LEFT__")
        )

    def test_09_obsolete_worktree_marker_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0]["argv"].append("meetily-moss-ft-1b9c29a-20260902")
        )

    def test_10_obsolete_run_id_is_rejected(self) -> None:
        self.invalid(lambda d: d.__setitem__("run_id", acceptance.OLD_RUN_ID))

    def test_11_obsolete_candidate_sha_is_rejected_anywhere(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0].__setitem__(
                "candidate_sha256", acceptance.OLD_CANDIDATE_SHA256
            )
        )

    def test_12_short_source_commit_is_rejected(self) -> None:
        self.invalid(lambda d: d.__setitem__("source_commit", self.head[:12]))

    def test_13_full_but_wrong_source_commit_is_rejected(self) -> None:
        self.invalid(lambda d: d.__setitem__("source_commit", "a" * 40))

    def test_14_malformed_candidate_sha_is_rejected(self) -> None:
        self.invalid(lambda d: d["candidate"].__setitem__("sha256", "ABC"))

    def test_15_candidate_real_hash_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["candidate"].__setitem__("sha256", "A" * 64))

    def test_16_candidate_real_byte_count_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["candidate"].__setitem__("bytes", 999))

    def test_17_input_real_hash_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0]["inputs"][0].__setitem__("sha256", "B" * 64))

    def test_18_input_real_byte_count_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0]["inputs"][0].__setitem__("bytes", 0))

    def test_19_missing_cwd_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0].__setitem__("cwd", str(self.repo / "missing"))
        )

    def test_20_cwd_outside_checkout_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0].__setitem__("cwd", str(self.external)))

    def test_21_missing_command_executable_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0]["argv"].__setitem__(
                0, "definitely-not-a-real-moss-command-20260902"
            )
        )

    def test_22_empty_public_evidence_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0]["evidence"].__setitem__("public", []))

    def test_23_evidence_path_escape_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0]["evidence"]["public"].__setitem__(0, "../escape.json")
        )

    def test_24_cleanup_without_checks_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0]["cleanup"].__setitem__("checks", []))

    def test_25_missing_dependency_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][1].__setitem__("dependencies", ["FT-99"])
        )

    def test_26_dependency_cycle_is_rejected(self) -> None:
        def mutate(document):
            document["scenarios"][0]["dependencies"] = ["FT-02"]
            document["scenarios"][1]["dependencies"] = ["FT-01"]

        self.invalid(mutate)

    def test_27_wrong_expected_total_is_rejected(self) -> None:
        self.invalid(lambda d: d["statistics"].__setitem__("expected_total", 28))

    def test_28_wrong_run_once_total_is_rejected(self) -> None:
        self.invalid(lambda d: d["statistics"].__setitem__("run_once_total", 7))

    def test_29_shared_definition_mismatch_is_rejected(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][1].__setitem__("timeout_seconds", 11), same_group=True
        )

    def test_30_evidence_collision_between_groups_is_rejected(self) -> None:
        def mutate(document):
            document["scenarios"][1]["evidence"]["public"][0] = "FT-01/result.public.json"
            document["scenarios"][1]["pass_assertions"][0]["path"] = "FT-01/result.public.json"
            document["scenarios"][1]["pass_assertions"][1]["path"] = "FT-01/result.public.json"
            document["scenarios"][1]["fail_assertions"][0]["path"] = "FT-01/result.public.json"

        self.invalid(mutate)

    def test_31_assertion_path_must_be_declared_evidence(self) -> None:
        self.invalid(
            lambda d: d["scenarios"][0]["pass_assertions"][0].__setitem__(
                "path", "undeclared.json"
            )
        )

    def test_32_fail_assertions_cannot_be_empty(self) -> None:
        self.invalid(lambda d: d["scenarios"][0].__setitem__("fail_assertions", []))

    def test_33_materialize_replaces_tokens_and_hashes_real_inputs(self) -> None:
        template = self.make_document()
        template["template_only"] = True
        template["source_commit"] = "__SOURCE_COMMIT__"
        template["run_id"] = "__RUN_ID__"
        template["machine_run_id"] = "__RUN_ID__"
        template["candidate"] = {
            "path": "__CANDIDATE_PATH__",
            "bytes": 0,
            "sha256": "__CANDIDATE_SHA256__",
        }
        template["build_manifest"] = {
            "path": "__BUILD_MANIFEST_PATH__",
            "bytes": 0,
            "sha256": "__BUILD_MANIFEST_SHA256__",
        }
        template["evidence_roots"] = {
            "public": "__PUBLIC_EVIDENCE_ROOT__",
            "private": "__PRIVATE_EVIDENCE_ROOT__",
        }
        for scenario in template["scenarios"]:
            scenario["source_commit"] = "__SOURCE_COMMIT__"
            scenario["candidate_sha256"] = "__CANDIDATE_SHA256__"
            scenario["cwd"] = "__REPO_ROOT__"
            scenario["inputs"][0] = {"path": "__INPUT_PATH__", "bytes": 0, "sha256": "0" * 64}
            scenario["cleanup"]["cwd"] = "__REPO_ROOT__"
        template_path = self.external / "template.json"
        output_path = self.external / "materialized.json"
        atomic_write_json(template_path, template)
        context = acceptance.materialize_config(
            repo=self.repo,
            template_path=template_path,
            output_path=output_path,
            candidate_path=self.candidate,
            build_manifest_path=self.manifest,
            run_id="FIX-MATERIALIZED-20260902-02",
            public_root=self.public_root,
            private_root=self.private_root,
            tokens={"INPUT_PATH": str(self.input_file)},
            required_ids=self.ids,
            required_names=self.names,
            expected_run_keys=self.run_keys,
        )
        self.assertEqual(context["source_commit"], self.head)
        self.assertEqual(context["scenarios"][0]["inputs"][0]["sha256"], sha256_file(self.input_file))

    def test_34_materialize_rejects_unresolved_deep_token(self) -> None:
        template = self.make_document()
        template["template_only"] = True
        template["scenarios"][0]["pass_assertions"][0]["value"] = "__UNRESOLVED__"
        template_path = self.external / "template.json"
        atomic_write_json(template_path, template)
        with self.assertRaises(GateError):
            acceptance.materialize_config(
                repo=self.repo,
                template_path=template_path,
                output_path=self.external / "materialized.json",
                candidate_path=self.candidate,
                build_manifest_path=self.manifest,
                run_id="FIX-MATERIALIZED-20260902-02",
                public_root=self.public_root,
                private_root=self.private_root,
                tokens={},
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=self.run_keys,
            )

    def test_35_shared_run_once_group_executes_producer_once(self) -> None:
        (code, public, _), _, _, _ = self.run_fixture(same_group=True)
        self.assertEqual(code, 0)
        self.assertEqual(public["producer_run_counts"], {"group-one": 1})
        self.assertEqual(public["statistics"]["pass"], 2)

    def test_36_failed_dependency_skips_later_producer_but_runs_cleanup(self) -> None:
        document = self.make_document(producer_behavior="fail")
        (code, public, _), _, _, _ = self.run_fixture(document)
        self.assertEqual(code, 1)
        by_id = {item["id"]: item for item in public["scenarios"]}
        self.assertEqual(by_id["FT-01"]["status"], "FAIL")
        self.assertEqual(by_id["FT-02"]["status"], "NOT RUN")
        self.assertEqual(by_id["FT-02"]["producer"]["status"], "NOT RUN")
        self.assertEqual(by_id["FT-02"]["cleanup"]["status"], "PASS")

    def test_37_timeout_fails_and_cleanup_still_runs(self) -> None:
        document = self.make_document()
        document["scenarios"][0]["argv"] = [
            sys.executable,
            "-c",
            "import time; time.sleep(5)",
        ]
        document["scenarios"][0]["timeout_seconds"] = 0.05
        document["scenarios"][0]["force_stop_seconds"] = 0.5
        (code, public, _), _, _, _ = self.run_fixture(document)
        first = {item["id"]: item for item in public["scenarios"]}["FT-01"]
        self.assertEqual(code, 1)
        self.assertTrue(first["producer"]["timed_out"])
        self.assertGreaterEqual(first["producer"]["termination_attempts"], 1)
        self.assertEqual(first["cleanup"]["status"], "PASS")

    def test_38_config_change_during_run_forces_failure(self) -> None:
        document = self.make_document(same_group=True)
        mutation = (
            "from pathlib import Path\n"
            f"p=Path({str(self.config_path)!r})\n"
            "p.write_bytes(p.read_bytes()+b' ')\n"
        )
        for scenario in document["scenarios"]:
            scenario["argv"][2] = mutation + PRODUCER_CODE
        (code, public, _), _, _, _ = self.run_fixture(document, same_group=True)
        self.assertEqual(code, 1)
        self.assertFalse(public["integrity_checks"]["config_unchanged"])
        self.assertEqual(public["statistics"]["pass"], 0)

    def test_39_public_report_contains_no_argv_cwd_private_paths_or_output(self) -> None:
        (result, public_path, _, _) = self.run_fixture()
        self.assertEqual(result[0], 0)
        text = public_path.read_text(encoding="utf-8")
        self.assertNotIn('"argv"', text)
        self.assertNotIn('"cwd"', text)
        self.assertNotIn(str(self.private_root), text)
        self.assertNotIn("stdout_base64", text)
        self.assertNotIn("private body", text)

    def test_40_verify_accepts_real_complete_pass(self) -> None:
        (result, public_path, private_path, keys) = self.run_fixture()
        self.assertEqual(result[0], 0)
        report = acceptance.verify_acceptance(
            self.config_path,
            self.repo,
            public_path,
            private_path,
            required_ids=self.ids,
            required_names=self.names,
            expected_run_keys=keys,
        )
        self.assertEqual(report["status"], "PASS")

    def test_41_verify_rejects_forged_scenario_pass(self) -> None:
        result, public_path, private_path, keys = self.run_fixture()
        document = read_json(public_path)
        document["scenarios"][0]["producer"]["status"] = "FAIL"
        atomic_write_json(public_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_42_verify_rejects_missing_public_evidence_file(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        (self.public_root / "FT-01/result.public.json").unlink()
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_43_verify_rejects_false_statistics(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        document = read_json(public_path)
        document["statistics"]["pass"] = 28
        atomic_write_json(public_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_44_verify_rejects_wrong_config_hash(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        document = read_json(public_path)
        document["config_sha256"] = "A" * 64
        atomic_write_json(public_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_45_verify_rejects_wrong_candidate_hash(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        document = read_json(public_path)
        document["candidate_sha256"] = "B" * 64
        atomic_write_json(public_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_46_verify_rejects_wrong_source_commit(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        document = read_json(public_path)
        document["source_commit"] = "c" * 40
        atomic_write_json(public_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_47_verify_rejects_private_report_hash_mismatch(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        document = read_json(private_path)
        document["generated_at"] = "changed"
        atomic_write_json(private_path, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_48_verify_recomputes_assertions_from_evidence(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        evidence = self.public_root / "FT-01/result.public.json"
        document = read_json(evidence)
        document["verdict"] = "FAIL"
        atomic_write_json(evidence, document)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_49_public_private_roots_cannot_be_nested(self) -> None:
        nested = self.public_root / "private"
        nested.mkdir()
        self.invalid(
            lambda d: d["evidence_roots"].__setitem__("private", str(nested))
        )

    def test_50_materialized_config_inside_repo_is_rejected(self) -> None:
        document = self.make_document()
        inside = self.repo / "acceptance.json"
        atomic_write_json(inside, document)
        with self.assertRaises(GateError):
            acceptance.validate_config(
                inside,
                self.repo,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=self.run_keys,
            )

    def test_51_build_manifest_real_hash_mismatch_is_rejected(self) -> None:
        self.invalid(lambda d: d["build_manifest"].__setitem__("sha256", "D" * 64))

    def test_52_cleanup_without_command_is_rejected(self) -> None:
        self.invalid(lambda d: d["scenarios"][0]["cleanup"].__setitem__("argv", []))

    def test_53_verify_rejects_missing_private_evidence_file(self) -> None:
        _, public_path, private_path, keys = self.run_fixture()
        (self.private_root / "FT-01/result.private.json").unlink()
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_54_preexisting_declared_evidence_is_rejected_without_touching_it(self) -> None:
        self.write_document(self.make_document())
        existing = self.public_root / "FT-01/result.public.json"
        existing.parent.mkdir(parents=True)
        existing.write_bytes(b"old evidence must remain untouched")
        before = existing.read_bytes()
        with self.assertRaises(GateError):
            acceptance.run_acceptance(
                self.config_path,
                self.repo,
                self.public_root / "acceptance.public.json",
                self.private_root / "acceptance.private.json",
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=self.run_keys,
            )
        self.assertEqual(existing.read_bytes(), before)

    def test_55_cleanup_cannot_rewrite_frozen_business_evidence(self) -> None:
        document = self.make_document()
        first = document["scenarios"][0]
        first["cleanup"]["argv"][-1] = "tamper"
        first["cleanup"]["argv"].append(str(self.public_root / "FT-01/result.public.json"))
        (code, public, _), _, _, _ = self.run_fixture(document)
        by_id = {item["id"]: item for item in public["scenarios"]}
        self.assertEqual(code, 1)
        self.assertEqual(by_id["FT-01"]["status"], "FAIL")
        self.assertEqual(
            by_id["FT-01"]["changed_public_evidence_after_cleanup"],
            ["FT-01/result.public.json"],
        )

    def test_56_formal_template_path_is_fixed(self) -> None:
        expected = self.repo / acceptance.FORMAL_TEMPLATE_RELATIVE_PATH
        expected.parent.mkdir(parents=True)
        expected.write_text("{}", encoding="utf-8")
        self.assertEqual(
            acceptance._require_formal_template_path(self.repo, expected), expected.resolve()
        )
        wrong = self.external / "lookalike-template.json"
        wrong.write_text("{}", encoding="utf-8")
        with self.assertRaises(GateError):
            acceptance._require_formal_template_path(self.repo, wrong)

    def test_57_formal_contract_requires_case_specific_facts_not_verdict(self) -> None:
        pass_assertions, _ = acceptance._formal_assertion_contract(
            "FT-28",
            acceptance.FORMAL_RUN_KEY_BY_ID["FT-28"],
            source_commit="a" * 40,
            candidate_sha256="B" * 64,
            build_manifest_sha256="C" * 64,
            run_id="FIX-FACTS-20260902-02",
            producer_nonce="D" * 64,
            run_registration_sha256="E" * 64,
        )
        pointers = {item["pointer"] for item in pass_assertions}
        self.assertNotIn("/verdict", pointers)
        for check in acceptance.FORMAL_CHECK_KEYS_BY_ID["FT-28"]:
            self.assertIn(f"/checks/{check}", pointers)
        self.assertIn("/machine_run_id", pointers)
        self.assertIn("/producer_nonce", pointers)
        self.assertIn("/run_registration_sha256", pointers)

    def test_58_machine_run_id_must_equal_legacy_run_id(self) -> None:
        self.invalid(lambda d: d.__setitem__("machine_run_id", "FIX-OTHER-20260905-01"))

    def test_59_execution_contract_is_exact(self) -> None:
        self.invalid(
            lambda d: d["execution_contract"].__setitem__(
                "formal_run_scope", "FAILED_CASES_ONLY"
            )
        )

    def test_60_run_cli_has_no_single_formal_case_selector(self) -> None:
        completed = subprocess.run(
            [sys.executable, str(Path(acceptance.__file__).resolve()), "run", "--help"],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
        )
        self.assertEqual(completed.returncode, 0, completed.stdout)
        help_text = completed.stdout.casefold()
        self.assertIn("all 28 cases", help_text)
        self.assertNotIn("--case", help_text)
        self.assertNotIn("--ft", help_text)

    def test_61_group_reports_require_whole_group_rerun(self) -> None:
        (code, public, private), _, _, _ = self.run_fixture(same_group=True)
        self.assertEqual(code, 0)
        for scenario in public["scenarios"]:
            self.assertEqual(
                scenario["rerun_scope_on_failure"], acceptance.GROUP_RERUN_SCOPE
            )
        self.assertEqual(
            private["group_runs"][0]["rerun_scope_on_failure"],
            acceptance.GROUP_RERUN_SCOPE,
        )

    def test_62_verify_rejects_cross_run_spliced_case(self) -> None:
        _, public_path, private_path, keys = self.run_fixture(same_group=True)
        public = read_json(public_path)
        private = read_json(private_path)
        old_machine_run_id = "FIX-OLD-FORMAL-20260905-01"
        public["scenarios"][0]["machine_run_id"] = old_machine_run_id
        private["scenarios"][0]["machine_run_id"] = old_machine_run_id
        atomic_write_json(private_path, private)
        public["private_report_bytes"] = private_path.stat().st_size
        public["private_report_sha256"] = sha256_file(private_path)
        atomic_write_json(public_path, public)
        with self.assertRaises(GateError):
            acceptance.verify_acceptance(
                self.config_path,
                self.repo,
                public_path,
                private_path,
                required_ids=self.ids,
                required_names=self.names,
                expected_run_keys=keys,
            )

    def test_63_group_member_failure_keeps_case_status_but_requires_whole_group_rerun(self) -> None:
        document = self.make_document(same_group=True, producer_behavior="partial")
        (code, public, private), _, _, _ = self.run_fixture(document, same_group=True)
        self.assertEqual(code, 1)
        self.assertEqual(public["statistics"]["pass"], 1)
        self.assertEqual(public["statistics"]["fail"], 1)
        self.assertEqual(public["producer_run_counts"], {"group-one": 1})
        self.assertEqual(
            {item["rerun_scope_on_failure"] for item in public["scenarios"]},
            {acceptance.GROUP_RERUN_SCOPE},
        )
        self.assertEqual(
            private["group_runs"][0]["rerun_scope_on_failure"],
            acceptance.GROUP_RERUN_SCOPE,
        )

    def test_64_formal_dependency_contract_cannot_regress(self) -> None:
        template_path = Path(__file__).resolve().parents[2] / acceptance.FORMAL_TEMPLATE_RELATIVE_PATH
        document = read_json(template_path, "formal template")
        ft26 = next(item for item in document["scenarios"] if item["id"] == "FT-26")
        ft26["dependencies"] = ["FT-21"]
        with self.assertRaises(GateError):
            acceptance._template_shape(
                document,
                acceptance.FORMAL_CASE_IDS,
                acceptance.FORMAL_CASE_NAMES,
                acceptance.FORMAL_RUN_KEY_BY_ID,
            )

    def test_65_tracked_contract_docs_use_one_release_policy(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        docs_root = repo / "target/release/docs/方案/MOSS功能修复计划-20260902"
        documents = [
            docs_root / "TASKS.md",
            docs_root / "ACCEPTANCE-FORMAT.md",
            docs_root / "QUALITY-GATE-FORMAT.md",
            docs_root / "FUNCTIONAL-FT-PRODUCER.md",
        ]
        required = (
            "候选锁定 → 注册 `machine_run_id` → AT-00～AT-02 → Q-00 → `materialize` → FT-01～FT-28",
            "machine_run_id",
            "ui_run_id",
            "同一 `run_once_key`",
            "新证据目录",
        )
        for path in documents:
            text = path.read_text(encoding="utf-8")
            for phrase in required:
                self.assertIn(phrase, text, f"{path.name} missing {phrase}")
            self.assertNotIn("正式 FT 失败：修复后换新运行编号，只重跑失败项", text)
        tasks = documents[0].read_text(encoding="utf-8")
        self.assertIn("AT-09`：`P0", tasks)
        self.assertIn("FT-26`：必须依赖 `FT-22`～`FT-25", tasks)


class CuaAcceptanceTest(unittest.TestCase):
    maxDiff = None

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="moss-cua-contract-test-")
        self.root = Path(self.temporary.name)
        self.private_root = self.root / "private"
        self.public_root = self.root / "public"
        self.registry_root = self.root / "registry"
        for path in (self.private_root, self.public_root, self.registry_root):
            path.mkdir(parents=True)
        self.machine_run_id = "MOSS-FORMAL-20260905-0001"
        self.source_commit = "a" * 40
        self.candidate_sha256 = "b" * 64
        self.main_exe_path = self.root / "installed" / "meetily.exe"
        self.main_exe_path.parent.mkdir(parents=True)
        self.main_exe_path.write_bytes(b"synthetic candidate main executable")
        self.main_exe_sha256 = sha256_file(self.main_exe_path)
        self.registration_sha256 = "d" * 64
        self.version = "0.4.2"
        self.producer_nonces = {
            key: f"{index:064x}"
            for index, key in enumerate(acceptance.FORMAL_RUN_KEYS, start=1)
        }
        self.context = {
            "private_root": self.private_root,
            "public_root": self.public_root,
            "machine_run_id": self.machine_run_id,
            "run_id": self.machine_run_id,
            "source_commit": self.source_commit,
            "candidate": {"sha256": self.candidate_sha256},
            "build_manifest_details": {
                "document": {"version": self.version},
                "installed_files": {
                    "main_executable": {"sha256": self.main_exe_sha256},
                    "uninstaller": {"sha256": "e" * 64},
                },
            },
            "producer_nonces": self.producer_nonces,
            "run_registration": {
                "sha256": self.registration_sha256,
                "document": {
                    "registered_at": "2026-09-05T00:00:00Z",
                    "registration_nonce": "f" * 64,
                },
            },
        }
        self.records: dict[str, Path] = {}
        self.manifests: dict[str, Path] = {}
        self._write_valid_cua_set()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _write_json(path: Path, value: object) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_json(path, value)

    def _window(self, task_id: str) -> dict[str, object]:
        index = int(task_id.split("-")[1]) + 1000
        process_started_at = "2026-09-05T00:00:30Z"
        executable_path = str(self.main_exe_path.resolve())
        process_instance_id = acceptance.sha256_bytes(
            acceptance.canonical_json_bytes(
                {
                    "exe_path": executable_path,
                    "exe_path_sha256": self.main_exe_sha256,
                    "pid": index,
                    "process_started_at": process_started_at,
                }
            )
        )
        return {
            "application": "Meetily",
            "identity_role": "candidate_main",
            "title": f"Meetily acceptance {task_id}",
            "window_id": f"window-{task_id}",
            "pid": index,
            "process_started_at": process_started_at,
            "process_instance_id": process_instance_id,
            "exe_path": executable_path,
            "exe_path_sha256": self.main_exe_sha256,
        }

    def _write_valid_cua_set(self) -> None:
        machine_documents: dict[str, dict[str, object]] = {}
        task_events: dict[str, tuple[str, str]] = {}
        for task_id in acceptance.CUA_TASK_IDS:
            evidence_path = acceptance.CUA_ALLOWED_MACHINE_PATHS_BY_TASK[task_id][0]
            event_id = f"machine-event-{task_id}"
            window = self._window(task_id)
            producer_key = acceptance.CUA_PRODUCER_KEY_BY_MACHINE_PATH.get(evidence_path)
            event = {
                "event_id": event_id,
                "machine_run_id": self.machine_run_id,
                "source_commit": self.source_commit,
                "candidate_sha256": self.candidate_sha256,
                "run_registration_sha256": self.registration_sha256,
                "producer_nonce": (
                    self.producer_nonces[producer_key] if producer_key is not None else None
                ),
                "started_at": "2026-09-05T00:01:00Z",
                "ended_at": "2026-09-05T00:02:00Z",
                "target_window": window,
                "observed_version": self.version,
            }
            document = machine_documents.setdefault(
                evidence_path,
                {
                    "schema_version": 1,
                    "stage": "MOSS_FUNCTIONAL_MACHINE_EVIDENCE",
                    "machine_events": [],
                },
            )
            document["machine_events"].append(event)
            task_events[task_id] = (evidence_path, event_id)

        for relative, document in machine_documents.items():
            self._write_json(self.private_root / relative, document)

        for task_id in acceptance.CUA_TASK_IDS:
            evidence_path, event_id = task_events[task_id]
            machine_path = self.private_root / evidence_path
            window = self._window(task_id)
            record_path = self.private_root / acceptance.CUA_RECORD_PATH_BY_TASK[task_id]
            manifest_path = self.private_root / acceptance.CUA_HASH_MANIFEST_PATH_BY_TASK[task_id]
            record = {
                "schema_version": 1,
                "stage": acceptance.CUA_RECORD_STAGE,
                "task_id": task_id,
                "machine_run_id": self.machine_run_id,
                "ui_run_id": f"UI-{task_id}-20260905-0001",
                "source_commit": self.source_commit,
                "candidate_sha256": self.candidate_sha256,
                "run_registration_sha256": self.registration_sha256,
                "candidate_identity": {
                    "candidate_sha256": self.candidate_sha256,
                    "main_exe_sha256": self.main_exe_sha256,
                    "version": self.version,
                },
                "observed_version": self.version,
                "target_window": window,
                "window_selection": {
                    "enumerated_at": "2026-09-05T00:02:30Z",
                    "matching_window_count": 1,
                    "selected_window_id": window["window_id"],
                    "selection_query": "candidate executable identity and task window",
                    "window_list_sha256": "1" * 64,
                },
                "session_started_at": "2026-09-05T00:03:00Z",
                "session_ended_at": "2026-09-05T00:04:00Z",
                "producer_session_active": False,
                "actions": [
                    {
                        "sequence": 1,
                        "actor": "cua",
                        "started_at": "2026-09-05T00:03:10Z",
                        "ended_at": "2026-09-05T00:03:20Z",
                        "before_state": "unique target window selected",
                        "action": f"perform the required short UI action for {task_id}",
                        "after_state": "task-specific UI state observed",
                        "observed_text": f"verified {task_id}",
                        "bound_machine_event_id": event_id,
                        "machine_evidence_path": evidence_path,
                        "machine_evidence_bytes": machine_path.stat().st_size,
                        "machine_evidence_sha256": sha256_file(machine_path),
                        "codex_tool_record": {
                            "thread_id": "01a00000-0000-7000-8000-000000000001",
                            "turn_id": f"turn-{task_id}",
                            "item_id": f"item-{task_id}-1",
                        },
                    }
                ],
            }
            self._write_json(record_path, record)
            self.records[task_id] = record_path
            self.manifests[task_id] = manifest_path
            self._rewrite_manifest(task_id)

    def _rewrite_manifest(self, task_id: str) -> None:
        record_path = self.records.get(
            task_id, self.private_root / acceptance.CUA_RECORD_PATH_BY_TASK[task_id]
        )
        manifest_path = self.manifests.get(
            task_id, self.private_root / acceptance.CUA_HASH_MANIFEST_PATH_BY_TASK[task_id]
        )
        self._write_json(
            manifest_path,
            {
                "schema_version": 1,
                "stage": acceptance.CUA_HASH_MANIFEST_STAGE,
                "task_id": task_id,
                "machine_run_id": self.machine_run_id,
                "files": [
                    {
                        "path": acceptance.CUA_RECORD_PATH_BY_TASK[task_id],
                        "bytes": record_path.stat().st_size,
                        "sha256": sha256_file(record_path),
                    }
                ],
            },
        )

    def _mutate_record(self, task_id: str, mutate, *, refresh_manifest: bool = True) -> None:
        record = read_json(self.records[task_id])
        mutate(record)
        self._write_json(self.records[task_id], record)
        if refresh_manifest:
            self._rewrite_manifest(task_id)

    def _mutate_machine_event(self, task_id: str, mutate) -> None:
        record = read_json(self.records[task_id])
        action = record["actions"][0]
        relative = action["machine_evidence_path"]
        event_id = action["bound_machine_event_id"]
        machine_path = self.private_root / relative
        machine = read_json(machine_path)
        event = next(item for item in machine["machine_events"] if item["event_id"] == event_id)
        mutate(event)
        self._write_json(machine_path, machine)
        byte_count = machine_path.stat().st_size
        digest = sha256_file(machine_path)
        for linked_task_id, linked_path in self.records.items():
            linked = read_json(linked_path)
            changed = False
            for linked_action in linked["actions"]:
                if linked_action["machine_evidence_path"] == relative:
                    linked_action["machine_evidence_bytes"] = byte_count
                    linked_action["machine_evidence_sha256"] = digest
                    changed = True
            if changed:
                self._write_json(linked_path, linked)
                self._rewrite_manifest(linked_task_id)

    def _rejects(self) -> None:
        with self.assertRaises(GateError):
            acceptance._verify_cua_records(self.context)

    def test_66_valid_22_record_cua_contract_passes(self) -> None:
        report = acceptance._verify_cua_records(self.context)
        self.assertEqual(report["cua_records_verified"], 22)
        self.assertEqual(report["cua_ui_run_id_unique_count"], 22)
        self.assertEqual(report["undeclared_shared_evidence"], 0)

    def test_67_cua_schema_files_are_tracked_and_parseable(self) -> None:
        repo = Path(__file__).resolve().parents[2]
        for relative in (
            acceptance.ACCEPTANCE_SCHEMA_RELATIVE_PATH,
            acceptance.CUA_SCHEMA_RELATIVE_PATH,
        ):
            tracked = subprocess.run(
                ["git", "-C", str(repo), "ls-files", "--error-unmatch", relative],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertEqual(tracked.returncode, 0, f"schema is not tracked: {relative}")
            schema = read_json(repo / relative, relative)
            self.assertEqual(schema["$schema"], "https://json-schema.org/draft/2020-12/schema")
            self.assertEqual(schema["type"], "object")

    def test_68_missing_ui_run_id_is_rejected(self) -> None:
        self._mutate_record("AT-03", lambda d: d.pop("ui_run_id"))
        self._rejects()

    def test_69_wrong_pid_is_rejected(self) -> None:
        self._mutate_record("AT-04", lambda d: d["target_window"].__setitem__("pid", 99999))
        self._rejects()

    def test_70_wrong_exe_sha_is_rejected(self) -> None:
        self._mutate_record(
            "AT-05",
            lambda d: d["target_window"].__setitem__("exe_path_sha256", "9" * 64),
        )
        self._rejects()

    def test_71_wrong_observed_version_is_rejected(self) -> None:
        self._mutate_record("AT-06", lambda d: d.__setitem__("observed_version", "0.4.1"))
        self._rejects()

    def test_72_missing_machine_event_is_rejected(self) -> None:
        self._mutate_record(
            "AT-07",
            lambda d: d["actions"][0].__setitem__("bound_machine_event_id", "missing-event"),
        )
        self._rejects()

    def test_73_duplicate_ui_run_id_reuse_is_rejected(self) -> None:
        reused = read_json(self.records["AT-08"])["ui_run_id"]
        self._mutate_record("AT-09", lambda d: d.__setitem__("ui_run_id", reused))
        self._rejects()

    def test_74_undeclared_machine_evidence_reuse_is_rejected(self) -> None:
        foreign = acceptance.CUA_ALLOWED_MACHINE_PATHS_BY_TASK["AT-21"][0]
        foreign_path = self.private_root / foreign
        foreign_event = read_json(foreign_path)["machine_events"][0]["event_id"]

        def mutate(document):
            action = document["actions"][0]
            action["machine_evidence_path"] = foreign
            action["machine_evidence_bytes"] = foreign_path.stat().st_size
            action["machine_evidence_sha256"] = sha256_file(foreign_path)
            action["bound_machine_event_id"] = foreign_event

        self._mutate_record("AT-03", mutate)
        self._rejects()

    def test_75_different_candidate_reusing_cua_is_rejected(self) -> None:
        self._mutate_record("AT-10", lambda d: d.__setitem__("candidate_sha256", "8" * 64))
        self._rejects()

    def test_76_cua_before_formal_registration_is_rejected(self) -> None:
        self._mutate_record(
            "AT-11",
            lambda d: d.__setitem__("session_started_at", "2026-09-04T23:59:59Z"),
        )
        self._rejects()

    def test_77_pid_reuse_process_instance_mismatch_is_rejected(self) -> None:
        self._mutate_record(
            "AT-12",
            lambda d: d["target_window"].__setitem__(
                "process_instance_id", "same-pid-new-process-start"
            ),
        )
        self._rejects()

    def test_78_cua_hash_manifest_is_recomputed(self) -> None:
        self._mutate_record(
            "AT-13",
            lambda d: d["actions"][0].__setitem__("observed_text", "tampered after hash"),
            refresh_manifest=False,
        )
        self._rejects()

    def test_79_duplicate_producer_nonce_is_rejected(self) -> None:
        duplicate = dict(self.producer_nonces)
        duplicate[acceptance.FORMAL_RUN_KEYS[-1]] = duplicate[acceptance.FORMAL_RUN_KEYS[0]]
        with self.assertRaises(GateError):
            acceptance._validate_producer_nonces(duplicate)

    def test_80_formal_assertion_contract_registers_performance_tail_and_process_keys(self) -> None:
        contracts = {
            "FT-01": {
                "/checks/pause_observed",
                "/checks/transcript_continues_after_resume",
                "/checks/tail_marker_present_exactly_once",
                "/metrics/tail_coverage_ratio",
                "/metrics/chunks_in_queue_at_finalization",
            },
            "FT-02": {
                "/metrics/stop_feedback_seconds",
                "/metrics/page_unlock_seconds",
            },
            "FT-04": {
                "/metrics/enhance_feedback_seconds",
                "/metrics/whisper_enhance_elapsed_seconds",
            },
            "FT-05": {
                "/metrics/cancel_feedback_seconds",
                "/metrics/residual_process_count",
            },
            "FT-12": {
                "/metrics/summary_elapsed_seconds",
                "/metrics/summary_feedback_seconds",
            },
            "FT-21": {
                "/metrics/moss_and_qwen_overlap_seconds",
                "/metrics/residual_process_count",
            },
            "FT-27": {
                "/metrics/tail_coverage_ratio",
                "/metrics/chunks_in_queue_at_finalization",
            },
            "FT-28": {
                "/metrics/moss_and_qwen_overlap_seconds",
                "/metrics/residual_process_count",
            },
        }
        for case_id, expected in contracts.items():
            pass_assertions, _ = acceptance._formal_assertion_contract(
                case_id,
                acceptance.FORMAL_RUN_KEY_BY_ID[case_id],
                source_commit="a" * 40,
                candidate_sha256="b" * 64,
                build_manifest_sha256="c" * 64,
                run_id="MOSS-FORMAL-20260905-0001",
                producer_nonce="d" * 64,
                run_registration_sha256="e" * 64,
            )
            pointers = {item["pointer"] for item in pass_assertions}
            self.assertTrue(expected.issubset(pointers), f"{case_id}: {expected - pointers}")

    def test_81_run_registry_rejects_duplicate_run_id_even_with_new_roots(self) -> None:
        first_public = self.root / "first-public"
        first_private = self.root / "first-private"
        second_public = self.root / "second-public"
        second_private = self.root / "second-private"
        for path in (first_public, first_private, second_public, second_private):
            path.mkdir()
        acceptance._create_run_registration(
            registry_root=self.registry_root,
            run_id=self.machine_run_id,
            source_commit=self.source_commit,
            candidate_sha256=self.candidate_sha256,
            build_manifest_sha256="6" * 64,
            public_root=first_public,
            private_root=first_private,
            registered_at="2026-09-05T00:00:00Z",
            registration_nonce="7" * 64,
        )
        with self.assertRaises(GateError):
            acceptance._create_run_registration(
                registry_root=self.registry_root,
                run_id=self.machine_run_id,
                source_commit=self.source_commit,
                candidate_sha256=self.candidate_sha256,
                build_manifest_sha256="6" * 64,
                public_root=second_public,
                private_root=second_private,
                registered_at="2026-09-05T00:01:00Z",
                registration_nonce="8" * 64,
            )

    def test_82_nonexistent_window_executable_is_rejected(self) -> None:
        window = self._window("AT-03")
        window["exe_path"] = str(self.root / "missing" / "meetily.exe")
        with self.assertRaises(GateError):
            acceptance._validate_window_identity(window, "test window")

    def test_83_machine_event_cannot_overlap_the_cua_session(self) -> None:
        self._mutate_machine_event(
            "AT-03", lambda event: event.__setitem__("ended_at", "2026-09-05T00:03:05Z")
        )
        self._rejects()

    def test_84_obsolete_machine_run_id_cannot_be_registered(self) -> None:
        public_root = self.root / "obsolete-public"
        private_root = self.root / "obsolete-private"
        public_root.mkdir()
        private_root.mkdir()
        with self.assertRaises(GateError):
            acceptance._create_run_registration(
                registry_root=self.registry_root,
                run_id=acceptance.OLD_RUN_ID,
                source_commit=self.source_commit,
                candidate_sha256=self.candidate_sha256,
                build_manifest_sha256="6" * 64,
                public_root=public_root,
                private_root=private_root,
                registered_at="2026-09-05T00:00:00Z",
                registration_nonce="7" * 64,
            )

    def test_85_process_instance_id_must_be_derived_from_pid_start_and_executable(self) -> None:
        window = self._window("AT-03")
        window["process_instance_id"] = "not-derived-from-the-process-facts"
        with self.assertRaises(GateError):
            acceptance._validate_window_identity(window, "test window")


class TrackedWorkspaceBindingTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="moss-tracked-path-test-")
        self.repo = Path(self.temporary.name) / "repo"
        self.repo.mkdir()
        self._git("init", "-q")
        self._git("config", "user.email", "qa@example.invalid")
        self._git("config", "user.name", "QA")
        self._git("config", "core.quotePath", "true")
        (self.repo / "tracked.txt").write_text("frozen source\n", encoding="utf-8")
        self._git("add", "tracked.txt")
        self._git("commit", "-q", "-m", "fixture")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _git(self, *args: str) -> str:
        completed = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        return completed.stdout

    def test_86_DEBUG_chinese_tracked_path_is_bound_with_quote_path_enabled(self) -> None:
        relative_path = "target/release/docs/方案/DEBUG-P0-中文已跟踪.txt"
        tracked_path = self.repo / relative_path
        tracked_path.parent.mkdir(parents=True)
        tracked_path.write_text("frozen 中文 source\n", encoding="utf-8")
        self._git("add", "--", relative_path)
        self._git("commit", "-q", "-m", "add DEBUG Chinese tracked fixture")

        binding = acceptance._tracked_workspace_binding(
            self.repo, relative_path, "DEBUG-P0 Chinese tracked fixture"
        )

        self.assertEqual(binding["relative_path"], relative_path)
        self.assertEqual(
            binding["git_blob"], self._git("rev-parse", f"HEAD:{relative_path}").strip()
        )
        self.assertEqual(binding["workspace_sha256"], sha256_file(tracked_path))

    def test_87_DEBUG_chinese_untracked_workspace_path_is_rejected(self) -> None:
        relative_path = "target/release/docs/方案/DEBUG-P0-中文未跟踪.txt"
        untracked_path = self.repo / relative_path
        untracked_path.parent.mkdir(parents=True)
        untracked_path.write_text("untracked 中文 source\n", encoding="utf-8")

        with self.assertRaisesRegex(
            GateError, "DEBUG-P0 Chinese untracked fixture failed"
        ):
            acceptance._tracked_workspace_binding(
                self.repo, relative_path, "DEBUG-P0 Chinese untracked fixture"
            )

    def test_88_DEBUG_chinese_tracked_workspace_blob_must_match_head(self) -> None:
        relative_path = "target/release/docs/方案/DEBUG-P0-中文内容漂移.txt"
        tracked_path = self.repo / relative_path
        tracked_path.parent.mkdir(parents=True)
        tracked_path.write_text("frozen 中文 source\n", encoding="utf-8")
        self._git("add", "--", relative_path)
        self._git("commit", "-q", "-m", "add DEBUG Chinese drift fixture")
        tracked_path.write_text("changed 中文 workspace source\n", encoding="utf-8")

        with self.assertRaisesRegex(
            GateError, "workspace bytes do not match the current HEAD blob"
        ):
            acceptance._tracked_workspace_binding(
                self.repo, relative_path, "DEBUG-P0 Chinese changed fixture"
            )


class BuildManifestProducerContractTest(unittest.TestCase):
    MANIFEST_PRODUCER_RELATIVE_PATH = "scripts/qa/new-install-build-manifest.ps1"
    WEBVIEW2_CACHE_ROOT = r"D:\MeetilyData\build-deps\webview2-fixed"

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="moss-build-manifest-producer-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "release-repo"
        self.evidence_root = self.root / "DEBUG-P0-build-evidence"
        self.repo.mkdir()
        self.evidence_root.mkdir()
        self._git("init", "-q")
        self._git("config", "user.email", "qa@example.invalid")
        self._git("config", "user.name", "QA")

        self.manifest_producer = self._write_repo_file(
            self.MANIFEST_PRODUCER_RELATIVE_PATH, b"official manifest producer\n"
        )
        self.build_producer = self._write_repo_file(
            acceptance.BUILD_PRODUCER_RELATIVE_PATH, b"controlled build producer\n"
        )
        self.rollback_tool = self._write_repo_file(
            acceptance.ROLLBACK_TOOL_RELATIVE_PATH, b"fixed rollback tool\n"
        )
        self.artifact_paths = {
            role: self._write_repo_file(relative, f"{role} artifact\n".encode("utf-8"))
            for role, relative in acceptance.BUILD_ARTIFACT_ROLE_PATHS.items()
        }
        self.candidate = self._write_repo_file(
            "target/release/bundle/nsis/meetily_0.4.2_x64-setup.exe",
            b"candidate installer\n",
        )
        self._git("add", "--", ".")
        self._git("commit", "-q", "-m", "DEBUG official build manifest fixture")
        self.source_commit = self._git("rev-parse", "HEAD").strip()

        self.build_log = self.evidence_root / "candidate-build.log"
        self.build_log.write_bytes(b"controlled build log\n")
        self.attestation_path = self.evidence_root / "candidate-build-attestation.private.json"
        self.manifest_path = self.evidence_root / "candidate-build-manifest.private.json"
        self.attestation_document = self._official_attestation_document()
        atomic_write_json(self.attestation_path, self.attestation_document)
        self.manifest_document = self._official_manifest_document()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _git(self, *args: str) -> str:
        completed = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
        return completed.stdout

    def _write_repo_file(self, relative_path: str, content: bytes) -> Path:
        path = self.repo / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return path

    @staticmethod
    def _evidence(path: Path, relative_path: str, *, include_path: bool = False) -> dict[str, object]:
        record: dict[str, object] = {
            "relative_path": relative_path,
            "bytes": path.stat().st_size,
            "sha256": sha256_file(path),
        }
        if include_path:
            record["path"] = str(path.resolve())
        return record

    def _official_attestation_document(self) -> dict[str, object]:
        artifacts = [
            {
                "role": role,
                **self._evidence(self.artifact_paths[role], relative_path),
            }
            for role, relative_path in acceptance.BUILD_ARTIFACT_ROLE_PATHS.items()
        ]
        artifacts.append(
            {
                "role": "nsis_installer",
                **self._evidence(
                    self.candidate, self.candidate.relative_to(self.repo).as_posix()
                ),
            }
        )
        return {
            "schema_version": 1,
            "stage": acceptance.BUILD_ATTESTATION_STAGE,
            "status": "PASS",
            "source_commit": self.source_commit,
            "repository_head_before": self.source_commit,
            "repository_head_after": self.source_commit,
            "worktree_clean_before": True,
            "worktree_clean_after": True,
            "product_name": acceptance.APPROVED_PRODUCT_NAME,
            "bundle_id": acceptance.APPROVED_BUNDLE_ID,
            "version": "0.4.2",
            "started_at": "2026-09-05T00:00:00Z",
            "completed_at": "2026-09-05T00:01:00Z",
            "commands": list(acceptance.APPROVED_BUILD_COMMANDS),
            "executable": str(self.artifact_paths["main_executable"].resolve()),
            "environment": {
                **acceptance.APPROVED_BUILD_ENVIRONMENT,
                "MEETILY_WEBVIEW2_CACHE_ROOT": self.WEBVIEW2_CACHE_ROOT,
            },
            "producer": self._evidence(
                self.build_producer, acceptance.BUILD_PRODUCER_RELATIVE_PATH
            ),
            "build_log": self._evidence(
                self.build_log, "candidate-build.log", include_path=True
            ),
            "artifacts": artifacts,
        }

    def _official_manifest_document(self) -> dict[str, object]:
        installed_files = [
            {
                "role": role,
                "relative_path": relative_path,
                "bytes": index,
                "sha256": f"{index:064X}",
            }
            for index, (role, relative_path) in enumerate(
                acceptance.INSTALLED_ROLE_PATHS.items(), start=1
            )
        ]
        return {
            "schema_version": 1,
            "stage": acceptance.BUILD_MANIFEST_STAGE,
            "status": "PASS",
            "role": "candidate",
            "generated_at": "2026-09-05T00:02:00Z",
            "source_commit": self.source_commit,
            "product_name": acceptance.APPROVED_PRODUCT_NAME,
            "bundle_id": acceptance.APPROVED_BUNDLE_ID,
            "version": "0.4.2",
            "installer": self._evidence(self.candidate, self.candidate.name),
            "installed_files": installed_files,
            "producer": self._evidence(
                self.manifest_producer, self.MANIFEST_PRODUCER_RELATIVE_PATH
            ),
            "rollback_tool": self._evidence(
                self.rollback_tool, acceptance.ROLLBACK_TOOL_RELATIVE_PATH
            ),
            "build_provenance": {
                "evidence": self._evidence(
                    self.attestation_path,
                    "candidate-build-attestation.private.json",
                    include_path=True,
                ),
                "build_log": self._evidence(
                    self.build_log, "candidate-build.log", include_path=True
                ),
                "commands": list(acceptance.APPROVED_BUILD_COMMANDS),
                "artifact_count": 7,
            },
        }

    def _validate(self) -> dict[str, object]:
        atomic_write_json(self.manifest_path, self.manifest_document)
        bound_manifest = {
            "path": self.manifest_path,
            "bytes": self.manifest_path.stat().st_size,
            "sha256": sha256_file(self.manifest_path),
        }
        candidate = {
            "path": self.candidate,
            "bytes": self.candidate.stat().st_size,
            "sha256": sha256_file(self.candidate),
        }
        return acceptance._validate_build_manifest(
            bound_manifest, candidate, self.repo, self.source_commit
        )

    def _refresh_attestation_evidence(self) -> None:
        atomic_write_json(self.attestation_path, self.attestation_document)
        provenance = self.manifest_document["build_provenance"]
        assert isinstance(provenance, dict)
        provenance["evidence"] = self._evidence(
            self.attestation_path,
            "candidate-build-attestation.private.json",
            include_path=True,
        )

    def _manifest_producer_record(self) -> dict[str, object]:
        producer = self.manifest_document["producer"]
        assert isinstance(producer, dict)
        return producer

    def _provenance_record(self, field: str) -> dict[str, object]:
        provenance = self.manifest_document["build_provenance"]
        assert isinstance(provenance, dict)
        record = provenance[field]
        assert isinstance(record, dict)
        return record

    def _environment(self) -> dict[str, object]:
        environment = self.attestation_document["environment"]
        assert isinstance(environment, dict)
        return environment

    def test_89_DEBUG_official_candidate_manifest_with_producer_is_accepted(self) -> None:
        details = self._validate()
        self.assertEqual(
            details["producer"]["relative_path"], self.MANIFEST_PRODUCER_RELATIVE_PATH
        )

    def test_90_DEBUG_candidate_manifest_without_producer_is_rejected(self) -> None:
        self.manifest_document.pop("producer")
        with self.assertRaisesRegex(
            GateError, "candidate build manifest fields are not exact"
        ):
            self._validate()

    def test_91_DEBUG_candidate_manifest_producer_path_is_fixed(self) -> None:
        self._manifest_producer_record()["relative_path"] = "scripts/qa/lookalike.ps1"
        with self.assertRaisesRegex(
            GateError, "candidate build manifest producer relative_path is not the fixed path"
        ):
            self._validate()

    def test_92_DEBUG_candidate_manifest_producer_bytes_match_real_file(self) -> None:
        producer = self._manifest_producer_record()
        producer["bytes"] = int(producer["bytes"]) + 1
        with self.assertRaisesRegex(
            GateError, "candidate build manifest producer bytes or sha256 do not match the real file"
        ):
            self._validate()

    def test_93_DEBUG_candidate_manifest_producer_hash_matches_real_file(self) -> None:
        self._manifest_producer_record()["sha256"] = "0" * 64
        with self.assertRaisesRegex(
            GateError, "candidate build manifest producer bytes or sha256 do not match the real file"
        ):
            self._validate()

    def test_94_DEBUG_candidate_manifest_producer_workspace_blob_matches_head(self) -> None:
        self.manifest_producer.write_bytes(b"changed workspace manifest producer\n")
        self.manifest_document["producer"] = self._evidence(
            self.manifest_producer, self.MANIFEST_PRODUCER_RELATIVE_PATH
        )
        with self.assertRaisesRegex(
            GateError, "workspace bytes do not match the current HEAD blob"
        ):
            self._validate()

    def test_95_DEBUG_build_provenance_evidence_requires_absolute_path(self) -> None:
        self._provenance_record("evidence").pop("path")
        with self.assertRaisesRegex(GateError, "build provenance evidence path is required"):
            self._validate()

    def test_96_DEBUG_build_provenance_log_requires_absolute_path(self) -> None:
        self._provenance_record("build_log").pop("path")
        with self.assertRaisesRegex(GateError, "build provenance log path is required"):
            self._validate()

    def test_97_DEBUG_build_provenance_evidence_rejects_wrong_absolute_path(self) -> None:
        wrong_path = self.evidence_root / "wrong-attestation.private.json"
        wrong_path.write_text("{}", encoding="utf-8")
        self._provenance_record("evidence")["path"] = str(wrong_path.resolve())
        with self.assertRaisesRegex(
            GateError, "build provenance evidence path does not match the required file"
        ):
            self._validate()

    def test_98_DEBUG_build_provenance_log_rejects_escaping_path(self) -> None:
        self._provenance_record("build_log")["path"] = "../escaped-build.log"
        with self.assertRaisesRegex(
            GateError, "build provenance log path must be an absolute path"
        ):
            self._validate()

    def test_99_DEBUG_build_environment_requires_webview2_cache_root(self) -> None:
        self._environment().pop("MEETILY_WEBVIEW2_CACHE_ROOT")
        self._refresh_attestation_evidence()
        with self.assertRaisesRegex(
            GateError, "build attestation environment fields are not exact"
        ):
            self._validate()

    def test_100_DEBUG_build_environment_rejects_wrong_webview2_cache_root(self) -> None:
        self._environment()["MEETILY_WEBVIEW2_CACHE_ROOT"] = r"D:\wrong\webview2-fixed"
        self._refresh_attestation_evidence()
        with self.assertRaisesRegex(
            GateError, "candidate build attestation environment is not approved"
        ):
            self._validate()

    def test_101_DEBUG_build_environment_rejects_extra_key(self) -> None:
        self._environment()["UNAPPROVED_BUILD_SETTING"] = "forbidden"
        self._refresh_attestation_evidence()
        with self.assertRaisesRegex(
            GateError, "build attestation environment fields are not exact"
        ):
            self._validate()


if __name__ == "__main__":
    unittest.main(verbosity=2)
