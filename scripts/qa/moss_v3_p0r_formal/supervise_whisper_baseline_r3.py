#!/usr/bin/env python3
"""Produce the formal current-Meetily Whisper baseline from the frozen product EXE.

The child writes only a raw producer result.  This parent process owns process
timing, exit status, pipes, network observations, one-attempt consumption, and
the mechanical conversion into the scorer's formal baseline/run-record files.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import score_s8_m00_r3 as scorer
import supervise_windows_cuda_gate_r3 as gate


RAW_SCHEMA_VERSION = 1
RAW_ROLE = "FORMAL_CURRENT_MEETILY_WHISPER_RAW_OUTPUT"
FORMAL_SCHEMA_VERSION = 3
FORMAL_ROLE = "FORMAL_CURRENT_WHISPER_FULL_OUTPUT"
RUN_RECORD_SCHEMA_VERSION = 2
RUN_RECORD_ROLE = "FORMAL_CURRENT_WHISPER_RUN_RECORD"
MAX_CAPTURE_BYTES = 16 * 1024 * 1024


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat()


def canonical_path(path: Path, *, must_exist: bool) -> Path:
    resolved = path.resolve(strict=must_exist)
    text = str(resolved)
    if text.startswith("\\\\?\\UNC\\"):
        return Path("\\\\" + text[8:])
    if text.startswith("\\\\?\\"):
        return Path(text[4:])
    return resolved


def path_sha256(path: Path) -> str:
    canonical = os.path.normcase(str(canonical_path(path, must_exist=False)))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
    ).encode("utf-8")


def canonical_json_sha256(value: Any) -> str:
    return hashlib.sha256(canonical_json_bytes(value)).hexdigest()


def write_new_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())


def replace_json(path: Path, value: dict[str, Any]) -> None:
    partial = path.with_name(path.name + ".partial")
    encoded = json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    with partial.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(partial, path)


def is_inside(path: Path, root: Path) -> bool:
    try:
        canonical_path(path, must_exist=False).relative_to(
            canonical_path(root, must_exist=False)
        )
        return True
    except ValueError:
        return False


def require_inputs(args: argparse.Namespace) -> dict[str, Any]:
    if sys.platform != "win32":
        raise scorer.ScoringError("正式 Whisper supervisor 只允许在 Windows 运行")
    if not re.fullmatch(r"[0-9a-f]{32}", args.run_id):
        raise scorer.ScoringError("run_id 必须是 32 位小写十六进制")
    if args.language != "zh":
        raise scorer.ScoringError("正式中文基线只允许 language=zh")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.model_name):
        raise scorer.ScoringError("Whisper 模型名含不允许字符")
    if not math.isfinite(args.timeout_seconds) or not 60 <= args.timeout_seconds <= 21600:
        raise scorer.ScoringError("timeout_seconds 必须在 60 到 21600 秒之间")

    workspace = canonical_path(args.workspace_root, must_exist=True)
    run_dir = canonical_path(args.run_dir, must_exist=False)
    ledger_dir = canonical_path(args.run_ledger_dir, must_exist=True)
    if not ledger_dir.is_dir() or is_inside(ledger_dir, workspace):
        raise scorer.ScoringError("一次性运行台账必须是工作区外的已存在目录")
    if args.run_dir.exists() or not args.run_dir.is_absolute():
        raise scorer.ScoringError("run-dir 必须是尚不存在的绝对路径")
    if is_inside(run_dir, ledger_dir) or is_inside(ledger_dir, run_dir):
        raise scorer.ScoringError("run-dir 与一次性运行台账不得互相包含")

    stable_exe = canonical_path(args.stable_exe, must_exist=True)
    audio = canonical_path(args.full_audio, must_exist=True)
    window_audio = canonical_path(args.window_audio, must_exist=True)
    models_dir = canonical_path(args.models_dir, must_exist=True)
    model_file = canonical_path(args.model_file, must_exist=True)
    powershell = canonical_path(args.powershell, must_exist=True)
    locked_python = canonical_path(args.locked_python, must_exist=True)
    if not stable_exe.is_file() or stable_exe.suffix.casefold() != ".exe":
        raise scorer.ScoringError("stable-exe 不是可执行文件")
    if not audio.is_file() or not window_audio.is_file():
        raise scorer.ScoringError("正式完整音频或评分窗口音频不存在")
    if not models_dir.is_dir() or not model_file.is_file():
        raise scorer.ScoringError("Whisper 模型目录或模型文件不存在")
    expected_model = canonical_path(
        models_dir / f"ggml-{args.model_name}.bin", must_exist=True
    )
    if expected_model != model_file:
        raise scorer.ScoringError("model-name、models-dir 与 model-file 不指向同一文件")
    if not powershell.is_file() or not locked_python.is_file():
        raise scorer.ScoringError("冻结 PowerShell 或 Python 路径不存在")

    input_paths = {
        "stable_exe": stable_exe,
        "full_audio": audio,
        "window_audio": window_audio,
        "model_file": model_file,
        "supervisor": canonical_path(Path(__file__), must_exist=True),
        "shared_gate_supervisor": canonical_path(Path(gate.__file__), must_exist=True),
        "scorer": canonical_path(Path(scorer.__file__), must_exist=True),
        "locked_python": locked_python,
        "powershell": powershell,
    }
    identities = [os.path.normcase(str(item)) for item in input_paths.values()]
    if len(identities) != len(set(identities)):
        raise scorer.ScoringError("正式输入文件发生路径别名冲突")
    return {
        "workspace": workspace,
        "run_dir": run_dir,
        "ledger_dir": ledger_dir,
        "models_dir": models_dir,
        "paths": input_paths,
        "hashes": {name: scorer.sha256_file(path) for name, path in input_paths.items()},
    }


def consume_attempt(args: argparse.Namespace, frozen: dict[str, Any]) -> Path:
    marker = frozen["ledger_dir"] / f"whisper-{args.run_id}.consumed.json"
    payload = {
        "schema_version": 1,
        "role": "FORMAL_WHISPER_SINGLE_USE_ATTEMPT",
        "run_id": args.run_id,
        "consumed_at": now_iso(),
        "run_dir_path_sha256": path_sha256(frozen["run_dir"]),
        "workspace_root_path_sha256": path_sha256(frozen["workspace"]),
        "ledger_dir_path_sha256": path_sha256(frozen["ledger_dir"]),
        "input_hashes": frozen["hashes"],
        "policy": "RUN_ID_AND_RUN_DIR_BOUND_ONE_ATTEMPT_ONLY",
    }
    try:
        write_new_json(marker, payload)
    except FileExistsError as exc:
        raise scorer.ScoringError("该 Whisper run_id 已消费，禁止重跑或覆盖") from exc
    return marker


def require_raw_output(
    raw: dict[str, Any],
    *,
    args: argparse.Namespace,
    frozen: dict[str, Any],
    expected_command_sha256: str,
) -> None:
    exact = {
        "schema_version",
        "role",
        "run_id",
        "started_at",
        "finished_at",
        "elapsed_seconds",
        "command_sha256",
        "input_audio_sha256",
        "input_audio_duration_seconds",
        "stable_exe_sha256",
        "model_name",
        "model_file_sha256",
        "backend",
        "language",
        "decode_parameters",
        "speech_segment_count_before_split",
        "speech_segment_count_after_split",
        "segments",
    }
    if set(raw) != exact:
        raise scorer.ScoringError("Whisper 产品原始输出字段集合不严格")
    if (
        raw.get("schema_version") != RAW_SCHEMA_VERSION
        or raw.get("role") != RAW_ROLE
        or raw.get("run_id") != args.run_id
        or raw.get("command_sha256") != expected_command_sha256
        or str(raw.get("input_audio_sha256", "")).casefold()
        != frozen["hashes"]["full_audio"].casefold()
        or str(raw.get("stable_exe_sha256", "")).casefold()
        != frozen["hashes"]["stable_exe"].casefold()
        or raw.get("model_name") != args.model_name
        or str(raw.get("model_file_sha256", "")).casefold()
        != frozen["hashes"]["model_file"].casefold()
        or raw.get("language") != "zh"
        or raw.get("backend") not in {"Cpu", "Cuda", "Vulkan", "HipBlas", "Metal"}
        or not isinstance(raw.get("decode_parameters"), dict)
        or not isinstance(raw.get("segments"), list)
        or not raw["segments"]
    ):
        raise scorer.ScoringError("Whisper 产品原始输出与父进程冻结输入不一致")
    duration = float(raw.get("input_audio_duration_seconds", math.nan))
    elapsed = float(raw.get("elapsed_seconds", math.nan))
    if (
        not math.isfinite(duration)
        or duration <= 0
        or not math.isfinite(elapsed)
        or elapsed < 0
        or not math.isclose(duration, scorer.wav_duration_seconds(frozen["paths"]["full_audio"]), abs_tol=0.001)
    ):
        raise scorer.ScoringError("Whisper 产品原始输出的音频时长或运行时长无效")
    scorer.parse_iso_datetime(raw.get("started_at"), "raw_whisper.started_at")
    scorer.parse_iso_datetime(raw.get("finished_at"), "raw_whisper.finished_at")

    previous_end = 0.0
    for index, segment in enumerate(raw["segments"]):
        if not isinstance(segment, dict) or set(segment) != {
            "start",
            "end",
            "speaker",
            "text",
            "confidence",
        }:
            raise scorer.ScoringError(f"Whisper 原始 segment[{index}] 字段不严格")
        start = float(segment.get("start", math.nan))
        end = float(segment.get("end", math.nan))
        confidence = float(segment.get("confidence", math.nan))
        if (
            not math.isfinite(start)
            or not math.isfinite(end)
            or not math.isfinite(confidence)
            or start < previous_end - 0.001
            or end <= start
            or end > duration + 0.001
            or not isinstance(segment.get("text"), str)
            or not segment["text"].strip()
            or segment.get("speaker") != ""
        ):
            raise scorer.ScoringError(f"Whisper 原始 segment[{index}] 内容无效")
        previous_end = end


def log_leak_audit(
    stdout_bytes: bytes,
    stderr_bytes: bytes,
    raw: dict[str, Any],
    sensitive_paths: list[Path],
) -> dict[str, Any]:
    combined = (stdout_bytes + b"\n" + stderr_bytes).decode("utf-8", errors="replace")
    normalized_log = scorer.normalize_text(combined)
    matches: list[dict[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for segment in raw.get("segments", []):
        normalized = scorer.normalize_text(str(segment.get("text", "")))
        for index in range(max(0, len(normalized) - 7)):
            probe = normalized[index : index + 8]
            if probe and probe in normalized_log and ("transcript_ngram", probe) not in seen:
                seen.add(("transcript_ngram", probe))
                matches.append({"kind": "transcript_ngram", "sha256": hashlib.sha256(probe.encode("utf-8")).hexdigest()})
    log_casefold = combined.casefold()
    for path in sensitive_paths:
        probe = str(path).casefold()
        if probe and probe in log_casefold and ("sensitive_path", probe) not in seen:
            seen.add(("sensitive_path", probe))
            matches.append({"kind": "sensitive_path", "sha256": hashlib.sha256(probe.encode("utf-8")).hexdigest()})
    return {
        "method": "PARENT_PIPE_CAPTURE_TRANSCRIPT_8GRAM_AND_PATH_SCAN",
        "stdout_bytes": len(stdout_bytes),
        "stderr_bytes": len(stderr_bytes),
        "stdout_sha256": hashlib.sha256(stdout_bytes).hexdigest(),
        "stderr_sha256": hashlib.sha256(stderr_bytes).hexdigest(),
        "raw_logs_written_to_disk": False,
        "matched_probe_count": len(matches),
        "matches": matches,
        "clean": not matches,
    }


def _execute_impl(args: argparse.Namespace) -> int:
    frozen = require_inputs(args)
    marker = consume_attempt(args, frozen)
    run_dir: Path = frozen["run_dir"]
    run_dir.mkdir(parents=True, exist_ok=False)
    raw_output = run_dir / "01-whisper-product-raw.json"
    formal_output = run_dir / "02-formal-current-whisper-full.json"
    run_record_path = run_dir / "03-formal-current-whisper-run-record.json"
    status_path = run_dir / "99-status.json"
    started_at = now_iso()
    started_monotonic = time.monotonic()
    replace_json(
        status_path,
        {
            "schema_version": 1,
            "role": "FORMAL_WHISPER_SUPERVISOR_STATUS",
            "run_id": args.run_id,
            "status": "RUNNING",
            "go_allowed": False,
            "started_at": started_at,
        },
    )

    stable_exe = frozen["paths"]["stable_exe"]
    command = [
        str(stable_exe),
        "--formal-whisper-baseline",
        "--run-id",
        args.run_id,
        "--audio",
        str(frozen["paths"]["full_audio"]),
        "--models-dir",
        str(frozen["models_dir"]),
        "--model-name",
        args.model_name,
        "--language",
        args.language,
        "--output",
        str(raw_output),
    ]
    command_sha256 = canonical_json_sha256(command)
    process: subprocess.Popen[bytes] | None = None
    job: gate.WindowsKillOnCloseJob | None = None
    stdout_capture: gate.ParentOwnedPipeCapture | None = None
    stderr_capture: gate.ParentOwnedPipeCapture | None = None
    firewall: dict[str, Any] | None = None
    firewall_removed: dict[str, Any] | None = None
    job_close: dict[str, Any] | None = None
    timed_out = False
    runtime_checks: list[dict[str, Any]] = []
    observations: list[dict[str, Any]] = []
    failure: BaseException | None = None
    try:
        firewall = gate.firewall_add(
            args.run_id,
            frozen["paths"]["powershell"],
            frozen["paths"]["locked_python"],
            protected_programs=[
                frozen["paths"]["stable_exe"],
                frozen["paths"]["locked_python"],
            ],
        )
        child_env = {
            key: os.environ[key]
            for key in ("SYSTEMROOT", "WINDIR", "TEMP", "TMP")
            if key in os.environ
        }
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=child_env,
            creationflags=gate.CREATE_SUSPENDED,
        )
        job = gate.WindowsKillOnCloseJob()
        job.assign(process)
        if process.stdout is None or process.stderr is None:
            raise scorer.ScoringError("父进程没有取得 Whisper 匿名管道")
        stdout_capture = gate.ParentOwnedPipeCapture(process.stdout, "whisper-stdout", MAX_CAPTURE_BYTES)
        stderr_capture = gate.ParentOwnedPipeCapture(process.stderr, "whisper-stderr", MAX_CAPTURE_BYTES)
        stdout_capture.start()
        stderr_capture.start()
        gate.resume_suspended_process(process)

        next_firewall_check = time.monotonic()
        while process.poll() is None:
            current = time.monotonic()
            if current - started_monotonic > args.timeout_seconds:
                timed_out = True
                gate.terminate_tree(process.pid)
                break
            observation = gate.observe_process_tree(process.pid)
            observation["sampled_at_monotonic_seconds"] = current
            observations.append(observation)
            if current >= next_firewall_check:
                snapshot = gate._firewall_snapshot(
                    frozen["paths"]["powershell"], firewall["rule_names"]
                )
                valid = (
                    snapshot.get("records") == firewall["installed"].get("records")
                    and gate.firewall_platform_is_effective(snapshot)
                )
                runtime_checks.append(
                    {
                        "sampled_at_monotonic_seconds": current,
                        "valid": valid,
                        "snapshot": snapshot,
                    }
                )
                if not valid:
                    gate.terminate_tree(process.pid)
                    raise scorer.ScoringError("Whisper 运行期间防火墙状态发生变化")
                next_firewall_check = current + 5.0
            time.sleep(0.25)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            gate.terminate_tree(process.pid)
            process.wait(timeout=15)
    except BaseException as exc:
        failure = exc
        if process is not None and process.poll() is None:
            gate.terminate_tree(process.pid)
            try:
                process.wait(timeout=15)
            except BaseException:
                pass
    finally:
        if job is not None:
            try:
                job_close = job.close()
            except BaseException as exc:
                failure = failure or exc
        if stdout_capture is not None:
            stdout_capture.finish()
        if stderr_capture is not None:
            stderr_capture.finish()
        if firewall is not None:
            try:
                firewall_removed = gate.firewall_remove(
                    firewall, frozen["paths"]["powershell"]
                )
            except BaseException as exc:
                failure = failure or exc
                firewall_removed = {
                    "removed": False,
                    "error_type": type(exc).__name__,
                    "after": {"records": ["REMOVAL_CHECK_FAILED"]},
                }

    finished_at = now_iso()
    exit_code = process.returncode if process is not None else None
    stdout_bytes = stdout_capture.captured_bytes() if stdout_capture else b""
    stderr_bytes = stderr_capture.captured_bytes() if stderr_capture else b""
    capture_complete = bool(
        stdout_capture
        and stderr_capture
        and stdout_capture.complete
        and stderr_capture.complete
    )
    remote_connections = sorted(
        {
            connection
            for observation in observations
            for connection in observation.get("remote_connections", [])
        }
    )
    listeners = sorted(
        {
            listener
            for observation in observations
            for listener in observation.get("listening_sockets", [])
        }
    )
    observation_errors = sorted(
        {
            error
            for observation in observations
            for error in observation.get("errors", [])
        }
    )
    hard_failure_reasons: list[str] = []
    if failure is not None:
        hard_failure_reasons.append("supervisor_exception")
    if timed_out:
        hard_failure_reasons.append("process_timeout")
    if exit_code != 0:
        hard_failure_reasons.append("process_exit_code")
    if not raw_output.is_file():
        hard_failure_reasons.append("raw_output_missing")
    if not capture_complete:
        hard_failure_reasons.append("parent_pipe_capture_incomplete")
    if not firewall_removed:
        hard_failure_reasons.append("firewall_removal_evidence_missing")
    elif firewall_removed.get("removed") is not True:
        hard_failure_reasons.append("firewall_rules_not_verified_removed")
    if not runtime_checks:
        hard_failure_reasons.append("firewall_runtime_checks_missing")
    elif any(item.get("valid") is not True for item in runtime_checks):
        hard_failure_reasons.append("firewall_runtime_check_invalid")
    if remote_connections:
        hard_failure_reasons.append("remote_connection_observed")
    if listeners:
        hard_failure_reasons.append("listening_socket_observed")
    if observation_errors:
        hard_failure_reasons.append("network_observation_error")
    if not job_close:
        hard_failure_reasons.append("job_close_evidence_missing")
    elif job_close.get("close_handle_succeeded") is not True:
        hard_failure_reasons.append("job_close_failed")
    hard_failure = bool(hard_failure_reasons)
    if hard_failure:
        replace_json(
            status_path,
            {
                "schema_version": 1,
                "role": "FORMAL_WHISPER_SUPERVISOR_STATUS",
                "run_id": args.run_id,
                "status": "FAILED_CLOSED",
                "go_allowed": False,
                "started_at": started_at,
                "finished_at": finished_at,
                "failure_type": type(failure).__name__ if failure else None,
                "hard_failure_reasons": hard_failure_reasons,
                "timed_out": timed_out,
                "process_exit_code": exit_code,
                "raw_output_present": raw_output.is_file(),
                "capture_complete": capture_complete,
                "stdout_capture": {
                    "total_bytes": stdout_capture.total_bytes if stdout_capture else 0,
                    "overflow": stdout_capture.overflow if stdout_capture else None,
                    "error_type": stdout_capture.error_type if stdout_capture else None,
                },
                "stderr_capture": {
                    "total_bytes": stderr_capture.total_bytes if stderr_capture else 0,
                    "overflow": stderr_capture.overflow if stderr_capture else None,
                    "error_type": stderr_capture.error_type if stderr_capture else None,
                },
                "firewall_removal": {
                    "evidence_present": firewall_removed is not None,
                    "removed": (
                        firewall_removed.get("removed")
                        if firewall_removed
                        else None
                    ),
                    "remove_exit_code": (
                        firewall_removed.get("remove_exit_code")
                        if firewall_removed
                        else None
                    ),
                    "attempt_count": (
                        firewall_removed.get("attempt_count")
                        if firewall_removed
                        else None
                    ),
                    "remaining_rule_count": (
                        len(firewall_removed.get("after", {}).get("records", []))
                        if firewall_removed
                        else None
                    ),
                    "error_type": (
                        firewall_removed.get("error_type")
                        if firewall_removed
                        else None
                    ),
                },
                "firewall_runtime_check_count": len(runtime_checks),
                "invalid_firewall_runtime_check_count": sum(
                    1 for item in runtime_checks if item.get("valid") is not True
                ),
                "remote_connection_count": len(remote_connections),
                "listening_socket_count": len(listeners),
                "network_observation_error_count": len(observation_errors),
                "job_close": {
                    "evidence_present": job_close is not None,
                    "close_handle_succeeded": (
                        job_close.get("close_handle_succeeded") if job_close else None
                    ),
                },
                "run_attempt_marker_sha256": scorer.sha256_file(marker),
            },
        )
        return 2

    raw = scorer.load_json(raw_output)
    require_raw_output(
        raw,
        args=args,
        frozen=frozen,
        expected_command_sha256=command_sha256,
    )
    log_audit = log_leak_audit(
        stdout_bytes,
        stderr_bytes,
        raw,
        list(frozen["paths"].values()) + [frozen["models_dir"], run_dir],
    )
    if not log_audit["clean"]:
        raise scorer.ScoringError("Whisper 子进程日志泄露了转写文本或敏感路径")

    raw_sha256 = scorer.sha256_file(raw_output)
    formal = {
        "schema_version": FORMAL_SCHEMA_VERSION,
        "role": FORMAL_ROLE,
        "run_id": args.run_id,
        "source_full_audio_sha256": frozen["hashes"]["full_audio"],
        "clip_audio_sha256": frozen["hashes"]["full_audio"],
        "source_transcripts_sha256": raw_sha256,
        "source_producer_role": RAW_ROLE,
        "source_producer_schema_version": RAW_SCHEMA_VERSION,
        "source_producer_command_sha256": command_sha256,
        "model_name": raw["model_name"],
        "model_file_sha256": raw["model_file_sha256"],
        "backend": raw["backend"],
        "stable_exe_sha256": raw["stable_exe_sha256"],
        "language": raw["language"],
        "audio_duration_seconds": raw["input_audio_duration_seconds"],
        "decode_parameters": raw["decode_parameters"],
        "transformation": {
            "method": "PARENT_EXACT_SEGMENT_COPY_FROM_PRODUCT_RAW_OUTPUT",
            "text_changed": False,
            "timestamps_changed": False,
            "speaker_labels_changed": False,
        },
        "segments": raw["segments"],
    }
    write_new_json(formal_output, formal)
    formal_sha256 = scorer.sha256_file(formal_output)
    run_record = {
        "schema_version": RUN_RECORD_SCHEMA_VERSION,
        "role": RUN_RECORD_ROLE,
        "evidence_class": "SUPERVISED_CURRENT_WHISPER_RUN",
        "runner_source": "MEETILY_WHISPER_SUPERVISOR",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": finished_at,
        "execution_status": "COMPLETED",
        "process_exit_code": exit_code,
        "source_full_audio_sha256": frozen["hashes"]["full_audio"],
        "window_audio_sha256": frozen["hashes"]["window_audio"],
        "stable_exe_sha256": frozen["hashes"]["stable_exe"],
        "model_file_sha256": frozen["hashes"]["model_file"],
        "backend": raw["backend"],
        "language": raw["language"],
        "decode_parameters": raw["decode_parameters"],
        "command_sha256": command_sha256,
        "raw_product_output_sha256": raw_sha256,
        "output_full_sha256": formal_sha256,
        "supervisor_source_sha256": frozen["hashes"]["supervisor"],
        "shared_gate_supervisor_sha256": frozen["hashes"]["shared_gate_supervisor"],
        "scorer_sha256": frozen["hashes"]["scorer"],
        "run_consumption": {
            "policy": "RUN_ID_AND_RUN_DIR_BOUND_ONE_ATTEMPT_ONLY",
            "marker_sha256": scorer.sha256_file(marker),
            "ledger_dir_path_sha256": path_sha256(frozen["ledger_dir"]),
            "run_dir_path_sha256": path_sha256(run_dir),
        },
        "supervision": {
            "process_started_suspended": True,
            "job_assigned_before_resume": True,
            "kill_on_close_configured": True,
            "job_close": job_close,
            "process_exit_code_captured_by_parent": True,
            "wall_clock_captured_by_parent": True,
            "wall_clock_seconds": time.monotonic() - started_monotonic,
            "timeout_seconds": args.timeout_seconds,
            "timed_out": timed_out,
            "parent_owned_pipe_capture": True,
            "capture_complete": capture_complete,
            "log_audit": log_audit,
            "firewall": {
                "installed": firewall,
                "runtime_checks": runtime_checks,
                "removed": firewall_removed,
            },
            "observed_remote_connections": remote_connections,
            "observed_listening_sockets": listeners,
            "observation_errors": observation_errors,
            "observation_count": len(observations),
            "raw_logs_written_to_disk": False,
            "process_tree_cleanup_complete": process.poll() is not None,
        },
    }
    write_new_json(run_record_path, run_record)
    replace_json(
        status_path,
        {
            "schema_version": 1,
            "role": "FORMAL_WHISPER_SUPERVISOR_STATUS",
            "run_id": args.run_id,
            "status": "COMPLETED_NOT_YET_FROZEN_IN_SCORING_BUNDLE",
            "go_allowed": False,
            "started_at": started_at,
            "finished_at": finished_at,
            "formal_output_sha256": formal_sha256,
            "run_record_sha256": scorer.sha256_file(run_record_path),
            "run_attempt_marker_sha256": scorer.sha256_file(marker),
        },
    )
    return 0


def execute(args: argparse.Namespace) -> int:
    try:
        return _execute_impl(args)
    except BaseException as exc:
        try:
            run_dir = canonical_path(args.run_dir, must_exist=False)
            status_path = run_dir / "99-status.json"
            if run_dir.is_dir() and status_path.is_file():
                replace_json(
                    status_path,
                    {
                        "schema_version": 1,
                        "role": "FORMAL_WHISPER_SUPERVISOR_STATUS",
                        "run_id": args.run_id,
                        "status": "FAILED_CLOSED",
                        "go_allowed": False,
                        "finished_at": now_iso(),
                        "failure_type": type(exc).__name__,
                        "run_id_consumed": True,
                    },
                )
        except BaseException:
            pass
        raise


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--run-dir", type=Path, required=True)
    parser.add_argument("--run-ledger-dir", type=Path, required=True)
    parser.add_argument("--workspace-root", type=Path, required=True)
    parser.add_argument("--stable-exe", type=Path, required=True)
    parser.add_argument("--full-audio", type=Path, required=True)
    parser.add_argument("--window-audio", type=Path, required=True)
    parser.add_argument("--models-dir", type=Path, required=True)
    parser.add_argument("--model-name", required=True)
    parser.add_argument("--model-file", type=Path, required=True)
    parser.add_argument("--language", default="zh")
    parser.add_argument("--powershell", type=Path, required=True)
    parser.add_argument("--locked-python", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=float, default=7200.0)
    args = parser.parse_args()
    try:
        return execute(args)
    except (scorer.ScoringError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"Whisper formal supervisor failed closed: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
