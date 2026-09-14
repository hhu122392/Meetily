#!/usr/bin/env python3
"""Exercise the exact formal-run firewall add/check/remove lifecycle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


RUN_ID_PATTERN = re.compile(r"^[0-9a-f]{32}$")


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def write_new_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_name(path.name + ".partial")
    encoded = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    with partial.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(partial, path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--gate-root", type=Path, required=True)
    parser.add_argument("--scorer-root", type=Path, required=True)
    parser.add_argument("--powershell", type=Path, required=True)
    parser.add_argument("--locked-python", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if not RUN_ID_PATTERN.fullmatch(args.run_id):
        raise SystemExit("run-id must contain exactly 32 lowercase hexadecimal characters")
    output = args.output.resolve(strict=False)
    if output.exists() or output.with_name(output.name + ".partial").exists():
        raise SystemExit("preflight output already exists")

    gate_root = args.gate_root.resolve(strict=True)
    scorer_root = args.scorer_root.resolve(strict=True)
    sys.path.insert(0, str(gate_root))
    sys.path.insert(1, str(scorer_root))
    import supervise_windows_cuda_gate_r3 as gate

    powershell = args.powershell.resolve(strict=True)
    locked_python = args.locked_python.resolve(strict=True)
    started_at = now_iso()
    firewall: dict[str, Any] | None = None
    removal: dict[str, Any] | None = None
    failure_type: str | None = None
    failure_message: str | None = None
    try:
        firewall = gate.firewall_add(
            args.run_id,
            powershell,
            locked_python,
            protected_programs=[locked_python],
        )
    except BaseException as exc:
        failure_type = type(exc).__name__
        failure_message = str(exc)
    finally:
        if firewall is not None:
            try:
                removal = gate.firewall_remove(firewall, powershell)
            except BaseException as exc:
                failure_type = failure_type or type(exc).__name__
                failure_message = failure_message or str(exc)

    rule_names = [
        f"Meetily-R3-{args.run_id}-P01-IN",
        f"Meetily-R3-{args.run_id}-P01-OUT",
    ]
    final_snapshot_error_type: str | None = None
    try:
        final_snapshot = gate._firewall_snapshot(powershell, rule_names)
    except BaseException as exc:
        final_snapshot_error_type = type(exc).__name__
        final_snapshot = {"records": ["QUERY_FAILED"]}

    passed = bool(
        failure_type is None
        and firewall is not None
        and firewall.get("scope")
        == "PROGRAM_SCOPED_ALL_PROFILES_BOTH_DIRECTIONS"
        and firewall.get("protected_program_count") == 1
        and firewall.get("outbound_block_probe", {}).get(
            "blocked_by_firewall_policy"
        )
        is True
        and removal is not None
        and removal.get("removed") is True
        and not final_snapshot.get("records")
        and final_snapshot_error_type is None
        and gate.firewall_platform_is_effective(final_snapshot)
    )
    audit = {
        "schema_version": 1,
        "role": "FORMAL_FIREWALL_LIFECYCLE_PREFLIGHT",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": now_iso(),
        "status": "PASS" if passed else "FAIL_CLOSED",
        "go_allowed": passed,
        "failure_type": failure_type,
        "failure_message": failure_message,
        "gate_path_sha256": sha256_file(Path(gate.__file__).resolve(strict=True)),
        "preflight_path_sha256": sha256_file(Path(__file__).resolve(strict=True)),
        "installed_rule_count": (
            len(firewall.get("installed", {}).get("records", []))
            if firewall
            else 0
        ),
        "is_program_scoped": (
            firewall.get("scope")
            == "PROGRAM_SCOPED_ALL_PROFILES_BOTH_DIRECTIONS"
            if firewall
            else None
        ),
        "protected_program_count": (
            firewall.get("protected_program_count") if firewall else None
        ),
        "outbound_block_probe_passed": (
            firewall.get("outbound_block_probe", {}).get(
                "blocked_by_firewall_policy"
            )
            if firewall
            else None
        ),
        "removal_evidence_present": removal is not None,
        "removal_verified": removal.get("removed") if removal else None,
        "removal_attempt_count": removal.get("attempt_count") if removal else None,
        "removal_attempts": removal.get("attempts") if removal else None,
        "final_remaining_rule_count": len(final_snapshot.get("records", [])),
        "final_snapshot_error_type": final_snapshot_error_type,
        "firewall_platform_effective_after_removal": (
            gate.firewall_platform_is_effective(final_snapshot)
            if final_snapshot_error_type is None
            else False
        ),
        "transcript_content_accessed": False,
    }
    write_new_json(output, audit)
    print(json.dumps({"status": audit["status"], "run_id": args.run_id}))
    return 0 if passed else 2


if __name__ == "__main__":
    raise SystemExit(main())
