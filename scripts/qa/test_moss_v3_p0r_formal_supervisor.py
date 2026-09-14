#!/usr/bin/env python3

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
FORMAL_ROOT = ROOT / "scripts" / "qa" / "moss_v3_p0r_formal"
SCORER_ROOT = (
    ROOT
    / "target"
    / "release"
    / "docs"
    / "方案"
    / "证据"
    / "MOSS-V3-P0R-BALANCED-226S-20260830-R3"
)
sys.path.insert(0, str(FORMAL_ROOT))
sys.path.insert(1, str(SCORER_ROOT))

import supervise_windows_cuda_gate_r3 as gate  # noqa: E402


def snapshot(records: list[dict[str, str]], *, effective: bool = True) -> dict:
    service_status = "Running" if effective else "Stopped"
    return {
        "rule_names": ["Meetily-R3-test-IN", "Meetily-R3-test-OUT"],
        "records": records,
        "services": [
            {"Name": "BFE", "Status": service_status},
            {"Name": "MpsSvc", "Status": service_status},
        ],
        "profiles": [
            {"Name": "Domain", "Enabled": True},
            {"Name": "Private", "Enabled": True},
            {"Name": "Public", "Enabled": True},
        ],
    }


class FirewallRemovalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.evidence = {
            "rule_names": ["Meetily-R3-test-IN", "Meetily-R3-test-OUT"]
        }

    @mock.patch.object(gate.time, "sleep")
    @mock.patch.object(gate, "_firewall_snapshot")
    @mock.patch.object(gate.subprocess, "run")
    def test_retries_until_windows_reports_rules_absent(
        self,
        run: mock.Mock,
        firewall_snapshot: mock.Mock,
        sleep: mock.Mock,
    ) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, b"", b"")
        firewall_snapshot.side_effect = [
            snapshot([{"DisplayName": "Meetily-R3-test-IN"}]),
            snapshot([]),
        ]

        result = gate.firewall_remove(self.evidence, Path("powershell.exe"))

        self.assertTrue(result["removed"])
        self.assertEqual(result["attempt_count"], 2)
        self.assertEqual(result["after"]["records"], [])
        self.assertEqual(run.call_count, 2)
        for call in run.call_args_list:
            command = call.args[0][-1]
            self.assertTrue(command.endswith("-ErrorAction Stop}}"))
            self.assertNotIn("-ErrorAction Stop}}}", command)
        sleep.assert_called_once_with(0.5)

    @mock.patch.object(gate.time, "sleep")
    @mock.patch.object(gate.time, "monotonic", side_effect=[0.0, 21.0])
    @mock.patch.object(gate, "_firewall_snapshot")
    @mock.patch.object(gate.subprocess, "run")
    def test_does_not_claim_success_when_firewall_platform_is_not_effective(
        self,
        run: mock.Mock,
        firewall_snapshot: mock.Mock,
        monotonic: mock.Mock,
        sleep: mock.Mock,
    ) -> None:
        run.return_value = subprocess.CompletedProcess([], 0, b"", b"")
        firewall_snapshot.return_value = snapshot([], effective=False)

        result = gate.firewall_remove(self.evidence, Path("powershell.exe"))

        self.assertFalse(result["removed"])
        self.assertEqual(result["attempt_count"], 1)
        self.assertEqual(result["after"]["records"], [])
        sleep.assert_not_called()


class ProgramScopedFirewallTests(unittest.TestCase):
    @mock.patch.object(gate, "outbound_block_probe")
    @mock.patch.object(gate, "_firewall_snapshot")
    @mock.patch.object(gate.subprocess, "run")
    def test_formal_rules_block_only_named_programs(
        self,
        run: mock.Mock,
        firewall_snapshot: mock.Mock,
        outbound_probe: mock.Mock,
    ) -> None:
        stable_exe = Path(
            r"D:\MeetilyData\apps\Meetily-0.4.1-portable\meetily.exe"
        ).resolve(strict=True)
        locked_python = (
            ROOT / ".tools" / "moss-firewall-probe" / "Scripts" / "python.exe"
        ).resolve(strict=True)
        run_id = "a" * 32
        installed_records = []
        for index, program in enumerate((stable_exe, locked_python), start=1):
            for direction, short_direction in (
                ("Inbound", "IN"),
                ("Outbound", "OUT"),
            ):
                installed_records.append(
                    {
                        "DisplayName": (
                            f"Meetily-R3-{run_id}-P{index:02d}-{short_direction}"
                        ),
                        "Enabled": "True",
                        "Direction": direction,
                        "Action": "Block",
                        "Profile": "Any",
                        "ProgramScope": "ExactPath",
                        "ProgramPathSha256": gate.canonical_path_sha256(program),
                    }
                )
        installed_records.sort(key=lambda item: item["DisplayName"])
        firewall_snapshot.side_effect = [
            snapshot([]),
            snapshot(installed_records),
        ]
        run.return_value = subprocess.CompletedProcess([], 0, b"", b"")
        outbound_probe.return_value = {
            "blocked_by_firewall_policy": True,
            "exit_code": 0,
        }

        result = gate.firewall_add(
            run_id,
            Path("powershell.exe"),
            locked_python,
            protected_programs=[stable_exe, locked_python],
        )

        self.assertEqual(
            result["scope"],
            "PROGRAM_SCOPED_ALL_PROFILES_BOTH_DIRECTIONS",
        )
        self.assertEqual(result["protected_program_count"], 2)
        self.assertEqual(len(result["rule_names"]), 4)
        command = run.call_args.args[0][-1]
        self.assertIn(f"-Program '{stable_exe}'", command)
        self.assertIn(f"-Program '{locked_python}'", command)
        self.assertNotIn("New-NetFirewallRule -DisplayName 'Meetily-R3-" + run_id + "-IN'", command)

    def test_program_scope_rejects_missing_probe_program(self) -> None:
        stable_exe = Path(
            r"D:\MeetilyData\apps\Meetily-0.4.1-portable\meetily.exe"
        ).resolve(strict=True)
        locked_python = (
            ROOT / ".tools" / "moss-firewall-probe" / "Scripts" / "python.exe"
        ).resolve(strict=True)

        with self.assertRaises(gate.scorer.ScoringError):
            gate.firewall_add(
                "b" * 32,
                Path("powershell.exe"),
                locked_python,
                protected_programs=[stable_exe],
            )


if __name__ == "__main__":
    unittest.main()
