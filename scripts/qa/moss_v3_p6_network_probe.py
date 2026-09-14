#!/usr/bin/env python3
"""Produce direct-IP evidence that a P6 test lane has or lacks network egress."""

from __future__ import annotations

import argparse
import errno
import ipaddress
import json
from pathlib import Path
import socket
import subprocess
import sys
import time
from typing import Any

from moss_v3_p6_common import (
    FAIL,
    PASS,
    GateError,
    atomic_write_json,
    canonical_json_bytes,
    require_git_commit,
    sha256_bytes,
    status_exit_code,
    utc_now,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def git_head(repo: Path) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "HEAD"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise GateError(f"Cannot resolve Git HEAD: {completed.stderr.strip()}")
    return require_git_commit(completed.stdout.strip(), "Git HEAD")


def parse_endpoint(value: str) -> tuple[ipaddress.IPv4Address | ipaddress.IPv6Address, int]:
    text = value.strip()
    if text.startswith("["):
        closing = text.find("]")
        if closing < 0 or closing + 1 >= len(text) or text[closing + 1] != ":":
            raise GateError(f"invalid bracketed IPv6 endpoint: {value!r}")
        host, raw_port = text[1:closing], text[closing + 2 :]
    else:
        if text.count(":") != 1:
            raise GateError(f"endpoint must be IPv4:port or [IPv6]:port: {value!r}")
        host, raw_port = text.rsplit(":", 1)
    try:
        address = ipaddress.ip_address(host)
        port = int(raw_port)
    except ValueError as exc:
        raise GateError(f"invalid direct-IP endpoint: {value!r}") from exc
    if not address.is_global:
        raise GateError(f"network probe endpoint must be a public IP address: {address}")
    if not 1 <= port <= 65535:
        raise GateError(f"network probe port is out of range: {port}")
    return address, port


def probe_endpoint(
    address: ipaddress.IPv4Address | ipaddress.IPv6Address, port: int, timeout: float
) -> dict[str, Any]:
    family = socket.AF_INET6 if address.version == 6 else socket.AF_INET
    started = time.perf_counter()
    reachable = False
    tcp_connected = False
    error_type: str | None = None
    probe = socket.socket(family, socket.SOCK_STREAM)
    try:
        probe.settimeout(timeout)
        target: tuple[Any, ...]
        if address.version == 6:
            target = (str(address), port, 0, 0)
        else:
            target = (str(address), port)
        probe.connect(target)
        reachable = True
        tcp_connected = True
    except OSError as exc:
        error_type = type(exc).__name__
        # A refusal/reset is still proof that the packet reached a network
        # stack.  Treat it as egress, conservatively failing blocked mode.
        reachable_errnos = {errno.ECONNREFUSED, errno.ECONNRESET, errno.ECONNABORTED}
        reachable_winerrors = {10053, 10054, 10061}
        reachable = exc.errno in reachable_errnos or getattr(exc, "winerror", None) in reachable_winerrors
    finally:
        probe.close()
    return {
        "address": str(address),
        "port": port,
        "reachable": reachable,
        "tcp_connected": tcp_connected,
        "error_type": error_type,
        "elapsed_seconds": round(time.perf_counter() - started, 3),
    }


def run_probe(args: argparse.Namespace) -> int:
    if not 0.1 <= float(args.timeout_seconds) <= 30.0:
        raise GateError("timeout_seconds must be between 0.1 and 30")
    parsed = [parse_endpoint(value) for value in args.endpoint]
    canonical_endpoints = [(str(address), port) for address, port in parsed]
    distinct_addresses = {address for address, _ in canonical_endpoints}
    if (
        len(parsed) < 3
        or len(set(canonical_endpoints)) != len(parsed)
        or len(distinct_addresses) < 3
    ):
        raise GateError("network evidence requires at least three distinct public IP addresses")
    results = [
        probe_endpoint(address, port, float(args.timeout_seconds)) for address, port in parsed
    ]
    reachable_count = sum(1 for result in results if result["reachable"])
    connected_count = sum(
        1 for result in results if result.get("tcp_connected", result["reachable"])
    )
    if args.mode == "blocked":
        passed = reachable_count == 0
    else:
        passed = connected_count == len(results)
    status = PASS if passed else FAIL
    contract = {
        "endpoints": [f"[{address}]:{port}" if ":" in address else f"{address}:{port}" for address, port in canonical_endpoints],
        "timeout_seconds": float(args.timeout_seconds),
    }
    report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_NETWORK_PROBE",
        "generated_at": utc_now(),
        "source_commit": git_head(args.repo.resolve()),
        "status": status,
        "expected_state": args.mode,
        "network_contract": contract,
        "network_contract_sha256": sha256_bytes(canonical_json_bytes(contract)),
        "reachable_count": reachable_count,
        "connected_count": connected_count,
        "endpoint_count": len(results),
        "results": results,
    }
    atomic_write_json(args.output.resolve(), report)
    print(
        json.dumps(
            {
                "status": status,
                "expected_state": args.mode,
                "reachable_count": reachable_count,
                "connected_count": connected_count,
                "output": str(args.output.resolve()),
            },
            ensure_ascii=False,
        )
    )
    return status_exit_code(status)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--mode", choices=["reachable", "blocked"], required=True)
    parser.add_argument("--endpoint", action="append", default=[], metavar="PUBLIC_IP:PORT")
    parser.add_argument("--timeout-seconds", type=float, default=3.0)
    parser.add_argument("--output", type=Path, required=True)
    return parser


def main() -> int:
    try:
        return run_probe(build_parser().parse_args())
    except GateError as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
