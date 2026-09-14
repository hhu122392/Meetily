from __future__ import annotations

import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from moss_v3_p6_common import FAIL, PASS, GateError
from moss_v3_p6_network_probe import parse_endpoint, run_probe


REPO = Path(__file__).resolve().parents[2]
ENDPOINTS = ["1.1.1.1:443", "8.8.8.8:443", "9.9.9.9:443"]


class NetworkProbeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def args(self, mode: str) -> SimpleNamespace:
        return SimpleNamespace(
            repo=REPO,
            mode=mode,
            endpoint=ENDPOINTS,
            timeout_seconds=1.0,
            output=self.root / f"{mode}.json",
        )

    @patch("moss_v3_p6_network_probe.probe_endpoint")
    def test_blocked_requires_every_endpoint_to_be_unreachable(self, probe: object) -> None:
        probe.side_effect = [
            {"reachable": False},
            {"reachable": False},
            {"reachable": False},
        ]
        args = self.args("blocked")
        self.assertEqual(run_probe(args), 0)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], PASS)
        self.assertEqual(report["reachable_count"], 0)

    @patch("moss_v3_p6_network_probe.probe_endpoint")
    def test_one_reachable_endpoint_fails_blocked_mode(self, probe: object) -> None:
        probe.side_effect = [
            {"reachable": False},
            {"reachable": True},
            {"reachable": False},
        ]
        args = self.args("blocked")
        self.assertEqual(run_probe(args), 1)
        report = json.loads(args.output.read_text(encoding="utf-8"))
        self.assertEqual(report["status"], FAIL)

    @patch("moss_v3_p6_network_probe.probe_endpoint")
    def test_reachable_control_requires_every_endpoint(self, probe: object) -> None:
        probe.side_effect = [
            {"reachable": True},
            {"reachable": True},
            {"reachable": True},
        ]
        self.assertEqual(run_probe(self.args("reachable")), 0)

    def test_private_or_duplicate_endpoints_are_rejected(self) -> None:
        with self.assertRaises(GateError):
            parse_endpoint("127.0.0.1:443")
        args = self.args("blocked")
        args.endpoint = ["1.1.1.1:443"] * 3
        with self.assertRaises(GateError):
            run_probe(args)


if __name__ == "__main__":
    unittest.main()
