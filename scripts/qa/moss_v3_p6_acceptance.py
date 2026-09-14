#!/usr/bin/env python3
"""Run the hash-bound MOSS v3 P6 end-to-end acceptance command matrix.

The runner never uses a shell.  Exact commands and raw logs are private; the
public report contains only executable names, counts, hashes, timings, exit
codes, and evidence-file hashes.  P0-P5 stage gates must all be PASS and merged
before any product scenario can run.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
from typing import Any, Iterable

from moss_v3_p6_common import (
    FAIL,
    NOT_RUN,
    PASS,
    REQUIRED_P6_SCENARIOS,
    GateError,
    atomic_write_json,
    canonical_json_bytes,
    file_record,
    normalize_relative_path,
    read_json,
    require_list,
    require_mapping,
    require_string,
    resolve_under,
    sha256_bytes,
    sha256_file,
    status_exit_code,
    utc_now,
)
from moss_v3_p6_release import (
    REQUIRED_STAGES,
    git_head,
    parse_stage_gate_arguments,
    validate_release_dependency_chain,
    validate_stage_gate,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


REQUIRED_SCENARIOS = REQUIRED_P6_SCENARIOS
SAFE_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")


def require_safe_id(value: Any, label: str) -> str:
    text = require_string(value, label)
    if not SAFE_ID_RE.fullmatch(text) or text in {".", ".."}:
        raise GateError(f"{label} must be a filesystem-safe identifier")
    return text


def validate_argv(value: Any, label: str) -> list[str]:
    raw = require_list(value, label)
    if not raw or not all(isinstance(item, str) and item for item in raw):
        raise GateError(f"{label} must be a non-empty array of non-empty strings")
    return list(raw)


def validate_expected_exit_codes(value: Any, label: str) -> list[int]:
    raw = require_list(value, label)
    if not raw or not all(isinstance(item, int) and not isinstance(item, bool) for item in raw):
        raise GateError(f"{label} must be a non-empty array of integer exit codes")
    return sorted(set(raw))


def command_definition(value: Any, label: str, repo: Path) -> dict[str, Any]:
    command = require_mapping(value, label)
    command_id = require_safe_id(command.get("id"), f"{label} id")
    argv = validate_argv(command.get("argv"), f"{label} argv")
    cwd_value = command.get("cwd", str(repo))
    cwd = Path(require_string(cwd_value, f"{label} cwd")).resolve()
    if not cwd.is_dir():
        raise GateError(f"{label} cwd is not a directory: {cwd}")
    expected_exit_codes = validate_expected_exit_codes(
        command.get("expected_exit_codes", [0]), f"{label} expected_exit_codes"
    )
    timeout_seconds = command.get("timeout_seconds", 300)
    if (
        isinstance(timeout_seconds, bool)
        or not isinstance(timeout_seconds, (int, float))
        or not 0.1 <= float(timeout_seconds) <= 86_400
    ):
        raise GateError(f"{label} timeout_seconds must be between 0.1 and 86400")
    raw_env = require_mapping(command.get("env", {}), f"{label} env")
    env: dict[str, str] = {}
    for key, raw_value in raw_env.items():
        name = require_string(key, f"{label} env name")
        if not isinstance(raw_value, str):
            raise GateError(f"{label} env {name} must be a string")
        env[name] = raw_value
    return {
        "id": command_id,
        "argv": argv,
        "cwd": cwd,
        "expected_exit_codes": expected_exit_codes,
        "timeout_seconds": float(timeout_seconds),
        "env": env,
    }


def scenario_definition(value: Any, repo: Path) -> dict[str, Any]:
    raw = require_mapping(value, "acceptance scenario")
    scenario_id = require_safe_id(raw.get("id"), "scenario id")
    main = command_definition(
        {
            "id": "main",
            "argv": raw.get("argv"),
            "cwd": raw.get("cwd", str(repo)),
            "expected_exit_codes": raw.get("expected_exit_codes", [0]),
            "timeout_seconds": raw.get("timeout_seconds", 300),
            "env": raw.get("env", {}),
        },
        f"scenario {scenario_id}",
        repo,
    )
    prechecks = [
        command_definition(item, f"scenario {scenario_id} precheck", repo)
        for item in require_list(raw.get("prechecks", []), f"scenario {scenario_id} prechecks")
    ]
    postchecks = [
        command_definition(item, f"scenario {scenario_id} postcheck", repo)
        for item in require_list(raw.get("postchecks", []), f"scenario {scenario_id} postchecks")
    ]
    check_ids = [item["id"] for item in [*prechecks, *postchecks]]
    if len(check_ids) != len(set(check_ids)):
        raise GateError(f"scenario {scenario_id} has duplicate pre/postcheck ids")
    if scenario_id == "offline-release-chain":
        if "network-blocked-before" not in {item["id"] for item in prechecks}:
            raise GateError("offline-release-chain requires precheck network-blocked-before")
        if "network-blocked-after" not in {item["id"] for item in postchecks}:
            raise GateError("offline-release-chain requires postcheck network-blocked-after")
    raw_evidence = require_list(raw.get("evidence_files", []), f"scenario {scenario_id} evidence_files")
    evidence_files = [
        normalize_relative_path(item, f"scenario {scenario_id} evidence file")
        for item in raw_evidence
    ]
    if len(evidence_files) != len(set(evidence_files)):
        raise GateError(f"scenario {scenario_id} has duplicate evidence files")
    return {
        "id": scenario_id,
        "main": main,
        "prechecks": prechecks,
        "postchecks": postchecks,
        "evidence_files": evidence_files,
    }


def resolve_executable(argv: list[str], env: dict[str, str], cwd: Path) -> list[str]:
    first = Path(argv[0])
    if first.is_absolute() or first.parent != Path("."):
        executable = (first if first.is_absolute() else cwd / first).resolve()
        if not executable.is_file():
            raise GateError(f"Executable is missing: {executable}")
        return [str(executable), *argv[1:]]
    resolved = shutil.which(argv[0], path=env.get("PATH"))
    if resolved is None:
        raise GateError(f"Executable is not on PATH: {argv[0]}")
    return [resolved, *argv[1:]]


def terminate_process_tree(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/PID", str(process.pid), "/T", "/F"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_one_command(
    command: dict[str, Any],
    *,
    scenario_id: str,
    position: str,
    private_log_dir: Path,
) -> tuple[dict[str, Any], dict[str, Any]]:
    environment = os.environ.copy()
    environment.update(command["env"])
    argv = resolve_executable(command["argv"], environment, command["cwd"])
    executable_record = file_record(Path(argv[0]), relative_path=Path(argv[0]).name)
    log_name = f"{scenario_id}.{position}.{command['id']}.log"
    log_path = private_log_dir / log_name
    creation_flags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    started = time.perf_counter()
    process = subprocess.Popen(
        argv,
        cwd=command["cwd"],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        creationflags=creation_flags,
        start_new_session=os.name != "nt",
    )
    timed_out = False
    try:
        stdout, _ = process.communicate(timeout=command["timeout_seconds"])
    except subprocess.TimeoutExpired:
        timed_out = True
        terminate_process_tree(process)
        stdout, _ = process.communicate()
    elapsed = time.perf_counter() - started
    log_path.write_bytes(stdout)
    exit_code = process.returncode
    passed = not timed_out and exit_code in command["expected_exit_codes"]
    env_hashes = {
        name: sha256_bytes(value.encode("utf-8")) for name, value in sorted(command["env"].items())
    }
    common = {
        "id": command["id"],
        "status": PASS if passed else FAIL,
        "exit_code": exit_code,
        "expected_exit_codes": command["expected_exit_codes"],
        "timed_out": timed_out,
        "elapsed_seconds": round(elapsed, 3),
        "stdout_bytes": len(stdout),
        "stdout_sha256": sha256_bytes(stdout),
        "log_bytes": log_path.stat().st_size,
        "log_sha256": sha256_file(log_path),
        "executable": Path(argv[0]).name,
        "executable_bytes": executable_record["bytes"],
        "executable_sha256": executable_record["sha256"],
        "argv_sha256": sha256_bytes(canonical_json_bytes(argv)),
        "cwd_sha256": sha256_bytes(str(command["cwd"]).encode("utf-8")),
        "env_override_names": sorted(command["env"]),
        "env_override_value_hashes": env_hashes,
    }
    public = dict(common)
    private = {
        **common,
        "argv": argv,
        "cwd": str(command["cwd"]),
        "private_log_path": str(log_path),
    }
    return public, private


def validate_offline_network_evidence(
    evidence_paths: list[tuple[Path, str]], expected_commit: str
) -> tuple[list[dict[str, Any]], list[str]]:
    summaries: list[dict[str, Any]] = []
    errors: list[str] = []
    for path, relative in evidence_paths:
        try:
            raw = read_json(path, f"offline evidence {relative}")
        except GateError:
            continue
        if not isinstance(raw, dict) or raw.get("stage") != "MOSS_V3_P6_NETWORK_PROBE":
            continue
        try:
            if raw.get("schema_version") != 1 or raw.get("source_commit") != expected_commit:
                raise GateError("network report schema/source commit mismatch")
            if raw.get("status") != PASS or raw.get("expected_state") not in {
                "reachable",
                "blocked",
            }:
                raise GateError("network report is not a valid PASS control")
            contract = require_mapping(raw.get("network_contract"), "network contract")
            contract_sha256 = sha256_bytes(canonical_json_bytes(contract))
            if raw.get("network_contract_sha256") != contract_sha256:
                raise GateError("network contract hash mismatch")
            endpoints = require_list(contract.get("endpoints"), "network endpoints")
            results = [
                require_mapping(item, "network endpoint result")
                for item in require_list(raw.get("results"), "network results")
            ]
            endpoint_count = raw.get("endpoint_count")
            if (
                isinstance(endpoint_count, bool)
                or not isinstance(endpoint_count, int)
                or endpoint_count < 3
                or len(endpoints) != endpoint_count
                or len(results) != endpoint_count
            ):
                raise GateError("network report endpoint count is invalid")
            reachable_count = sum(1 for item in results if item.get("reachable") is True)
            connected_count = sum(1 for item in results if item.get("tcp_connected") is True)
            if raw.get("reachable_count") != reachable_count or raw.get(
                "connected_count"
            ) != connected_count:
                raise GateError("network report counters do not match endpoint results")
            if raw["expected_state"] == "reachable" and connected_count != endpoint_count:
                raise GateError("reachable control did not connect to every endpoint")
            if raw["expected_state"] == "blocked" and reachable_count != 0:
                raise GateError("blocked control still had network reachability")
            summaries.append(
                {
                    "path": relative,
                    "expected_state": raw["expected_state"],
                    "network_contract_sha256": contract_sha256,
                    "sha256": sha256_file(path),
                }
            )
        except GateError as exc:
            errors.append(f"{relative}: {exc}")

    reachable = [item for item in summaries if item["expected_state"] == "reachable"]
    blocked = [item for item in summaries if item["expected_state"] == "blocked"]
    if len(reachable) != 1 or len(blocked) != 2:
        errors.append("offline evidence requires one reachable control and two blocked controls")
    contracts = {str(item["network_contract_sha256"]) for item in summaries}
    if len(contracts) != 1:
        errors.append("offline network controls must use the same endpoint contract")
    return summaries, errors


def capture_evidence_files(
    scenario: dict[str, Any], expected_commit: str
) -> tuple[list[dict[str, Any]], list[str], list[dict[str, Any]], list[str]]:
    records: list[dict[str, Any]] = []
    missing: list[str] = []
    evidence_paths: list[tuple[Path, str]] = []
    root = scenario["main"]["cwd"]
    for relative in scenario["evidence_files"]:
        path, normalized = resolve_under(root, relative, f"scenario {scenario['id']} evidence")
        if path.is_file():
            records.append(file_record(path, relative_path=normalized))
            evidence_paths.append((path, normalized))
        else:
            missing.append(normalized)
    network_summaries: list[dict[str, Any]] = []
    validation_errors: list[str] = []
    if scenario["id"] == "offline-release-chain" and not missing:
        network_summaries, validation_errors = validate_offline_network_evidence(
            evidence_paths, expected_commit
        )
    return records, missing, network_summaries, validation_errors


def validate_scenarios(
    config: dict[str, Any], repo: Path, required_scenarios: Iterable[str]
) -> list[dict[str, Any]]:
    raw_scenarios = require_list(config.get("scenarios"), "acceptance scenarios")
    scenarios = [scenario_definition(value, repo) for value in raw_scenarios]
    ids = [item["id"] for item in scenarios]
    if len(ids) != len(set(ids)):
        raise GateError("acceptance scenario ids must be unique")
    required = set(required_scenarios)
    if set(ids) != required:
        raise GateError(
            f"acceptance config scenario set mismatch: missing={sorted(required - set(ids))}, "
            f"unexpected={sorted(set(ids) - required)}"
        )
    return scenarios


def run_suite(args: argparse.Namespace, *, required_scenarios: Iterable[str] = REQUIRED_SCENARIOS) -> int:
    repo = args.repo.resolve()
    head = git_head(repo)
    config_path = args.config.resolve()
    config_sha256_before = sha256_file(config_path)
    config = require_mapping(read_json(config_path, "acceptance config"), "acceptance config")
    config_sha256 = sha256_file(config_path)
    if config_sha256_before != config_sha256:
        raise GateError("acceptance config changed while it was being read")
    if config.get("template_only") is True:
        raise GateError("acceptance template must be copied and fully resolved before execution")
    if config.get("schema_version") != 1:
        raise GateError("acceptance config schema_version must be 1")
    scenarios = validate_scenarios(config, repo, required_scenarios)

    stage_paths = parse_stage_gate_arguments(args.stage_gate)
    stages: dict[str, dict[str, Any]] = {}
    blockers: list[str] = []
    for stage in REQUIRED_STAGES:
        path = stage_paths.get(stage)
        if path is None:
            stages[stage] = {"status": NOT_RUN, "reason": "gate not supplied"}
            blockers.append(f"{stage}_GATE_NOT_SUPPLIED")
            continue
        try:
            stages[stage] = validate_stage_gate(path, stage, repo, head)
            if stages[stage]["status"] != PASS:
                blockers.append(f"{stage}_STATUS_{stages[stage]['status'].replace(' ', '_')}")
        except GateError as exc:
            stages[stage] = {"status": FAIL, "reason": str(exc)}
            blockers.append(f"{stage}_GATE_INVALID")

    dependency_chain_valid, dependency_chain_errors = validate_release_dependency_chain(
        stages, repo, head
    )
    if not dependency_chain_valid:
        blockers.extend(dependency_chain_errors)

    public_output = args.public_output.resolve()
    private_output = args.private_output.resolve()
    private_log_dir = args.private_log_dir.resolve()
    if public_output == private_output:
        raise GateError("public and private acceptance reports must use different files")
    if public_output in {config_path} or private_output in {config_path}:
        raise GateError("acceptance outputs cannot overwrite the input config")
    try:
        public_output.relative_to(private_log_dir)
    except ValueError:
        pass
    else:
        raise GateError("public acceptance report cannot be written inside the private log directory")
    private_log_dir.mkdir(parents=True, exist_ok=True)
    public_results: list[dict[str, Any]] = []
    private_results: list[dict[str, Any]] = []

    if blockers:
        status = NOT_RUN
    else:
        for scenario in scenarios:
            public_commands: list[dict[str, Any]] = []
            private_commands: list[dict[str, Any]] = []
            pre_passed = True
            for command in scenario["prechecks"]:
                public, private = run_one_command(
                    command,
                    scenario_id=scenario["id"],
                    position="pre",
                    private_log_dir=private_log_dir,
                )
                public_commands.append(public)
                private_commands.append(private)
                pre_passed = pre_passed and public["status"] == PASS
            main_ran = False
            if pre_passed:
                main_ran = True
                public, private = run_one_command(
                    scenario["main"],
                    scenario_id=scenario["id"],
                    position="main",
                    private_log_dir=private_log_dir,
                )
                public_commands.append(public)
                private_commands.append(private)
            for command in scenario["postchecks"]:
                public, private = run_one_command(
                    command,
                    scenario_id=scenario["id"],
                    position="post",
                    private_log_dir=private_log_dir,
                )
                public_commands.append(public)
                private_commands.append(private)
            (
                evidence_records,
                missing_evidence,
                network_evidence,
                evidence_validation_errors,
            ) = capture_evidence_files(scenario, head)
            scenario_passed = (
                main_ran
                and all(command["status"] == PASS for command in public_commands)
                and not missing_evidence
                and not evidence_validation_errors
            )
            common = {
                "id": scenario["id"],
                "status": PASS if scenario_passed else FAIL,
                "commands": public_commands,
                "evidence_files": evidence_records,
                "missing_evidence_files": missing_evidence,
                "offline_network_evidence": network_evidence,
                "evidence_validation_errors": evidence_validation_errors,
            }
            public_results.append(common)
            private_results.append({**common, "commands": private_commands})
        status = PASS if all(item["status"] == PASS for item in public_results) else FAIL

    if sha256_file(config_path) != config_sha256:
        blockers.append("ACCEPTANCE_CONFIG_CHANGED_DURING_RUN")
        status = FAIL if public_results else NOT_RUN
    for stage, path in stage_paths.items():
        original = stages.get(stage, {})
        if original.get("status") == FAIL:
            continue
        try:
            current = validate_stage_gate(path, stage, repo, head)
            unchanged = current == original
        except GateError:
            unchanged = False
        if not unchanged:
            blockers.append(f"{stage}_GATE_CHANGED_DURING_RUN")
            status = FAIL if public_results else NOT_RUN

    private_report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_ACCEPTANCE_PRIVATE",
        "generated_at": utc_now(),
        "source_commit": head,
        "status": status,
        "config_path": str(config_path),
        "config_sha256": config_sha256,
        "stage_gates": stages,
        "release_dependency_chain_valid": dependency_chain_valid,
        "dependency_chain_errors": dependency_chain_errors,
        "blockers": blockers,
        "scenarios": private_results,
    }
    atomic_write_json(private_output, private_report)
    public_report = {
        "schema_version": 1,
        "stage": "MOSS_V3_P6_ACCEPTANCE",
        "generated_at": utc_now(),
        "source_commit": head,
        "status": status,
        "config_sha256": config_sha256,
        "private_report_bytes": private_output.stat().st_size,
        "private_report_sha256": sha256_file(private_output),
        "stage_gates": stages,
        "release_dependency_chain_valid": dependency_chain_valid,
        "dependency_chain_errors": dependency_chain_errors,
        "blockers": blockers,
        "required_scenarios": sorted(required_scenarios),
        "scenarios": public_results,
        "privacy_rule": "Raw command output and exact local paths exist only in the private report/log directory.",
    }
    atomic_write_json(public_output, public_report)
    print(
        json.dumps(
            {
                "status": status,
                "scenario_count": len(public_results),
                "blockers": blockers,
                "public_output": str(public_output),
                "private_output": str(private_output),
            },
            ensure_ascii=False,
        )
    )
    return status_exit_code(status)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--stage-gate", action="append", default=[], metavar="STAGE=PATH")
    parser.add_argument("--public-output", type=Path, required=True)
    parser.add_argument("--private-output", type=Path, required=True)
    parser.add_argument("--private-log-dir", type=Path, required=True)
    return parser


def main() -> int:
    try:
        return run_suite(build_parser().parse_args())
    except GateError as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
