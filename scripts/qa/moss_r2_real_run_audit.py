#!/usr/bin/env python3
"""Independently recheck the R2 737.728 second single-session evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
from pathlib import Path


EXPECTED_FIXTURE_SHA256 = "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
EXPECTED_MODEL_SHA256 = "64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039"
EXPECTED_R1_PRIVATE_SHA256 = "28463C1376A954973EABC866C352357F0B8D74CDF52C5417A1D6A08C116AD5C2"


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest().upper()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest().upper()


def verify_sidecar(path: Path) -> str:
    actual = sha256_file(path)
    sidecar = path.with_suffix(path.suffix + ".sha256")
    expected = sidecar.read_text(encoding="ascii").split()[0].upper()
    if actual != expected:
        raise AssertionError(f"SHA-256 sidecar mismatch: {path.name}")
    return actual


def artifact_matches(path: Path, record: dict[str, object]) -> bool:
    return path.stat().st_size == record["bytes"] and sha256_file(path) == record["sha256"]


def write_new(path: Path, value: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    with path.open("xb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())
    digest = sha256_bytes(payload)
    with path.with_suffix(path.suffix + ".sha256").open(
        "x", encoding="ascii", newline="\n"
    ) as stream:
        stream.write(f"{digest}  {path.name}\n")
        stream.flush()
        os.fsync(stream.fileno())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binding", type=Path, required=True)
    parser.add_argument("--fields", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--test-executable", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--runtime-manifest", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--r1-private", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()

    binding_sha256 = verify_sidecar(arguments.binding)
    fields_sha256 = verify_sidecar(arguments.fields)
    evidence_sha256 = verify_sidecar(arguments.evidence)
    binding = json.loads(arguments.binding.read_text(encoding="utf-8"))
    fields = json.loads(arguments.fields.read_text(encoding="utf-8"))
    evidence_text = arguments.evidence.read_text(encoding="utf-8")
    evidence = json.loads(evidence_text)
    records = binding["artifacts"]
    runs = evidence.get("helper_runs", [])
    limits = evidence.get("native_session_limits", {})

    r1_private_sha256 = sha256_file(arguments.r1_private)
    r1 = json.loads(arguments.r1_private.read_text(encoding="utf-8"))
    r1_raw_sha256 = sha256_bytes(r1["output"]["raw_text"].encode("utf-8"))
    r1_clean_sha256 = sha256_bytes(r1["output"]["text"].encode("utf-8"))

    checks = {
        "binding_sidecar": binding_sha256 == fields["binding_sha256"],
        "binding_fields_sidecar_present": len(fields_sha256) == 64,
        "evidence_sidecar": len(evidence_sha256) == 64,
        "test_executable_bound": artifact_matches(arguments.test_executable, records["test_executable"]),
        "helper_bound": artifact_matches(arguments.helper, records["moss_helper"]),
        "runtime_manifest_bound": artifact_matches(arguments.runtime_manifest, records["runtime_manifest"]),
        "model_bound": artifact_matches(arguments.model, records["q8_model"]),
        "fixture_bound": artifact_matches(arguments.fixture, records["frozen_fixture"]),
        "model_is_frozen_q8": records["q8_model"]["sha256"] == EXPECTED_MODEL_SHA256,
        "fixture_is_frozen_737s": records["frozen_fixture"]["sha256"] == EXPECTED_FIXTURE_SHA256,
        "evidence_status_pass": evidence.get("status") == "PASS",
        "duration_exact": evidence.get("duration_ms") == 737_728,
        "one_helper_run": len(runs) == 1,
        "one_input_range": bool(runs) and runs[0].get("chunkIndex") == 0 and runs[0].get("inputOffsetMs") == 0 and runs[0].get("inputDurationMs") == 737_728,
        "one_terminal": bool(runs) and runs[0].get("terminalCount") == 1,
        "helper_pid_present": evidence.get("helper_process_id", 0) > 0,
        "no_residual_process": evidence.get("residual_process_count") == 0 and bool(runs) and runs[0].get("residualProcessCount") == 0,
        "wall_rtf_pass": 0 <= evidence.get("supervisor_wall_rtf", 2) <= 1.0,
        "native_rtf_pass": 0 <= evidence.get("native_rtf", 2) <= 1.0,
        "actual_n_ctx_16384": limits.get("effective_n_ctx") == 16_384,
        "actual_audio_capacity_covers_input": limits.get("effective_max_audio_ms", 0) >= 737_728,
        "actual_kv_capacity_present": limits.get("max_kv_bytes", 0) > 0,
        "tail_present": 720_000 <= evidence.get("last_timestamp_ms", -1) <= 737_728,
        "r1_private_hash_frozen": r1_private_sha256 == EXPECTED_R1_PRIVATE_SHA256,
        "raw_output_matches_r1": evidence.get("raw_text_sha256") == r1_raw_sha256,
        "clean_output_matches_r1": evidence.get("clean_text_sha256") == r1_clean_sha256,
        "no_transcript_payload": evidence.get("transcript_in_evidence") is False and not any(key in evidence for key in ("raw_text", "clean_text", "segments")),
        "no_absolute_path": evidence.get("absolute_paths_in_evidence") is False and re.search(r"[A-Za-z]:\\\\", evidence_text) is None,
        "dirty_source_state_disclosed": evidence.get("source_tree_clean") is binding.get("source_tree_clean"),
    }
    status = "PASS" if all(checks.values()) else "FAIL"
    output: dict[str, object] = {
        "schema_version": 1,
        "stage": "MOSS_R2_737S_SINGLE_NATIVE_SESSION_INDEPENDENT_AUDIT",
        "status": status,
        "checks": checks,
        "check_count": len(checks),
        "passed_check_count": sum(checks.values()),
        "binding_sha256": binding_sha256,
        "binding_fields_sha256": fields_sha256,
        "real_run_evidence_sha256": evidence_sha256,
        "source_tree_clean": binding.get("source_tree_clean"),
        "raw_text_sha256": evidence.get("raw_text_sha256"),
        "clean_text_sha256": evidence.get("clean_text_sha256"),
        "r1_raw_text_sha256": r1_raw_sha256,
        "r1_clean_text_sha256": r1_clean_sha256,
        "transcript_in_evidence": False,
        "absolute_paths_in_evidence": False,
    }
    write_new(arguments.output, output)
    if status != "PASS":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
