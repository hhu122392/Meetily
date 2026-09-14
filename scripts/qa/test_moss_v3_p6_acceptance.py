from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest

from moss_v3_p6_acceptance import run_suite, validate_scenarios
from moss_v3_p6_common import (
    FAIL,
    NOT_RUN,
    PASS,
    GateError,
    canonical_json_bytes,
    sha256_bytes,
    sha256_file,
)
from moss_v3_p6_release import REQUIRED_STAGES, git_head


REPO = Path(__file__).resolve().parents[2]


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


class AcceptanceToolTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.head = git_head(REPO)
        self.scenario_root = self.root / "scenario"
        self.scenario_root.mkdir()
        self.config = self.root / "acceptance.json"
        self.public = self.root / "public.json"
        self.private = self.root / "private.json"
        self.logs = self.root / "logs"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def make_config(self, *, exit_code: int = 0, scenario_id: str = "fixture") -> None:
        secret_text = "PRIVATE_TRANSCRIPT_SHOULD_NOT_BE_PUBLIC"
        script = (
            "from pathlib import Path; "
            "Path('result.json').write_text('evidence', encoding='utf-8'); "
            f"print('{secret_text}'); raise SystemExit({exit_code})"
        )
        write_json(
            self.config,
            {
                "schema_version": 1,
                "scenarios": [
                    {
                        "id": scenario_id,
                        "argv": [sys.executable, "-c", script],
                        "cwd": str(self.scenario_root),
                        "expected_exit_codes": [0],
                        "timeout_seconds": 10,
                        "evidence_files": ["result.json"],
                    }
                ],
            },
        )

    def make_stage_gates(self) -> list[str]:
        arguments: list[str] = []
        for stage in REQUIRED_STAGES:
            directory = self.root / f"gate-{stage}"
            directory.mkdir()
            proof = directory / "proof.json"
            write_json(proof, {"stage": stage, "proof": True})
            gate = directory / "gate.json"
            write_json(
                gate,
                {
                    "schema_version": 1,
                    "stage": f"MOSS_V3_{stage}",
                    "status": PASS,
                    "source_commit": self.head,
                    "files": [
                        {
                            "path": proof.name,
                            "bytes": proof.stat().st_size,
                            "sha256": sha256_file(proof),
                        }
                    ],
                },
            )
            arguments.append(f"{stage}={gate}")
        return arguments

    def args(self, stage_gates: list[str]) -> SimpleNamespace:
        return SimpleNamespace(
            repo=REPO,
            config=self.config,
            stage_gate=stage_gates,
            public_output=self.public,
            private_output=self.private,
            private_log_dir=self.logs,
        )

    def test_no_stage_gates_means_not_run_and_command_is_not_executed(self) -> None:
        self.make_config()
        result = run_suite(self.args([]), required_scenarios={"fixture"})
        self.assertEqual(result, 2)
        self.assertFalse((self.scenario_root / "result.json").exists())
        public = json.loads(self.public.read_text(encoding="utf-8"))
        self.assertEqual(public["status"], NOT_RUN)
        self.assertIn("P3_GATE_NOT_SUPPLIED", public["blockers"])
        self.assertEqual(public["scenarios"], [])

    def test_pass_records_exact_command_privately_and_hashes_publicly(self) -> None:
        self.make_config()
        result = run_suite(self.args(self.make_stage_gates()), required_scenarios={"fixture"})
        self.assertEqual(result, 0)
        public_text = self.public.read_text(encoding="utf-8")
        public = json.loads(public_text)
        private = json.loads(self.private.read_text(encoding="utf-8"))
        self.assertEqual(public["status"], PASS)
        self.assertEqual(public["scenarios"][0]["status"], PASS)
        self.assertNotIn("PRIVATE_TRANSCRIPT_SHOULD_NOT_BE_PUBLIC", public_text)
        self.assertNotIn(str(self.scenario_root), public_text)
        self.assertEqual(public["private_report_sha256"], sha256_file(self.private))
        self.assertEqual(
            public["scenarios"][0]["commands"][0]["executable_sha256"],
            sha256_file(Path(sys.executable).resolve()),
        )
        private_command = private["scenarios"][0]["commands"][0]
        self.assertIn("PRIVATE_TRANSCRIPT_SHOULD_NOT_BE_PUBLIC", " ".join(private_command["argv"]))
        log = Path(private_command["private_log_path"])
        self.assertIn("PRIVATE_TRANSCRIPT_SHOULD_NOT_BE_PUBLIC", log.read_text(encoding="utf-8"))

    def test_executed_failure_is_fail_not_not_run(self) -> None:
        self.make_config(exit_code=7)
        result = run_suite(self.args(self.make_stage_gates()), required_scenarios={"fixture"})
        self.assertEqual(result, 1)
        public = json.loads(self.public.read_text(encoding="utf-8"))
        self.assertEqual(public["status"], FAIL)
        command = public["scenarios"][0]["commands"][0]
        self.assertEqual(command["exit_code"], 7)
        self.assertEqual(command["status"], FAIL)

    def test_offline_scenario_requires_before_and_after_network_proof(self) -> None:
        self.make_config(scenario_id="offline-release-chain")
        config = json.loads(self.config.read_text(encoding="utf-8"))
        with self.assertRaises(GateError):
            validate_scenarios(config, REPO, {"offline-release-chain"})

        check = {
            "argv": [sys.executable, "-c", "raise SystemExit(0)"],
            "cwd": str(self.scenario_root),
            "expected_exit_codes": [0],
            "timeout_seconds": 10,
        }
        config["scenarios"][0]["prechecks"] = [{"id": "network-blocked-before", **check}]
        config["scenarios"][0]["postchecks"] = [{"id": "network-blocked-after", **check}]
        config["scenarios"][0]["evidence_files"] = [
            "network-reachable-control.json",
            "network-blocked-before.json",
            "result.json",
            "network-blocked-after.json",
        ]
        contract = {
            "endpoints": ["1.1.1.1:443", "8.8.8.8:443", "9.9.9.9:443"],
            "timeout_seconds": 1.0,
        }
        contract_sha256 = sha256_bytes(canonical_json_bytes(contract))

        def network_report(expected_state: str) -> dict[str, object]:
            reachable = expected_state == "reachable"
            results = [
                {"reachable": reachable, "tcp_connected": reachable} for _ in range(3)
            ]
            return {
                "schema_version": 1,
                "stage": "MOSS_V3_P6_NETWORK_PROBE",
                "source_commit": self.head,
                "status": PASS,
                "expected_state": expected_state,
                "network_contract": contract,
                "network_contract_sha256": contract_sha256,
                "reachable_count": 3 if reachable else 0,
                "connected_count": 3 if reachable else 0,
                "endpoint_count": 3,
                "results": results,
            }

        write_json(
            self.scenario_root / "network-reachable-control.json",
            network_report("reachable"),
        )
        write_json(
            self.scenario_root / "network-blocked-before.json", network_report("blocked")
        )
        write_json(
            self.scenario_root / "network-blocked-after.json", network_report("blocked")
        )
        write_json(self.config, config)
        result = run_suite(
            self.args(self.make_stage_gates()), required_scenarios={"offline-release-chain"}
        )
        self.assertEqual(result, 0)
        public = json.loads(self.public.read_text(encoding="utf-8"))
        command_ids = [item["id"] for item in public["scenarios"][0]["commands"]]
        self.assertEqual(
            command_ids, ["network-blocked-before", "main", "network-blocked-after"]
        )
        self.assertEqual(
            len(public["scenarios"][0]["offline_network_evidence"]), 3
        )
        self.assertEqual(public["scenarios"][0]["evidence_validation_errors"], [])

    def test_missing_evidence_file_fails_even_when_command_returns_zero(self) -> None:
        self.make_config()
        config = json.loads(self.config.read_text(encoding="utf-8"))
        config["scenarios"][0]["evidence_files"] = ["missing.json"]
        write_json(self.config, config)
        result = run_suite(self.args(self.make_stage_gates()), required_scenarios={"fixture"})
        self.assertEqual(result, 1)
        public = json.loads(self.public.read_text(encoding="utf-8"))
        self.assertEqual(public["scenarios"][0]["missing_evidence_files"], ["missing.json"])

    def test_scenario_and_check_ids_cannot_escape_private_log_directory(self) -> None:
        self.make_config(scenario_id="../escape")
        config = json.loads(self.config.read_text(encoding="utf-8"))
        with self.assertRaises(GateError):
            validate_scenarios(config, REPO, {"../escape"})


if __name__ == "__main__":
    unittest.main()
