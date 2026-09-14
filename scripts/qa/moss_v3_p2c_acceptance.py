#!/usr/bin/env python3
"""Run P2-C process-level gates without retaining transcript or generated text.

This harness is intentionally Windows/local-only.  Private and public evidence both
contain hashes, counters, timings, and fixed error codes only.  The helper stdout is
parsed in memory and all text/segment payloads are discarded immediately.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import math
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from pathlib import Path
from typing import Any


PROTOCOL_VERSION = 1
REQUEST_ID = "798d8c63-5ff1-40e3-9db8-0f706aeb930a"
MODEL_BYTES = 986_899_616
MODEL_SHA256 = "64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039"
PCM_BYTES = 770_388
PCM_SAMPLES = 192_597
PCM_SHA256 = "2A1997D757808E64BED66B774DB0D1F4219ECCB13E77C2D7585E712458509A02"
ABSOLUTE_PATH = re.compile(r"(?i)(?:[a-z]:\\|[a-z]:/|\\\\)")
TEXT_KEYS = {"text", "raw_text", "clean_text", "prompt", "transcript"}
PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES = 0x00020009
EXTENDED_STARTUPINFO_PRESENT = 0x00080000
CREATE_NO_WINDOW = 0x08000000
WAIT_OBJECT_0 = 0
COMMON_PUBLIC_FIELDS = {
    "schema_version", "stage", "status", "integrity_status", "acceptance_status",
    "absolute_paths_in_evidence", "transcript_in_evidence", "binding_sha256",
    "source_commit", "source_tree_clean", "cargo_lock_sha256", "test_executable_sha256",
    "helper_binary_sha256", "runtime_manifest_sha256", "model_sha256", "fixture_sha256",
}
PUBLIC_STAGE_FIELDS = {
    "MOSS_V3_P2C_PROCESS_FAULTS": {"case_count", "cases"},
    "MOSS_V3_P2C_QWEN_RELEASE": {
        "model_bytes", "health_probe", "model_generate_health", "discarded_generated_text_bytes",
        "helper_exit_code", "preceding_moss_evidence_sha256", "preceding_moss_run_count",
        "process_counts_before", "process_counts_after", "residual_process_count",
        "available_memory_before_bytes", "available_memory_after_bytes", "available_memory_delta_bytes",
        "gpu_available_memory_bytes", "stderr_bytes", "stderr_sha256", "wall_elapsed_ms",
        "process_snapshot_before", "process_snapshot_after",
    },
    "MOSS_V3_P2C_ZERO_NETWORK_REAL_SHORT": {
        "isolation", "appcontainer_child_exit_code", "token_is_appcontainer", "capability_count",
        "network_before_blocked", "network_after_blocked", "terminal_type", "error_code", "error_phase",
        "native_status", "terminal_count", "helper_exit_code", "discarded_text_payload_bytes",
        "wall_elapsed_ms", "residual_process_count", "result_present", "failure_code",
    },
    "MOSS_V3_P2C_CHUNKED_PERFORMANCE": {
        "helper_binary_sha256", "identical_300_second_fixture_sha256",
        "identical_300_second_run_count", "identical_output_hashes", "maximum_product_request_seconds",
        "new_windows_error_1000_1001_count", "residual_process_count", "single_native_chunk_limit_seconds",
        "four_hundred_eighty", "six_hundred", "source_evidence_sha256",
        "source_sidecar_sha256", "unique_terminal_per_run",
        "failure_code",
    },
    "MOSS_V3_P2D_BINDING": {"commit", "tree_clean", "artifacts"},
    "MOSS_V3_P2D_RUST_TRUNCATION_GATE": {
        "test_name", "test_executable_sha256", "exit_code", "log_sha256", "log_bytes",
    },
    "MOSS_V3_P2D_PARENT_KILL": {
        "test_name", "test_executable_sha256", "runner_process_id", "helper_process_id",
        "descendant_process_ids", "residual_process_count", "exit_observed", "log_sha256", "log_bytes",
    },
    "MOSS_V3_P2D_DUPLICATE_REQUEST": {
        "first_helper_processes_started", "duplicate_error", "second_helper_started",
        "residual_process_count", "cache_entry_after",
    },
    "MOSS_V3_P2D_SECOND_CHUNK_FAILURE": {
        "failed_chunk_index", "first_attempt_helper_processes", "partial_success_returned",
        "failure_code", "retry_helper_processes", "retry_terminal", "residual_process_count",
        "cache_entry_after",
    },
    "MOSS_V3_P2D_CODE_VALIDATION": {"checks", "listed_test_count"},
    "MOSS_V3_P2C_FINAL_VALIDATION": {
        "source_commit", "code_validation", "real_gates", "traceability", "blocked_gates",
    },
    "MOSS_V3_P2C_PUBLIC_MANIFEST": {"files"},
    "MOSS_V3_P2C_PRIVATE_MANIFEST": {"files"},
    "MOSS_V3_P2C_CHUNK_CONTROL": {"cancel", "timeout"},
    "MOSS_V3_P2C_REAL_CONTROL": {"cancel", "timeout"},
    "MOSS_V3_P2C_TEN_SEQUENTIAL_MANAGER": {
        "available_memory_after_bytes", "available_memory_before_bytes", "available_memory_delta_bytes",
        "cross_run_hash_isolation", "distinct_helper_process_count", "git_commit", "maximum_peak_job_memory_bytes",
        "minimum_peak_job_memory_bytes", "run_count", "runs", "unique_context_hashes", "unique_request_hashes",
        "unique_terminal_per_run",
    },
    "MOSS_V3_P2C_300S_SINGLE_CHILD": {
        "duration_seconds", "helper_binary_sha256", "supervisor_wall_elapsed_ms", "supervisor_wall_rtf",
        "last_timestamp_ms", "segment_count", "raw_text_sha256", "clean_text_sha256", "helper_runs",
        "residual_process_count", "helper_process_id",
    },
    "MOSS_V3_P2C_480S_PERFORMANCE": {
        "duration_seconds", "max_wall_rtf", "helper_binary_sha256", "supervisor_wall_elapsed_ms",
        "supervisor_wall_rtf", "native_run_elapsed_ms", "native_rtf", "last_timestamp_ms", "segment_count",
        "raw_text_sha256", "clean_text_sha256", "helper_runs", "residual_process_count",
        "helper_process_id",
    },
    "MOSS_V3_P2C_600S_PERFORMANCE": {
        "duration_seconds", "max_wall_rtf", "helper_binary_sha256", "wall_elapsed_ms", "wall_rtf",
        "supervisor_wall_elapsed_ms", "supervisor_wall_rtf", "native_run_elapsed_ms", "native_rtf",
        "segment_count", "raw_text_sha256", "clean_text_sha256", "helper_peak_job_memory_bytes",
        "residual_process_count", "helper_runs",
        "helper_process_id",
    },
}

FINAL_GATE_NAMES = {
    "code_validation", "ten_sequential", "chunk_control", "real_control",
    "performance", "process_faults", "truncation", "offline", "parent_kill",
    "duplicate_request", "second_chunk_failure", "qwen",
}
BINDING_ARTIFACT_NAMES = {
    "cargo_lock", "test_executable", "moss_helper", "qwen_helper",
    "runtime_manifest", "q8_model", "qwen_model", "short_pcm",
    "three_hundred_pcm", "four_eighty_pcm", "six_hundred_pcm",
}
HELPER_RUN_FIELDS = {
    "chunkIndex", "cleanTextSha256", "inputDurationMs", "inputOffsetMs",
    "lastTimestampMs", "nativeRtf", "peakJobMemoryBytes", "processId",
    "rawTextSha256", "residualProcessCount", "terminalCount", "totalProcesses",
}
PROCESS_SNAPSHOT_FIELDS = {
    "pid", "parent_pid", "image_name", "image_path_sha256", "matches_expected_image",
}
FAULT_CASE_FIELDS = {
    "case", "discarded_text_payload_bytes", "error_code", "exit_code",
    "expected_code", "message_count", "process_id", "residual_process_count",
    "stderr_bytes", "stderr_sha256", "terminal_count", "terminal_type",
    "wall_elapsed_ms", "status", "test_executable_sha256", "test_kind",
    "test_log_sha256",
}
TEN_RUN_FIELDS = {
    "cache_entry_after", "clean_text_sha256", "context_sha256",
    "helper_peak_job_memory_bytes", "helper_process_id", "helper_total_processes",
    "manager_active_after", "native_rtf", "raw_text_sha256", "request_id_sha256",
    "residual_process_count", "run", "segment_count", "supervisor_wall_elapsed_ms",
    "supervisor_wall_rtf", "terminal_count", "test_wall_elapsed_ms",
    "wall_elapsed_ms", "wall_rtf",
}
PUBLIC_NESTED_FIELDS = {
    "MOSS_V3_P2D_BINDING": {
        "artifacts": BINDING_ARTIFACT_NAMES,
    },
    "MOSS_V3_P2C_CHUNK_CONTROL": {
        "cancel": {"helper_processes_started", "later_child_started", "residual_process_count", "terminal"},
        "timeout": {"helper_processes_started", "later_child_started", "residual_process_count", "terminal"},
    },
    "MOSS_V3_P2C_REAL_CONTROL": {
        "cancel": {"residual_process_count", "terminal"},
        "timeout": {"residual_process_count", "terminal"},
    },
    "MOSS_V3_P2D_CODE_VALIDATION": {
        "checks[]": {"exit_code", "gate", "log_bytes", "log_sha256"},
    },
    "MOSS_V3_P2C_PROCESS_FAULTS": {
        "cases[]": FAULT_CASE_FIELDS,
    },
    "MOSS_V3_P2C_QWEN_RELEASE": {
        "process_counts_before": {"moss-helper.exe", "llama-helper.exe"},
        "process_counts_after": {"moss-helper.exe", "llama-helper.exe"},
        "process_snapshot_before[]": PROCESS_SNAPSHOT_FIELDS,
        "process_snapshot_after[]": PROCESS_SNAPSHOT_FIELDS,
    },
    "MOSS_V3_P2C_CHUNKED_PERFORMANCE": {
        "four_hundred_eighty": {"child_durations_ms", "last_timestamp_ms", "supervisor_wall_rtf"},
        "six_hundred": {"child_durations_ms", "last_timestamp_ms", "supervisor_wall_rtf"},
        "source_evidence_sha256": {"single_first", "single_second", "single_third", "four_eighty", "six_hundred"},
        "source_sidecar_sha256": {"single_first", "single_second", "single_third", "four_eighty", "six_hundred"},
    },
    "MOSS_V3_P2C_TEN_SEQUENTIAL_MANAGER": {
        "runs[]": TEN_RUN_FIELDS,
    },
    "MOSS_V3_P2C_300S_SINGLE_CHILD": {
        "helper_runs[]": HELPER_RUN_FIELDS,
    },
    "MOSS_V3_P2C_480S_PERFORMANCE": {
        "helper_runs[]": HELPER_RUN_FIELDS,
    },
    "MOSS_V3_P2C_600S_PERFORMANCE": {
        "helper_runs[]": HELPER_RUN_FIELDS,
    },
    "MOSS_V3_P2C_FINAL_VALIDATION": {
        "real_gates": FINAL_GATE_NAMES - {"code_validation"},
        "traceability": FINAL_GATE_NAMES,
    },
    "MOSS_V3_P2C_PUBLIC_MANIFEST": {
        "files[]": {"file", "bytes", "sha256"},
    },
    "MOSS_V3_P2C_PRIVATE_MANIFEST": {
        "files[]": {"file", "bytes", "sha256"},
    },
}

FIXED_PUBLIC_VALUES = {
    "PASS", "FAIL", "BLOCKED", "completed", "cancelled", "failed", "timeout",
    "shutdown", "probe_result",
    "pong", "response_without_error", "bound_rust_unit_test",
    "Windows AppContainer with zero capabilities", "MOSS_DUPLICATE_REQUEST",
    "PROTOCOL_INVALID_JSON", "AUDIO_HASH_MISMATCH", "AUDIO_NON_FINITE",
    "MODEL_CONTRACT_MISMATCH", "DEVICE_NOT_FOUND", "NATIVE_OUTPUT_TRUNCATED",
    "invalid_json", "audio_hash_mismatch", "audio_non_finite",
    "model_contract_mismatch", "vulkan_device_unavailable",
    "native_output_truncated_rust_gate", "cargo_fmt", "git_diff_check",
    "moss_helper_clippy", "moss_helper_tests", "qa_harness_tests", "tauri_test_list",
    "moss-helper.exe", "llama-helper.exe", "Intel(R) Arc(TM) Graphics",
    "native::imp::binding_tests::native_output_truncated_status_maps_to_stable_terminal_code",
    "moss_helper::manager::tests::p2c_parent_force_victim",
    "APPCONTAINER_CHILD_FAILED", "APPCONTAINER_RESULT_INVALID",
    "PERFORMANCE_SOURCE_INVALID", "P2D_ACCEPTANCE_SOURCE_INVALID",
    "audio_validation", "backend_init", "device_policy", "device_selection",
    "model_binding", "model_contract", "model_validation", "native_run",
    "native_thread", "preflight", "result_validation", "runtime_commit",
    "runtime_contract", "runtime_environment", "runtime_load", "runtime_symbols",
    "runtime_version", "stdout",
}


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest().upper()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def verify_sha_sidecar(path: Path) -> None:
    sidecar = path.with_name(path.name + ".sha256")
    if not sidecar.is_file():
        raise AssertionError("evidence SHA sidecar is missing")
    parts = sidecar.read_text(encoding="ascii").strip().split()
    if len(parts) != 2 or parts[1] != path.name or parts[0].upper() != sha256_file(path):
        raise AssertionError("evidence SHA sidecar does not match")


def native_safe_environment() -> dict[str, str]:
    environment: dict[str, str] = {}
    for name, value in os.environ.items():
        upper = name.upper()
        if (
            upper.startswith("VK_")
            or upper == "VULKAN_SDK"
            or upper.startswith("TRANSCRIBE_")
            or upper.startswith("GGML_")
        ):
            continue
        environment[name] = value
    return environment


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode("utf-8") + b"\n"
    temporary = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    with temporary.open("xb") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    digest = sha256_bytes(encoded)
    sidecar = path.with_name(path.name + ".sha256")
    sidecar_temporary = sidecar.with_name(
        f".{sidecar.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"
    )
    sidecar_encoded = f"{digest}  {path.name}\n".encode("ascii")
    # Use binary output so Windows cannot translate LF into CRLF.  Rust gates
    # already emit LF-only sidecars; byte-identical sidecars are required for
    # final traceability to remain valid after publish-safe copies the JSON.
    with sidecar_temporary.open("xb") as stream:
        stream.write(sidecar_encoded)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(sidecar_temporary, sidecar)


def nested_public_fields(stage: str, path: str) -> set[str] | None:
    configured = PUBLIC_NESTED_FIELDS.get(stage, {}).get(path)
    if configured is not None:
        return configured
    if stage == "MOSS_V3_P2D_BINDING" and re.fullmatch(r"artifacts\.[a-z0-9_]+", path):
        return {"bytes", "sha256"}
    if stage == "MOSS_V3_P2C_FINAL_VALIDATION" and re.fullmatch(
        r"traceability\.[a-z_]+", path
    ):
        return {"json_sha256", "sidecar_sha256"}
    return None


def assert_public_scalar(value: Any, key: str, path: str) -> None:
    lowered = key.lower()
    if lowered in TEXT_KEYS:
        raise AssertionError(f"forbidden public evidence key: {key}")
    if value is None:
        if key not in {"error_code", "error_phase", "native_status", "gpu_available_memory_bytes", "image_path_sha256"}:
            raise AssertionError(f"unexpected null public evidence value: {path}")
        return
    if isinstance(value, bool):
        return
    if isinstance(value, int):
        return
    if isinstance(value, float):
        if not math.isfinite(value):
            raise AssertionError("public evidence contains a non-finite number")
        return
    if not isinstance(value, str):
        raise AssertionError(f"unsupported public evidence value type: {path}")
    if ABSOLUTE_PATH.search(value):
        raise AssertionError("absolute path found in public evidence")
    if key == "stage":
        if value not in PUBLIC_STAGE_FIELDS:
            raise AssertionError("unknown public evidence stage")
        return
    if (
        key in {"sha256", "json_sha256", "sidecar_sha256"}
        or key.lower().endswith("sha256")
        or path.startswith("source_evidence_sha256.")
        or path.startswith("source_sidecar_sha256.")
    ):
        if not re.fullmatch(r"[A-F0-9]{64}", value):
            raise AssertionError(f"invalid public evidence SHA-256: {path}")
        return
    if key in {"commit", "source_commit", "git_commit"}:
        if not re.fullmatch(r"[a-f0-9]{40}", value):
            raise AssertionError(f"invalid public evidence commit: {path}")
        return
    if key == "file":
        if not re.fullmatch(r"[A-Za-z0-9_.-]+\.json", value):
            raise AssertionError("unsafe evidence manifest file name")
        return
    if value not in FIXED_PUBLIC_VALUES:
        raise AssertionError(f"unapproved public evidence string: {path}")


def assert_public_safe(
    value: Any,
    key: str = "",
    root: bool = True,
    *,
    stage: str | None = None,
    path: str = "",
) -> None:
    if root:
        if not isinstance(value, dict) or not isinstance(value.get("stage"), str):
            raise AssertionError("public evidence has no fixed stage")
        stage = value["stage"]
        allowed = COMMON_PUBLIC_FIELDS | PUBLIC_STAGE_FIELDS.get(stage, set())
        if not allowed or set(value) - allowed:
            raise AssertionError("public evidence contains fields outside its DTO")
    assert stage is not None
    if isinstance(value, dict):
        if not root:
            allowed_nested = nested_public_fields(stage, path)
            if allowed_nested is None or set(value) - allowed_nested:
                raise AssertionError(f"public evidence contains an unapproved nested DTO: {path}")
        for child_key, child_value in value.items():
            child_path = str(child_key) if root else f"{path}.{child_key}"
            assert_public_safe(
                child_value,
                str(child_key),
                False,
                stage=stage,
                path=child_path,
            )
    elif isinstance(value, list):
        for child in value:
            assert_public_safe(child, key, False, stage=stage, path=f"{path}[]")
    else:
        assert_public_scalar(value, key, path)


def write_evidence(private_path: Path, public_path: Path, value: dict[str, Any]) -> None:
    safe = dict(value)
    safe["absolute_paths_in_evidence"] = False
    safe["transcript_in_evidence"] = False
    assert_public_safe(safe)
    atomic_json(private_path, safe)
    atomic_json(public_path, safe)


def windows_process_snapshot(expected_images: tuple[Path, ...] = ()) -> list[dict[str, Any]]:
    if os.name != "nt":
        return []

    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", ctypes.c_ulong), ("cntUsage", ctypes.c_ulong),
            ("th32ProcessID", ctypes.c_ulong), ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", ctypes.c_ulong), ("cntThreads", ctypes.c_ulong),
            ("th32ParentProcessID", ctypes.c_ulong), ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", ctypes.c_ulong), ("szExeFile", ctypes.c_wchar * 260),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateToolhelp32Snapshot.argtypes = [ctypes.c_ulong, ctypes.c_ulong]
    kernel32.CreateToolhelp32Snapshot.restype = ctypes.c_void_p
    kernel32.Process32FirstW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32FirstW.restype = ctypes.c_int
    kernel32.Process32NextW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32NextW.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    kernel32.OpenProcess.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
    kernel32.OpenProcess.restype = ctypes.c_void_p
    kernel32.QueryFullProcessImageNameW.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_ulong)]
    kernel32.QueryFullProcessImageNameW.restype = ctypes.c_int
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)
    if snapshot == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    helper_names = {
        "moss-helper.exe", "llama-helper.exe",
        "moss-helper-x86_64-pc-windows-msvc.exe",
        "llama-helper-x86_64-pc-windows-msvc.exe",
    }
    expected = {str(path.resolve(strict=True)).casefold() for path in expected_images}
    entries: list[dict[str, Any]] = []
    entry = PROCESSENTRY32W()
    entry.dwSize = ctypes.sizeof(entry)
    try:
        available = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while available:
            name = entry.szExeFile.lower()
            if name in helper_names:
                image = ""
                process = kernel32.OpenProcess(0x1000, 0, entry.th32ProcessID)
                if process:
                    try:
                        buffer = ctypes.create_unicode_buffer(32768)
                        length = ctypes.c_ulong(len(buffer))
                        if kernel32.QueryFullProcessImageNameW(process, 0, buffer, ctypes.byref(length)):
                            image = buffer.value[: length.value]
                    finally:
                        kernel32.CloseHandle(process)
                entries.append(
                    {
                        "pid": int(entry.th32ProcessID),
                        "parent_pid": int(entry.th32ParentProcessID),
                        "image_name": name,
                        "image_path_sha256": sha256_bytes(image.casefold().encode("utf-8")) if image else None,
                        "matches_expected_image": image.casefold() in expected if image else False,
                    }
                )
            available = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return entries


def windows_process_tree_snapshot() -> list[dict[str, Any]]:
    if os.name != "nt":
        return []

    class PROCESSENTRY32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", ctypes.c_ulong), ("cntUsage", ctypes.c_ulong),
            ("th32ProcessID", ctypes.c_ulong), ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", ctypes.c_ulong), ("cntThreads", ctypes.c_ulong),
            ("th32ParentProcessID", ctypes.c_ulong), ("pcPriClassBase", ctypes.c_long),
            ("dwFlags", ctypes.c_ulong), ("szExeFile", ctypes.c_wchar * 260),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateToolhelp32Snapshot.argtypes = [ctypes.c_ulong, ctypes.c_ulong]
    kernel32.CreateToolhelp32Snapshot.restype = ctypes.c_void_p
    kernel32.Process32FirstW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32FirstW.restype = ctypes.c_int
    kernel32.Process32NextW.argtypes = [ctypes.c_void_p, ctypes.POINTER(PROCESSENTRY32W)]
    kernel32.Process32NextW.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)
    if snapshot == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    entries: list[dict[str, Any]] = []
    entry = PROCESSENTRY32W()
    entry.dwSize = ctypes.sizeof(entry)
    try:
        available = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while available:
            entries.append(
                {
                    "pid": int(entry.th32ProcessID),
                    "parent_pid": int(entry.th32ParentProcessID),
                    "image_name": entry.szExeFile.lower(),
                }
            )
            available = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel32.CloseHandle(snapshot)
    return entries


def windows_process_counts(expected_images: tuple[Path, ...] = ()) -> dict[str, int]:
    counts = {"moss-helper.exe": 0, "llama-helper.exe": 0}
    for entry in windows_process_snapshot(expected_images):
        family = "moss-helper.exe" if entry["image_name"].startswith("moss-helper") else "llama-helper.exe"
        counts[family] += 1
    return counts


def descendant_process_ids(snapshot: list[dict[str, Any]], root_pid: int) -> list[int]:
    descendants: set[int] = set()
    changed = True
    while changed:
        changed = False
        parents = descendants | {root_pid}
        for entry in snapshot:
            if entry["parent_pid"] in parents and entry["pid"] not in descendants:
                descendants.add(entry["pid"])
                changed = True
    return sorted(descendants)


def available_physical_memory() -> int | None:
    if os.name != "nt":
        return None

    class MEMORYSTATUSEX(ctypes.Structure):
        _fields_ = [
            ("dwLength", ctypes.c_ulong), ("dwMemoryLoad", ctypes.c_ulong),
            ("ullTotalPhys", ctypes.c_ulonglong), ("ullAvailPhys", ctypes.c_ulonglong),
            ("ullTotalPageFile", ctypes.c_ulonglong), ("ullAvailPageFile", ctypes.c_ulonglong),
            ("ullTotalVirtual", ctypes.c_ulonglong), ("ullAvailVirtual", ctypes.c_ulonglong),
            ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.GlobalMemoryStatusEx.argtypes = [ctypes.POINTER(MEMORYSTATUSEX)]
    kernel32.GlobalMemoryStatusEx.restype = ctypes.c_int
    status = MEMORYSTATUSEX()
    status.dwLength = ctypes.sizeof(status)
    if not kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
        raise ctypes.WinError(ctypes.get_last_error())
    return int(status.ullAvailPhys)


def run_helper(helper: Path, payload: bytes, timeout: float = 45.0) -> dict[str, Any]:
    helper = helper.resolve(strict=True)
    started = time.perf_counter()
    process = subprocess.Popen(
        [str(helper)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=native_safe_environment(),
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    stderr_chunks: list[bytes] = []

    def drain_stderr() -> None:
        while True:
            chunk = process.stderr.read(4096)
            if not chunk:
                return
            stderr_chunks.append(chunk)

    stderr_thread = threading.Thread(target=drain_stderr, daemon=True)
    stderr_thread.start()
    process.stdin.write(payload)
    process.stdin.flush()
    messages: list[dict[str, Any]] = []
    text_payload_bytes = 0
    terminal: dict[str, Any] | None = None
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        line = process.stdout.readline()
        if not line:
            break
        message = json.loads(line)
        for text_key in ("text",):
            if isinstance(message.get(text_key), str):
                text_payload_bytes += len(message[text_key].encode("utf-8"))
                message[text_key] = ""
        messages.append(message)
        if message.get("terminal") is True or message.get("type") in {"shutdown"}:
            terminal = message
    if terminal is None:
        process.kill()
        process.wait(timeout=5)
        raise AssertionError("helper produced no terminal message")
    process.stdin.close()
    exit_code = process.wait(timeout=max(1.0, deadline - time.monotonic()))
    stderr_thread.join(timeout=2)
    stderr = b"".join(stderr_chunks)
    terminal_count = sum(
        1 for item in messages if item.get("terminal") is True or item.get("type") == "shutdown"
    )
    if terminal_count != 1:
        raise AssertionError(f"expected one terminal, got {terminal_count}")
    decoded_stderr = stderr.decode("utf-8", errors="replace")
    if ABSOLUTE_PATH.search(decoded_stderr):
        raise AssertionError("stderr leaked an absolute path")
    after = windows_process_tree_snapshot()
    live_ids = {entry["pid"] for entry in after}
    residual = sorted(
        ({process.pid} if process.pid in live_ids else set())
        | set(descendant_process_ids(after, process.pid))
    )
    if residual:
        raise AssertionError("helper process tree survived its terminal")
    return {
        "process_id": process.pid,
        "exit_code": exit_code,
        "terminal_type": terminal.get("type"),
        "error_code": terminal.get("code"),
        "terminal_count": terminal_count,
        "message_count": len(messages),
        "stderr_bytes": len(stderr),
        "stderr_sha256": sha256_bytes(stderr),
        "discarded_text_payload_bytes": text_payload_bytes,
        "wall_elapsed_ms": round((time.perf_counter() - started) * 1000),
        "residual_process_count": len(residual),
    }


def base_probe(runtime: Path, device_id: str | None = None) -> dict[str, Any]:
    return {
        "type": "probe",
        "v": PROTOCOL_VERSION,
        "request_id": REQUEST_ID,
        "client_seq": 1,
        "runtime": {"directory": str(runtime)},
        "device": {
            "kind": "vulkan",
            "description": "Intel(R) Arc(TM) Graphics",
            "device_id": device_id,
            "allow_primary_fallback": False,
        },
    }


def transcribe_message(runtime: Path, model: Path, audio: Path) -> dict[str, Any]:
    return {
        "type": "transcribe",
        "v": PROTOCOL_VERSION,
        "request_id": REQUEST_ID,
        "client_seq": 1,
        "context_sha256": "A" * 64,
        "runtime": {"directory": str(runtime)},
        "device": {
            "kind": "vulkan",
            "description": "Intel(R) Arc(TM) Graphics",
            "device_id": None,
            "allow_primary_fallback": False,
        },
        "model": {"path": str(model), "bytes": MODEL_BYTES, "sha256": MODEL_SHA256},
        "audio": {
            "path": str(audio),
            "format": "f32le",
            "sample_rate_hz": 16_000,
            "channels": 1,
            "samples": PCM_SAMPLES,
            "bytes": PCM_BYTES,
            "sha256": PCM_SHA256,
        },
    }


def jsonl(value: dict[str, Any]) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8") + b"\n"


def command_faults(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    helper = arguments.helper.resolve(strict=True)
    runtime = arguments.runtime.resolve(strict=True)
    model = arguments.model.resolve(strict=True)
    audio = arguments.audio.resolve(strict=True)
    if model.stat().st_size != MODEL_BYTES or sha256_file(model) != MODEL_SHA256:
        raise AssertionError("frozen model contract mismatch")
    if audio.stat().st_size != PCM_BYTES or sha256_file(audio) != PCM_SHA256:
        raise AssertionError("frozen audio contract mismatch")
    helper_hash = require_bound_artifact(helper, binding, "moss_helper")
    runtime_hash = require_bound_artifact(runtime / "contract.json", binding, "runtime_manifest")
    model_hash = require_bound_artifact(model, binding, "q8_model")
    fixture_hash = require_bound_artifact(audio, binding, "short_pcm")

    fixtures = arguments.private.parent / "fault-fixtures"
    fixtures.mkdir(parents=True, exist_ok=True)
    corrupt_model = fixtures / "corrupt-model.gguf"
    with corrupt_model.open("wb") as stream:
        stream.write(b"not-a-frozen-model")
    nan_audio = fixtures / "non-finite.f32le"
    shutil.copyfile(audio, nan_audio)
    with nan_audio.open("r+b") as stream:
        stream.write(struct.pack("<f", math.nan))

    cases: list[dict[str, Any]] = []

    def check(name: str, payload: bytes, code: str) -> None:
        result = run_helper(helper, payload)
        if result["error_code"] != code:
            raise AssertionError(f"{name}: expected {code}, got {result['error_code']}")
        cases.append({"case": name, "expected_code": code, **result})

    check("invalid_json", b"{not-json}\n", "PROTOCOL_INVALID_JSON")
    bad_audio_hash = transcribe_message(runtime, model, audio)
    bad_audio_hash["audio"]["sha256"] = "0" * 64
    check("audio_hash_mismatch", jsonl(bad_audio_hash), "AUDIO_HASH_MISMATCH")
    non_finite = transcribe_message(runtime, model, nan_audio)
    non_finite["audio"]["sha256"] = sha256_file(nan_audio)
    check("audio_non_finite", jsonl(non_finite), "AUDIO_NON_FINITE")
    bad_model = transcribe_message(runtime, corrupt_model, audio)
    check("model_contract_mismatch", jsonl(bad_model), "MODEL_CONTRACT_MISMATCH")
    unavailable = base_probe(runtime, "P2C-DEVICE-THAT-DOES-NOT-EXIST")
    check("vulkan_device_unavailable", jsonl(unavailable), "DEVICE_NOT_FOUND")
    verify_sha_sidecar(arguments.truncation_evidence)
    truncation = json.loads(arguments.truncation_evidence.read_text(encoding="utf-8"))
    expected_test = "native::imp::binding_tests::native_output_truncated_status_maps_to_stable_terminal_code"
    if (
        truncation.get("schema_version") != 1
        or truncation.get("stage") != "MOSS_V3_P2D_RUST_TRUNCATION_GATE"
        or truncation.get("status") != "PASS"
        or truncation.get("test_name") != expected_test
        or truncation.get("exit_code") != 0
        or truncation.get("binding_sha256") != binding_sha256
        or truncation.get("test_executable_sha256") != binding["artifacts"]["test_executable"]["sha256"]
    ):
        raise AssertionError("native truncation Rust gate is missing or invalid")
    cases.append(
        {
            "case": "native_output_truncated_rust_gate",
            "expected_code": "NATIVE_OUTPUT_TRUNCATED",
            "test_kind": "bound_rust_unit_test",
            "test_executable_sha256": truncation["test_executable_sha256"],
            "test_log_sha256": truncation["log_sha256"],
            "terminal_count": 1,
            "status": "PASS",
        }
    )
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2C_PROCESS_FAULTS",
        "status": "PASS",
        "case_count": len(cases),
        "cases": cases,
        **binding_fields(
            binding,
            binding_sha256,
            helper_binary_sha256=helper_hash,
            runtime_manifest_sha256=runtime_hash,
            model_sha256=model_hash,
            fixture_sha256=fixture_hash,
        ),
    }
    write_evidence(arguments.private, arguments.public, value)


def command_rust_truncation_gate(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    executable = arguments.test_executable.resolve(strict=True)
    require_bound_artifact(executable, binding, "test_executable")
    test_name = "native::imp::binding_tests::native_output_truncated_status_maps_to_stable_terminal_code"
    completed = subprocess.run(
        [str(executable), test_name, "--exact", "--nocapture"],
        capture_output=True,
        timeout=60,
    )
    log = completed.stdout + completed.stderr
    if completed.returncode != 0 or b"test result: ok" not in log:
        raise AssertionError("native truncation Rust gate failed")
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2D_RUST_TRUNCATION_GATE",
        "status": "PASS",
        "test_name": test_name,
        "test_executable_sha256": sha256_file(executable),
        "exit_code": completed.returncode,
        "log_sha256": sha256_bytes(log),
        "log_bytes": len(log),
        **binding_fields(binding, binding_sha256),
    }
    write_evidence(arguments.private, arguments.public, value)


def command_parent_kill(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    test_executable = arguments.test_executable.resolve(strict=True)
    helper = arguments.moss_helper.resolve(strict=True)
    require_bound_artifact(test_executable, binding, "test_executable")
    helper_hash = require_bound_artifact(helper, binding, "moss_helper")
    test_name = "moss_helper::manager::tests::p2c_parent_force_victim"
    with tempfile.TemporaryDirectory() as directory:
        ready_path = Path(directory) / "ready.json"
        environment = dict(os.environ)
        environment["MOSS_P2C_PARENT_READY"] = str(ready_path)
        process = subprocess.Popen(
            [str(test_executable), test_name, "--exact", "--ignored", "--nocapture"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline and not ready_path.is_file() and process.poll() is None:
            time.sleep(0.05)
        if not ready_path.is_file():
            process.kill()
            stdout, stderr = process.communicate(timeout=10)
            raise AssertionError(f"parent-kill victim never became ready ({sha256_bytes(stdout + stderr)})")
        ready = json.loads(ready_path.read_text(encoding="utf-8"))
        runner_pid = int(ready["runner_process_id"])
        helper_pid = int(ready["helper_process_id"])
        if runner_pid != process.pid or helper_pid <= 0:
            process.kill()
            process.communicate(timeout=10)
            raise AssertionError("parent-kill ready identity was invalid")
        before_helpers = windows_process_snapshot((helper,))
        if not any(
            entry["pid"] == helper_pid and entry["matches_expected_image"]
            for entry in before_helpers
        ):
            process.kill()
            process.communicate(timeout=10)
            raise AssertionError("parent-kill helper image was not the frozen sidecar")
        descendants = descendant_process_ids(windows_process_tree_snapshot(), runner_pid)
        if helper_pid not in descendants:
            process.kill()
            process.communicate(timeout=10)
            raise AssertionError("helper was not a descendant of the victim runner")
        process.kill()
        stdout, stderr = process.communicate(timeout=15)
        deadline = time.monotonic() + 10
        residual: list[int] = []
        while time.monotonic() < deadline:
            after = windows_process_tree_snapshot()
            live = {entry["pid"] for entry in after}
            residual = sorted(pid for pid in descendants if pid in live)
            if not residual:
                break
            time.sleep(0.05)
        if residual:
            raise AssertionError("parent-kill left a helper descendant")
        log = stdout + stderr
        value = {
            "schema_version": 1,
            "stage": "MOSS_V3_P2D_PARENT_KILL",
            "status": "PASS",
            "test_name": test_name,
            "test_executable_sha256": sha256_file(test_executable),
            "runner_process_id": runner_pid,
            "helper_process_id": helper_pid,
            "descendant_process_ids": descendants,
            "residual_process_count": 0,
            "exit_observed": process.returncode is not None,
            "log_sha256": sha256_bytes(log),
            "log_bytes": len(log),
            **binding_fields(
                binding,
                binding_sha256,
                helper_binary_sha256=helper_hash,
            ),
        }
        write_evidence(arguments.private, arguments.public, value)


def command_qwen(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    helper = arguments.helper.resolve(strict=True)
    moss_helper = arguments.moss_helper.resolve(strict=True)
    model = arguments.model.resolve(strict=True)
    if model.stat().st_size != arguments.model_bytes:
        raise AssertionError("Qwen model byte count mismatch")
    qwen_helper_hash = require_bound_artifact(helper, binding, "qwen_helper")
    require_bound_artifact(moss_helper, binding, "moss_helper")
    qwen_model_hash = require_bound_artifact(model, binding, "qwen_model")
    handoff_path = arguments.handoff_evidence.resolve(strict=True)
    verify_sha_sidecar(handoff_path)
    handoff = json.loads(handoff_path.read_text(encoding="utf-8"))
    assert_public_safe(handoff)
    if (
        handoff.get("status") != "PASS"
        or handoff.get("run_count") != 10
        or handoff.get("binding_sha256") != binding_sha256
        or handoff.get("helper_binary_sha256") != binding["artifacts"]["moss_helper"]["sha256"]
    ):
        raise AssertionError("the preceding MOSS handoff gate is not complete")
    expected_images = (moss_helper, helper)
    process_snapshot_before = windows_process_snapshot(expected_images)
    process_counts_before = windows_process_counts(expected_images)
    if process_counts_before["moss-helper.exe"] or process_counts_before["llama-helper.exe"]:
        raise AssertionError("a helper process was still active before Qwen handoff")
    available_before = available_physical_memory()
    started = time.perf_counter()
    process = subprocess.Popen(
        [str(helper)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        text=True,
        encoding="utf-8",
    )
    assert process.stdin is not None and process.stdout is not None and process.stderr is not None
    stderr_digest = hashlib.sha256()
    stderr_bytes = [0]

    def drain_stderr() -> None:
        while True:
            value = process.stderr.read(4096)
            if not value:
                return
            encoded = value.encode("utf-8", errors="replace")
            stderr_digest.update(encoded)
            stderr_bytes[0] += len(encoded)

    stderr_thread = threading.Thread(target=drain_stderr, daemon=True)
    stderr_thread.start()
    process.stdin.write(json.dumps({"type": "ping"}) + "\n")
    process.stdin.flush()
    if json.loads(process.stdout.readline()).get("type") != "pong":
        raise AssertionError("Qwen helper health probe failed")
    process.stdin.write(
        json.dumps(
            {
                "type": "generate",
                "prompt": "health",
                "max_tokens": 1,
                "context_size": 256,
                "model_path": str(model),
            }
        )
        + "\n"
    )
    process.stdin.flush()
    response = json.loads(process.stdout.readline())
    generated_bytes = len(str(response.pop("text", "")).encode("utf-8"))
    if response.get("type") != "response" or response.get("error") is not None:
        process.kill()
        raise AssertionError("Qwen model load/health failed")
    process.stdin.write(json.dumps({"type": "shutdown"}) + "\n")
    process.stdin.flush()
    if json.loads(process.stdout.readline()).get("type") != "goodbye":
        process.kill()
        raise AssertionError("Qwen helper shutdown failed")
    process.stdin.close()
    process.wait(timeout=30)
    stderr_thread.join(timeout=2)
    if process.returncode != 0:
        raise AssertionError("Qwen helper exit failed")
    process_snapshot_after = windows_process_snapshot(expected_images)
    process_counts_after = windows_process_counts(expected_images)
    if process_counts_after["moss-helper.exe"] or process_counts_after["llama-helper.exe"]:
        raise AssertionError("a helper process survived Qwen shutdown")
    available_after = available_physical_memory()
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2C_QWEN_RELEASE",
        "status": "PASS",
        "model_bytes": arguments.model_bytes,
        "health_probe": "pong",
        "model_generate_health": "response_without_error",
        "discarded_generated_text_bytes": generated_bytes,
        "helper_exit_code": process.returncode,
        "preceding_moss_evidence_sha256": sha256_file(arguments.handoff_evidence),
        "preceding_moss_run_count": handoff["run_count"],
        "process_counts_before": process_counts_before,
        "process_counts_after": process_counts_after,
        "process_snapshot_before": process_snapshot_before,
        "process_snapshot_after": process_snapshot_after,
        "residual_process_count": sum(process_counts_after.values()),
        "available_memory_before_bytes": available_before,
        "available_memory_after_bytes": available_after,
        "available_memory_delta_bytes": (
            available_after - available_before
            if available_after is not None and available_before is not None
            else None
        ),
        "gpu_available_memory_bytes": None,
        "stderr_bytes": stderr_bytes[0],
        "stderr_sha256": stderr_digest.hexdigest().upper(),
        "wall_elapsed_ms": round((time.perf_counter() - started) * 1000),
        **binding_fields(
            binding,
            binding_sha256,
            helper_binary_sha256=qwen_helper_hash,
            model_sha256=qwen_model_hash,
        ),
    }
    write_evidence(arguments.private, arguments.public, value)


def run_in_zero_capability_appcontainer(
    profile_name: str, executable: Path, command: list[str], working_directory: Path, timeout_seconds: int
) -> int:
    if os.name != "nt":
        raise AssertionError("AppContainer gate is Windows-only")

    class STARTUPINFO(ctypes.Structure):
        _fields_ = [
            ("cb", ctypes.c_ulong), ("lpReserved", ctypes.c_wchar_p),
            ("lpDesktop", ctypes.c_wchar_p), ("lpTitle", ctypes.c_wchar_p),
            ("dwX", ctypes.c_ulong), ("dwY", ctypes.c_ulong),
            ("dwXSize", ctypes.c_ulong), ("dwYSize", ctypes.c_ulong),
            ("dwXCountChars", ctypes.c_ulong), ("dwYCountChars", ctypes.c_ulong),
            ("dwFillAttribute", ctypes.c_ulong), ("dwFlags", ctypes.c_ulong),
            ("wShowWindow", ctypes.c_ushort), ("cbReserved2", ctypes.c_ushort),
            ("lpReserved2", ctypes.POINTER(ctypes.c_byte)),
            ("hStdInput", ctypes.c_void_p), ("hStdOutput", ctypes.c_void_p),
            ("hStdError", ctypes.c_void_p),
        ]

    class STARTUPINFOEX(ctypes.Structure):
        _fields_ = [("StartupInfo", STARTUPINFO), ("lpAttributeList", ctypes.c_void_p)]

    class PROCESS_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("hProcess", ctypes.c_void_p), ("hThread", ctypes.c_void_p),
            ("dwProcessId", ctypes.c_ulong), ("dwThreadId", ctypes.c_ulong),
        ]

    class SECURITY_CAPABILITIES(ctypes.Structure):
        _fields_ = [
            ("AppContainerSid", ctypes.c_void_p), ("Capabilities", ctypes.c_void_p),
            ("CapabilityCount", ctypes.c_ulong), ("Reserved", ctypes.c_ulong),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    userenv = ctypes.WinDLL("userenv", use_last_error=True)
    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel32.InitializeProcThreadAttributeList.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.POINTER(ctypes.c_size_t)]
    kernel32.InitializeProcThreadAttributeList.restype = ctypes.c_int
    kernel32.UpdateProcThreadAttribute.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p, ctypes.c_void_p]
    kernel32.UpdateProcThreadAttribute.restype = ctypes.c_int
    kernel32.DeleteProcThreadAttributeList.argtypes = [ctypes.c_void_p]
    kernel32.DeleteProcThreadAttributeList.restype = None
    kernel32.CreateProcessW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int, ctypes.c_ulong, ctypes.c_void_p, ctypes.c_wchar_p, ctypes.POINTER(STARTUPINFO), ctypes.POINTER(PROCESS_INFORMATION)]
    kernel32.CreateProcessW.restype = ctypes.c_int
    kernel32.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    kernel32.WaitForSingleObject.restype = ctypes.c_ulong
    kernel32.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    kernel32.TerminateProcess.restype = ctypes.c_int
    kernel32.GetExitCodeProcess.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
    kernel32.GetExitCodeProcess.restype = ctypes.c_int
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.restype = ctypes.c_int
    advapi32.FreeSid.argtypes = [ctypes.c_void_p]
    advapi32.FreeSid.restype = ctypes.c_void_p
    advapi32.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p)]
    advapi32.ConvertSidToStringSidW.restype = ctypes.c_int
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    kernel32.LocalFree.restype = ctypes.c_void_p
    sid = ctypes.c_void_p()
    userenv.DeriveAppContainerSidFromAppContainerName.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p)]
    userenv.DeriveAppContainerSidFromAppContainerName.restype = ctypes.c_long
    result = userenv.DeriveAppContainerSidFromAppContainerName(profile_name, ctypes.byref(sid))
    if result != 0 or not sid.value:
        raise OSError(result, "AppContainer profile SID lookup failed")
    size = ctypes.c_size_t()
    kernel32.InitializeProcThreadAttributeList(None, 1, 0, ctypes.byref(size))
    attribute_buffer = ctypes.create_string_buffer(size.value)
    attribute_list = ctypes.cast(attribute_buffer, ctypes.c_void_p)
    if not kernel32.InitializeProcThreadAttributeList(attribute_list, 1, 0, ctypes.byref(size)):
        raise ctypes.WinError(ctypes.get_last_error())
    security = SECURITY_CAPABILITIES(sid, None, 0, 0)
    if not kernel32.UpdateProcThreadAttribute(
        attribute_list,
        0,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        ctypes.byref(security),
        ctypes.sizeof(security),
        None,
        None,
    ):
        raise ctypes.WinError(ctypes.get_last_error())
    startup = STARTUPINFOEX()
    startup.StartupInfo.cb = ctypes.sizeof(startup)
    startup.lpAttributeList = attribute_list
    process_info = PROCESS_INFORMATION()
    command_line = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(executable), *command]))
    try:
        created = kernel32.CreateProcessW(
            str(executable), command_line, None, None, False,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            None, str(working_directory), ctypes.byref(startup.StartupInfo), ctypes.byref(process_info),
        )
        if not created:
            raise ctypes.WinError(ctypes.get_last_error())
        wait = kernel32.WaitForSingleObject(process_info.hProcess, timeout_seconds * 1000)
        if wait != WAIT_OBJECT_0:
            kernel32.TerminateProcess(process_info.hProcess, 0xE0000001)
            kernel32.WaitForSingleObject(process_info.hProcess, 5_000)
            raise TimeoutError("AppContainer child timed out")
        exit_code = ctypes.c_ulong()
        if not kernel32.GetExitCodeProcess(process_info.hProcess, ctypes.byref(exit_code)):
            raise ctypes.WinError(ctypes.get_last_error())
        return int(exit_code.value)
    finally:
        if process_info.hThread:
            kernel32.CloseHandle(process_info.hThread)
        if process_info.hProcess:
            kernel32.CloseHandle(process_info.hProcess)
        kernel32.DeleteProcThreadAttributeList(attribute_list)
        advapi32.FreeSid(sid)


def appcontainer_private_root(profile_name: str) -> Path:
    if os.name != "nt":
        raise AssertionError("AppContainer gate is Windows-only")
    userenv = ctypes.WinDLL("userenv", use_last_error=True)
    ole32 = ctypes.WinDLL("ole32", use_last_error=True)
    advapi32 = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    userenv.DeriveAppContainerSidFromAppContainerName.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_void_p)]
    userenv.DeriveAppContainerSidFromAppContainerName.restype = ctypes.c_long
    userenv.CreateAppContainerProfile.argtypes = [
        ctypes.c_wchar_p, ctypes.c_wchar_p, ctypes.c_wchar_p,
        ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_void_p),
    ]
    userenv.CreateAppContainerProfile.restype = ctypes.c_long
    userenv.GetAppContainerFolderPath.argtypes = [ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_wchar_p)]
    userenv.GetAppContainerFolderPath.restype = ctypes.c_long
    advapi32.FreeSid.argtypes = [ctypes.c_void_p]
    advapi32.FreeSid.restype = ctypes.c_void_p
    advapi32.ConvertSidToStringSidW.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p)]
    advapi32.ConvertSidToStringSidW.restype = ctypes.c_int
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    kernel32.LocalFree.restype = ctypes.c_void_p
    ole32.CoTaskMemFree.argtypes = [ctypes.c_void_p]
    ole32.CoTaskMemFree.restype = None
    sid = ctypes.c_void_p()
    result = userenv.CreateAppContainerProfile(
        profile_name, "Meetily MOSS P2D", "zero-capability MOSS acceptance",
        None, 0, ctypes.byref(sid),
    )
    if result not in (0, -2147024713):  # HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)
        raise OSError(result, "AppContainer profile creation failed")
    if not sid.value:
        result = userenv.DeriveAppContainerSidFromAppContainerName(profile_name, ctypes.byref(sid))
        if result != 0 or not sid.value:
            raise OSError(result, "AppContainer profile SID lookup failed")
    sid_text = ctypes.c_wchar_p()
    if not advapi32.ConvertSidToStringSidW(sid, ctypes.byref(sid_text)):
        advapi32.FreeSid(sid)
        raise ctypes.WinError(ctypes.get_last_error())
    advapi32.FreeSid(sid)
    folder = ctypes.c_wchar_p()
    result = userenv.GetAppContainerFolderPath(sid_text.value, ctypes.byref(folder))
    kernel32.LocalFree(ctypes.cast(sid_text, ctypes.c_void_p))
    if result != 0 or not folder.value:
        raise OSError(result, "AppContainer profile folder lookup failed")
    try:
        root = Path(folder.value) / "LocalState" / "MeetilyMossP2D"
    finally:
        ole32.CoTaskMemFree(ctypes.cast(folder, ctypes.c_void_p))
    root.mkdir(parents=True, exist_ok=True)
    return root.resolve(strict=True)


def copy_portable_python(venv_python: Path, destination: Path) -> Path:
    """Copy only the CPython runtime needed by the zero-capability child.

    A virtual-environment launcher still resolves its base interpreter outside the
    AppContainer package and consequently cannot start in the restricted token.
    Keeping a private CPython copy inside the profile avoids widening ACLs on any
    ancestor directory.  Third-party packages are deliberately excluded.
    """
    configuration = venv_python.resolve(strict=True).parent.parent / "pyvenv.cfg"
    values: dict[str, str] = {}
    for line in configuration.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if separator:
            values[key.strip().lower()] = value.strip()
    source = Path(values.get("home", "")).resolve(strict=True)
    destination.mkdir()
    for name in ("python.exe", "python3.dll", "python312.dll", "vcruntime140.dll", "vcruntime140_1.dll"):
        shutil.copyfile(source / name, destination / name)
    shutil.copytree(source / "DLLs", destination / "DLLs")
    shutil.copytree(
        source / "Lib",
        destination / "Lib",
        ignore=shutil.ignore_patterns("site-packages", "__pycache__", "*.pyc", "*.pyo"),
    )
    return (destination / "python.exe").resolve(strict=True)


OFFLINE_CHILD = r'''import ctypes,hashlib,json,os,socket,subprocess,sys,time
from pathlib import Path
helper,runtime,model,audio,output=map(Path,sys.argv[1:])
def token_policy():
    k=ctypes.WinDLL("kernel32",use_last_error=True); a=ctypes.WinDLL("advapi32",use_last_error=True); token=ctypes.c_void_p()
    a.OpenProcessToken.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.POINTER(ctypes.c_void_p)]; a.OpenProcessToken.restype=ctypes.c_int
    a.GetTokenInformation.argtypes=[ctypes.c_void_p,ctypes.c_int,ctypes.c_void_p,ctypes.c_ulong,ctypes.POINTER(ctypes.c_ulong)]; a.GetTokenInformation.restype=ctypes.c_int
    k.GetCurrentProcess.argtypes=[]; k.GetCurrentProcess.restype=ctypes.c_void_p; k.CloseHandle.argtypes=[ctypes.c_void_p]; k.CloseHandle.restype=ctypes.c_int
    if not a.OpenProcessToken(k.GetCurrentProcess(),0x0008,ctypes.byref(token)): raise ctypes.WinError(ctypes.get_last_error())
    try:
        app=ctypes.c_ulong(); needed=ctypes.c_ulong()
        if not a.GetTokenInformation(token,29,ctypes.byref(app),ctypes.sizeof(app),ctypes.byref(needed)): raise ctypes.WinError(ctypes.get_last_error())
        a.GetTokenInformation(token,30,None,0,ctypes.byref(needed)); buf=ctypes.create_string_buffer(needed.value)
        if not a.GetTokenInformation(token,30,buf,needed.value,ctypes.byref(needed)): raise ctypes.WinError(ctypes.get_last_error())
        return app.value==1,ctypes.c_ulong.from_buffer(buf).value
    finally: k.CloseHandle(token)
def blocked():
    try:
        socket.create_connection(("1.1.1.1",443),timeout=2).close(); return False
    except BaseException: return True
is_appcontainer,capability_count=token_policy(); before=blocked()
env={k:v for k,v in os.environ.items() if not (k.upper().startswith(("VK_","TRANSCRIBE_","GGML_")) or k.upper()=="VULKAN_SDK")}
message={"type":"transcribe","v":1,"request_id":"798d8c63-5ff1-40e3-9db8-0f706aeb930a","client_seq":1,"context_sha256":"A"*64,"runtime":{"directory":str(runtime)},"device":{"kind":"vulkan","description":"Intel(R) Arc(TM) Graphics","device_id":None,"allow_primary_fallback":False},"model":{"path":str(model),"bytes":986899616,"sha256":"64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039"},"audio":{"path":str(audio),"format":"f32le","sample_rate_hz":16000,"channels":1,"samples":192597,"bytes":770388,"sha256":"2A1997D757808E64BED66B774DB0D1F4219ECCB13E77C2D7585E712458509A02"}}
started=time.perf_counter(); p=subprocess.Popen([str(helper)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env=env,text=True,encoding="utf-8")
p.stdin.write(json.dumps(message,separators=(",",":"))+"\n"); p.stdin.flush(); terminal=None; count=0; discarded=0
for line in p.stdout:
    item=json.loads(line)
    if isinstance(item.get("text"),str): discarded+=len(item["text"].encode("utf-8"))
    if item.get("terminal") is True: terminal=item; count+=1; break
p.stdin.close(); code=p.wait(timeout=30); after=blocked()
value={"token_is_appcontainer":is_appcontainer,"capability_count":capability_count,"network_before_blocked":before,"network_after_blocked":after,"terminal_type":terminal.get("type") if terminal else None,"error_code":terminal.get("code") if terminal else None,"error_phase":terminal.get("phase") if terminal else None,"native_status":terminal.get("native_status") if terminal else None,"terminal_count":count,"helper_exit_code":code,"discarded_text_payload_bytes":discarded,"wall_elapsed_ms":round((time.perf_counter()-started)*1000)}
output.write_text(json.dumps(value,sort_keys=True),encoding="utf-8")
raise SystemExit(0 if is_appcontainer and capability_count==0 and before and after and value["terminal_type"]=="completed" and count==1 and code==0 else 2)
'''


def command_offline(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    helper_hash = require_bound_artifact(arguments.helper, binding, "moss_helper")
    runtime_hash = require_bound_artifact(arguments.runtime / "contract.json", binding, "runtime_manifest")
    model_hash = require_bound_artifact(arguments.model, binding, "q8_model")
    fixture_hash = require_bound_artifact(arguments.audio, binding, "short_pcm")
    bound = binding_fields(
        binding,
        binding_sha256,
        helper_binary_sha256=helper_hash,
        runtime_manifest_sha256=runtime_hash,
        model_sha256=model_hash,
        fixture_sha256=fixture_hash,
    )
    root = appcontainer_private_root(arguments.profile)
    run_directory = root / f"p2c-{uuid.uuid4().hex}"
    run_directory.mkdir()
    child = run_directory / "offline-child.py"
    helper_copy = run_directory / "moss-helper.exe"
    audio_copy = run_directory / "audio.f32le"
    runtime_copy = run_directory / "runtime"
    model_copy = run_directory / "model.gguf"
    result_path = run_directory / "result.json"
    private_python = copy_portable_python(arguments.python, run_directory / "python")
    child.write_text(OFFLINE_CHILD, encoding="utf-8")
    shutil.copyfile(arguments.helper.resolve(strict=True), helper_copy)
    shutil.copyfile(arguments.audio.resolve(strict=True), audio_copy)
    shutil.copytree(arguments.runtime.resolve(strict=True), runtime_copy)
    shutil.copyfile(arguments.model.resolve(strict=True), model_copy)
    exit_code: int | None = None
    try:
        exit_code = run_in_zero_capability_appcontainer(
            arguments.profile,
            private_python,
            [str(child), str(helper_copy), str(runtime_copy), str(model_copy), str(audio_copy), str(result_path)],
            run_directory,
            180,
        )
    except BaseException:
        exit_code = None
    redacted_child_result: dict[str, object] = {}
    if result_path.is_file():
        try:
            parsed_child_result = json.loads(result_path.read_text(encoding="utf-8"))
            for field in (
                "token_is_appcontainer", "capability_count", "network_before_blocked",
                "network_after_blocked", "terminal_type", "error_code", "error_phase",
                "native_status", "terminal_count", "helper_exit_code",
                "discarded_text_payload_bytes", "wall_elapsed_ms",
            ):
                if field in parsed_child_result:
                    redacted_child_result[field] = parsed_child_result[field]
        except (OSError, ValueError, TypeError):
            redacted_child_result = {}
    if exit_code != 0 or not result_path.is_file():
        value = {
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_ZERO_NETWORK_REAL_SHORT",
            "status": "FAIL",
            "isolation": "Windows AppContainer with zero capabilities",
            "appcontainer_child_exit_code": exit_code,
            "result_present": result_path.is_file(),
            "failure_code": "APPCONTAINER_CHILD_FAILED",
            "residual_process_count": sum(windows_process_counts().values()),
            **redacted_child_result,
            **bound,
        }
        write_evidence(arguments.private, arguments.public, value)
        shutil.rmtree(run_directory)
        raise AssertionError("offline AppContainer child failed")
    try:
        child_result = json.loads(result_path.read_text(encoding="utf-8"))
    except BaseException:
        value = {
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_ZERO_NETWORK_REAL_SHORT",
            "status": "FAIL",
            "isolation": "Windows AppContainer with zero capabilities",
            "appcontainer_child_exit_code": exit_code,
            "result_present": True,
            "failure_code": "APPCONTAINER_RESULT_INVALID",
            "residual_process_count": sum(windows_process_counts().values()),
            **bound,
        }
        write_evidence(arguments.private, arguments.public, value)
        shutil.rmtree(run_directory)
        raise AssertionError("offline AppContainer result was invalid")
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2C_ZERO_NETWORK_REAL_SHORT",
        "status": "PASS",
        "isolation": "Windows AppContainer with zero capabilities",
        "appcontainer_child_exit_code": exit_code,
        **child_result,
        "residual_process_count": 0,
        **bound,
    }
    write_evidence(arguments.private, arguments.public, value)
    shutil.rmtree(run_directory)


def command_manifest(arguments: argparse.Namespace) -> None:
    command_evidence_manifest(
        arguments.public_dir,
        "MOSS_V3_P2C_PUBLIC_MANIFEST",
    )


def command_private_manifest(arguments: argparse.Namespace) -> None:
    command_evidence_manifest(
        arguments.private_dir,
        "MOSS_V3_P2C_PRIVATE_MANIFEST",
    )


def command_publish_safe(arguments: argparse.Namespace) -> None:
    source = arguments.source.resolve(strict=True)
    verify_sha_sidecar(source)
    value = json.loads(source.read_text(encoding="utf-8"))
    assert_public_safe(value)
    atomic_json(arguments.public, value)


def moss_crash_event_count_since(start: str) -> int:
    if os.name != "nt":
        raise AssertionError("Windows Application event audit requires Windows")
    command = (
        "$start=[DateTimeOffset]::Parse($env:MOSS_WER_START).LocalDateTime;"
        "$events=Get-WinEvent -FilterHashtable "
        "@{LogName='Application';Id=1000,1001;StartTime=$start} "
        "-ErrorAction SilentlyContinue | Where-Object {$_.Message -match 'moss-helper'};"
        "[Console]::Out.Write(@($events).Count)"
    )
    environment = dict(os.environ)
    environment["MOSS_WER_START"] = start
    completed = subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", command],
        check=True,
        capture_output=True,
        text=True,
        env=environment,
        timeout=30,
    )
    return int(completed.stdout.strip())


def artifact_record(path: Path) -> dict[str, Any]:
    path = path.resolve(strict=True)
    return {"bytes": path.stat().st_size, "sha256": sha256_file(path)}


def load_acceptance_binding(path: Path) -> tuple[dict[str, Any], str]:
    path = path.resolve(strict=True)
    verify_sha_sidecar(path)
    binding = json.loads(path.read_text(encoding="utf-8"))
    validate_binding(binding)
    return binding, sha256_file(path)


def require_bound_artifact(path: Path, binding: dict[str, Any], name: str) -> str:
    record = artifact_record(path)
    if record != binding["artifacts"][name]:
        raise AssertionError("runtime gate artifact does not match the acceptance binding")
    return record["sha256"]


def binding_fields(binding: dict[str, Any], binding_sha256: str, **hashes: str) -> dict[str, Any]:
    return {
        "binding_sha256": binding_sha256,
        "source_commit": binding["commit"],
        "source_tree_clean": binding["tree_clean"],
        "cargo_lock_sha256": binding["artifacts"]["cargo_lock"]["sha256"],
        "test_executable_sha256": binding["artifacts"]["test_executable"]["sha256"],
        **hashes,
    }


def command_binding(arguments: argparse.Namespace) -> None:
    repo = arguments.repo.resolve(strict=True)
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()
    tree_clean = not subprocess.run(
        ["git", "status", "--porcelain"], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()
    if not tree_clean:
        raise AssertionError("source tree must be clean before binding acceptance artifacts")
    artifacts = {
        "cargo_lock": artifact_record(repo / "Cargo.lock"),
        "test_executable": artifact_record(arguments.test_executable),
        "moss_helper": artifact_record(arguments.moss_helper),
        "qwen_helper": artifact_record(arguments.qwen_helper),
        "runtime_manifest": artifact_record(arguments.runtime_manifest),
        "q8_model": artifact_record(arguments.q8_model),
        "qwen_model": artifact_record(arguments.qwen_model),
        "short_pcm": artifact_record(arguments.short_pcm),
        "three_hundred_pcm": artifact_record(arguments.three_hundred_pcm),
        "four_eighty_pcm": artifact_record(arguments.four_eighty_pcm),
        "six_hundred_pcm": artifact_record(arguments.six_hundred_pcm),
    }
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2D_BINDING",
        "status": "PASS",
        "commit": commit,
        "tree_clean": tree_clean,
        "artifacts": artifacts,
    }
    write_evidence(arguments.private, arguments.public, value)


def validate_binding(binding: dict[str, Any]) -> None:
    assert_public_safe(binding)
    if (
        binding.get("schema_version") != 1
        or binding.get("stage") != "MOSS_V3_P2D_BINDING"
        or binding.get("status") != "PASS"
        or binding.get("tree_clean") is not True
        or not isinstance(binding.get("commit"), str)
        or re.fullmatch(r"[0-9a-f]{40}", binding["commit"]) is None
        or not isinstance(binding.get("artifacts"), dict)
        or set(binding["artifacts"]) != BINDING_ARTIFACT_NAMES
    ):
        raise AssertionError("acceptance binding is incomplete")
    for artifact in binding["artifacts"].values():
        if (
            not isinstance(artifact, dict)
            or set(artifact) != {"bytes", "sha256"}
            or not isinstance(artifact.get("bytes"), int)
            or isinstance(artifact.get("bytes"), bool)
            or artifact["bytes"] <= 0
            or not isinstance(artifact.get("sha256"), str)
            or re.fullmatch(r"[0-9A-F]{64}", artifact["sha256"]) is None
        ):
            raise AssertionError("acceptance artifact binding is incomplete")


def validate_bound_source_value(
    value: dict[str, Any],
    expected_stage: str,
    binding: dict[str, Any],
    binding_sha256: str,
    fixture_name: str,
) -> dict[str, Any]:
    assert_public_safe(value)
    require_all_finite_numbers(value)
    if (
        value.get("schema_version") != 1
        or value.get("stage") != expected_stage
        or value.get("status") != "PASS"
        or value.get("binding_sha256") != binding_sha256
        or value.get("source_commit") != binding.get("commit")
        or value.get("source_tree_clean") is not True
    ):
        raise AssertionError("performance source identity is incomplete")
    artifacts = binding["artifacts"]
    expected = {
        "cargo_lock_sha256": artifacts["cargo_lock"]["sha256"],
        "test_executable_sha256": artifacts["test_executable"]["sha256"],
        "helper_binary_sha256": artifacts["moss_helper"]["sha256"],
        "runtime_manifest_sha256": artifacts["runtime_manifest"]["sha256"],
        "model_sha256": artifacts["q8_model"]["sha256"],
        "fixture_sha256": artifacts[fixture_name]["sha256"],
    }
    if any(value.get(key) != expected_value for key, expected_value in expected.items()):
        raise AssertionError("performance source artifact binding does not match")
    return value


def load_bound_source(
    path: Path,
    expected_stage: str,
    binding: dict[str, Any],
    binding_sha256: str,
    fixture_name: str,
) -> dict[str, Any]:
    verify_sha_sidecar(path)
    value = json.loads(path.read_text(encoding="utf-8"))
    return validate_bound_source_value(
        value, expected_stage, binding, binding_sha256, fixture_name
    )


def finite_nonnegative(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def require_all_finite_numbers(value: Any) -> None:
    if isinstance(value, float) and not math.isfinite(value):
        raise AssertionError("evidence contains a non-finite number")
    if isinstance(value, dict):
        for child in value.values():
            require_all_finite_numbers(child)
    elif isinstance(value, list):
        for child in value:
            require_all_finite_numbers(child)


def validate_helper_runs(
    runs: Any, expected_offsets: list[int], expected_durations: list[int]
) -> None:
    if not isinstance(runs, list) or len(runs) != len(expected_durations):
        raise AssertionError("helper run count does not match the frozen chunk plan")
    process_ids: set[int] = set()
    for index, (run, offset, duration) in enumerate(zip(runs, expected_offsets, expected_durations)):
        if not isinstance(run, dict):
            raise AssertionError("helper run entry is invalid")
        process_id = run.get("processId")
        last_timestamp = run.get("lastTimestampMs")
        native_rtf = run.get("nativeRtf")
        if (
            run.get("chunkIndex") != index
            or run.get("inputOffsetMs") != offset
            or run.get("inputDurationMs") != duration
            or not isinstance(process_id, int)
            or isinstance(process_id, bool)
            or process_id <= 0
            or process_id in process_ids
            or run.get("terminalCount") != 1
            or run.get("residualProcessCount") != 0
            or not finite_nonnegative(run.get("peakJobMemoryBytes"))
            or not finite_nonnegative(run.get("totalProcesses"))
            or not finite_nonnegative(native_rtf)
            or native_rtf > 1.0
            or not finite_nonnegative(last_timestamp)
            or last_timestamp > duration
            or last_timestamp < max(0, duration - 10_000)
            or re.fullmatch(r"[0-9A-F]{64}", str(run.get("rawTextSha256", ""))) is None
            or re.fullmatch(r"[0-9A-F]{64}", str(run.get("cleanTextSha256", ""))) is None
        ):
            raise AssertionError("helper run did not satisfy its frozen gate")
        process_ids.add(process_id)


def _command_performance_checked(arguments: argparse.Namespace) -> None:
    verify_sha_sidecar(arguments.binding)
    binding = json.loads(arguments.binding.read_text(encoding="utf-8"))
    validate_binding(binding)
    binding_sha256 = sha256_file(arguments.binding)
    single_first = load_bound_source(arguments.single_first, "MOSS_V3_P2C_300S_SINGLE_CHILD", binding, binding_sha256, "three_hundred_pcm")
    single_second = load_bound_source(arguments.single_second, "MOSS_V3_P2C_300S_SINGLE_CHILD", binding, binding_sha256, "three_hundred_pcm")
    single_third = load_bound_source(arguments.single_third, "MOSS_V3_P2C_300S_SINGLE_CHILD", binding, binding_sha256, "three_hundred_pcm")
    four_eighty = load_bound_source(arguments.four_eighty, "MOSS_V3_P2C_480S_PERFORMANCE", binding, binding_sha256, "four_eighty_pcm")
    six_hundred = load_bound_source(arguments.six_hundred, "MOSS_V3_P2C_600S_PERFORMANCE", binding, binding_sha256, "six_hundred_pcm")
    six_runs = six_hundred["helper_runs"]
    four_runs = four_eighty["helper_runs"]
    require_all_finite_numbers(binding)
    for source in (single_first, single_second, single_third, four_eighty, six_hundred):
        require_all_finite_numbers(source)
    validate_helper_runs(single_first.get("helper_runs"), [0], [300_000])
    validate_helper_runs(single_second.get("helper_runs"), [0], [300_000])
    validate_helper_runs(single_third.get("helper_runs"), [0], [300_000])
    validate_helper_runs(four_runs, [0, 300_000], [300_000, 180_000])
    validate_helper_runs(six_runs, [0, 300_000], [300_000, 300_000])
    if (
        six_hundred.get("helper_process_id") != 0
        or [run.get("inputDurationMs") for run in six_runs] != [300_000, 300_000]
        or [run.get("inputOffsetMs") for run in six_runs] != [0, 300_000]
        or any(run.get("terminalCount") != 1 or run.get("residualProcessCount") != 0 for run in six_runs)
        or len({run.get("processId") for run in six_runs}) != 2
    ):
        raise AssertionError("600 second request was not split into two 300 second children")
    if (
        four_eighty.get("helper_process_id") != 0
        or [run.get("inputDurationMs") for run in four_runs] != [300_000, 180_000]
        or [run.get("inputOffsetMs") for run in four_runs] != [0, 300_000]
        or any(run.get("terminalCount") != 1 or run.get("residualProcessCount") != 0 for run in four_runs)
        or len({run.get("processId") for run in four_runs}) != 2
    ):
        raise AssertionError("480 second request was not split into 300+180 second children")
    if (
        single_first.get("helper_process_id") != single_first["helper_runs"][0]["processId"]
        or single_second.get("helper_process_id") != single_second["helper_runs"][0]["processId"]
        or single_third.get("helper_process_id") != single_third["helper_runs"][0]["processId"]
        or any(source.get("residual_process_count") != 0 for source in (single_first, single_second, single_third, four_eighty, six_hundred))
    ):
        raise AssertionError("helper PID or residual accounting is inconsistent")
    helper_hashes = {
        single_first["helper_binary_sha256"],
        single_second["helper_binary_sha256"],
        single_third["helper_binary_sha256"],
        four_eighty["helper_binary_sha256"],
        six_hundred["helper_binary_sha256"],
    }
    if len(helper_hashes) != 1:
        raise AssertionError("performance gates used different helper binaries")
    if arguments.fixture_sha256.upper() != binding["artifacts"]["three_hundred_pcm"]["sha256"]:
        raise AssertionError("300 second fixture hash does not match the acceptance binding")
    numeric_values = [
        single_first.get("supervisor_wall_rtf"), single_second.get("supervisor_wall_rtf"), single_third.get("supervisor_wall_rtf"),
        four_eighty.get("supervisor_wall_rtf"), six_hundred.get("supervisor_wall_rtf"),
        single_first.get("last_timestamp_ms"), single_second.get("last_timestamp_ms"), single_third.get("last_timestamp_ms"),
        four_eighty.get("last_timestamp_ms"), six_hundred.get("supervisor_wall_elapsed_ms"),
    ]
    if not all(finite_nonnegative(value) for value in numeric_values):
        raise AssertionError("performance source contains missing or non-finite metrics")
    if four_eighty["supervisor_wall_rtf"] > 1.0 or six_hundred["supervisor_wall_rtf"] > 1.0:
        raise AssertionError("performance source exceeded the frozen wall RTF")
    repeated = [
        {
            "raw": single_first["raw_text_sha256"],
            "clean": single_first["clean_text_sha256"],
            "last": single_first["last_timestamp_ms"],
            "rtf": single_first["supervisor_wall_rtf"],
            "terminal": single_first["helper_runs"][0]["terminalCount"],
            "residual": single_first["helper_runs"][0]["residualProcessCount"],
        },
        {
            "raw": single_second["raw_text_sha256"],
            "clean": single_second["clean_text_sha256"],
            "last": single_second["last_timestamp_ms"],
            "rtf": single_second["supervisor_wall_rtf"],
            "terminal": single_second["helper_runs"][0]["terminalCount"],
            "residual": single_second["helper_runs"][0]["residualProcessCount"],
        },
        {
            "raw": single_third["raw_text_sha256"],
            "clean": single_third["clean_text_sha256"],
            "last": single_third["last_timestamp_ms"],
            "rtf": single_third["supervisor_wall_rtf"],
            "terminal": single_third["helper_runs"][0]["terminalCount"],
            "residual": single_third["helper_runs"][0]["residualProcessCount"],
        },
    ]
    if len({run["raw"] for run in repeated}) != 1 or len({run["clean"] for run in repeated}) != 1:
        raise AssertionError("three identical 300 second inputs produced different output hashes")
    if any(
        run["last"] < 290_000
        or not math.isfinite(run["rtf"])
        or run["rtf"] > 1.0
        or run["terminal"] != 1
        or run["residual"] != 0
        for run in repeated
    ):
        raise AssertionError("300 second repeatability gate failed")
    if (
        four_eighty["supervisor_wall_rtf"] > 1.0
        or six_hundred["supervisor_wall_rtf"] > 1.0
        or four_eighty["last_timestamp_ms"] < 470_000
        or any(run["residualProcessCount"] != 0 for run in four_runs + six_runs)
    ):
        raise AssertionError("chunked long-request gate failed")
    crash_events = moss_crash_event_count_since(arguments.wer_start)
    if crash_events != 0:
        raise AssertionError("new moss-helper Windows crash events were recorded")
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2C_CHUNKED_PERFORMANCE",
        "status": "PASS",
        **binding_fields(
            binding,
            binding_sha256,
            helper_binary_sha256=next(iter(helper_hashes)),
            runtime_manifest_sha256=binding["artifacts"]["runtime_manifest"]["sha256"],
            model_sha256=binding["artifacts"]["q8_model"]["sha256"],
            fixture_sha256=binding["artifacts"]["three_hundred_pcm"]["sha256"],
        ),
        "single_native_chunk_limit_seconds": 300,
        "maximum_product_request_seconds": 7_200,
        "identical_300_second_fixture_sha256": arguments.fixture_sha256.upper(),
        "identical_300_second_run_count": 3,
        "identical_output_hashes": True,
        "unique_terminal_per_run": True,
        "residual_process_count": 0,
        "new_windows_error_1000_1001_count": crash_events,
        "four_hundred_eighty": {
            "child_durations_ms": [run["inputDurationMs"] for run in four_runs],
            "last_timestamp_ms": four_eighty["last_timestamp_ms"],
            "supervisor_wall_rtf": four_eighty["supervisor_wall_rtf"],
        },
        "six_hundred": {
            "child_durations_ms": [run["inputDurationMs"] for run in six_runs],
            "last_timestamp_ms": six_runs[-1]["inputOffsetMs"] + six_runs[-1]["lastTimestampMs"],
            "supervisor_wall_rtf": six_hundred["supervisor_wall_rtf"],
        },
        "source_evidence_sha256": {
            "single_first": sha256_file(arguments.single_first),
            "single_second": sha256_file(arguments.single_second),
            "single_third": sha256_file(arguments.single_third),
            "four_eighty": sha256_file(arguments.four_eighty),
            "six_hundred": sha256_file(arguments.six_hundred),
        },
        "source_sidecar_sha256": {
            "single_first": sha256_file(arguments.single_first.with_name(arguments.single_first.name + ".sha256")),
            "single_second": sha256_file(arguments.single_second.with_name(arguments.single_second.name + ".sha256")),
            "single_third": sha256_file(arguments.single_third.with_name(arguments.single_third.name + ".sha256")),
            "four_eighty": sha256_file(arguments.four_eighty.with_name(arguments.four_eighty.name + ".sha256")),
            "six_hundred": sha256_file(arguments.six_hundred.with_name(arguments.six_hundred.name + ".sha256")),
        },
    }
    write_evidence(arguments.private, arguments.public, value)


def command_performance(arguments: argparse.Namespace) -> None:
    try:
        _command_performance_checked(arguments)
    except BaseException as error:
        value = {
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_CHUNKED_PERFORMANCE",
            "status": "BLOCKED",
            "failure_code": "PERFORMANCE_SOURCE_INVALID",
        }
        write_evidence(arguments.private, arguments.public, value)
        raise AssertionError("performance acceptance is blocked") from error


def command_code_validation(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    repo = arguments.repo.resolve(strict=True)
    test_executable = arguments.test_executable.resolve(strict=True)
    require_bound_artifact(repo / "Cargo.lock", binding, "cargo_lock")
    require_bound_artifact(test_executable, binding, "test_executable")
    python = arguments.python.resolve(strict=True)
    commands = [
        ("git_diff_check", ["git", "diff", "--check"]),
        ("cargo_fmt", ["cargo", "fmt", "--all", "--", "--check"]),
        (
            "moss_helper_clippy",
            ["cargo", "clippy", "--locked", "--offline", "-p", "moss-helper", "--all-targets", "--", "-D", "warnings"],
        ),
        ("moss_helper_tests", ["cargo", "test", "--locked", "--offline", "-p", "moss-helper"]),
        (
            "qa_harness_tests",
            [str(python), "-m", "unittest", "-v", "scripts.qa.test_moss_v3_p2c_acceptance"],
        ),
        ("tauri_test_list", [str(test_executable), "--list"]),
    ]
    checks: list[dict[str, Any]] = []
    listed_test_count = 0
    for name, command in commands:
        completed = subprocess.run(
            command,
            cwd=repo,
            capture_output=True,
            timeout=900,
            env=native_safe_environment(),
        )
        log = completed.stdout + completed.stderr
        if completed.returncode != 0:
            raise AssertionError(f"fixed code-validation gate failed: {name}")
        if name == "tauri_test_list":
            listed_test_count = sum(
                1 for line in completed.stdout.decode("utf-8", errors="replace").splitlines()
                if line.rstrip().endswith(": test")
            )
            if listed_test_count < 500:
                raise AssertionError("the Tauri test executable listed too few tests")
        checks.append(
            {
                "gate": name,
                "exit_code": completed.returncode,
                "log_sha256": sha256_bytes(log),
                "log_bytes": len(log),
            }
        )
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2D_CODE_VALIDATION",
        "status": "PASS",
        "checks": checks,
        "listed_test_count": listed_test_count,
        **binding_fields(binding, binding_sha256),
    }
    write_evidence(arguments.private, arguments.public, value)


FINAL_GATE_STAGES = {
    "code_validation": "MOSS_V3_P2D_CODE_VALIDATION",
    "ten_sequential": "MOSS_V3_P2C_TEN_SEQUENTIAL_MANAGER",
    "chunk_control": "MOSS_V3_P2C_CHUNK_CONTROL",
    "real_control": "MOSS_V3_P2C_REAL_CONTROL",
    "performance": "MOSS_V3_P2C_CHUNKED_PERFORMANCE",
    "process_faults": "MOSS_V3_P2C_PROCESS_FAULTS",
    "truncation": "MOSS_V3_P2D_RUST_TRUNCATION_GATE",
    "offline": "MOSS_V3_P2C_ZERO_NETWORK_REAL_SHORT",
    "parent_kill": "MOSS_V3_P2D_PARENT_KILL",
    "duplicate_request": "MOSS_V3_P2D_DUPLICATE_REQUEST",
    "second_chunk_failure": "MOSS_V3_P2D_SECOND_CHUNK_FAILURE",
    "qwen": "MOSS_V3_P2C_QWEN_RELEASE",
}

FINAL_GATE_ARTIFACT_FIELDS = {
    "ten_sequential": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "short_pcm",
    },
    "chunk_control": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "six_hundred_pcm",
    },
    "real_control": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "short_pcm",
    },
    "performance": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "three_hundred_pcm",
    },
    "process_faults": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "short_pcm",
    },
    "offline": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "short_pcm",
    },
    "parent_kill": {"helper_binary_sha256": "moss_helper"},
    "duplicate_request": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "six_hundred_pcm",
    },
    "second_chunk_failure": {
        "helper_binary_sha256": "moss_helper", "runtime_manifest_sha256": "runtime_manifest",
        "model_sha256": "q8_model", "fixture_sha256": "six_hundred_pcm",
    },
    "qwen": {"helper_binary_sha256": "qwen_helper", "model_sha256": "qwen_model"},
}


def validate_final_gate_source(
    name: str,
    value: dict[str, Any],
    binding: dict[str, Any],
    binding_sha256: str,
) -> None:
    expected_stage = FINAL_GATE_STAGES[name]
    assert_public_safe(value)
    require_all_finite_numbers(value)
    if (
        value.get("schema_version") != 1
        or value.get("stage") != expected_stage
        or value.get("status") != "PASS"
        or value.get("binding_sha256") != binding_sha256
        or value.get("source_commit") != binding["commit"]
        or value.get("source_tree_clean") is not True
        or value.get("cargo_lock_sha256") != binding["artifacts"]["cargo_lock"]["sha256"]
        or value.get("test_executable_sha256")
        != binding["artifacts"]["test_executable"]["sha256"]
    ):
        raise AssertionError("a final acceptance source is incomplete")
    for field, artifact_name in FINAL_GATE_ARTIFACT_FIELDS.get(name, {}).items():
        if value.get(field) != binding["artifacts"][artifact_name]["sha256"]:
            raise AssertionError("a final acceptance source used an unbound artifact")


def validate_final_gate_invariants(sources: dict[str, dict[str, Any]]) -> None:
    if (
        set(sources) != set(FINAL_GATE_STAGES)
        or sources["ten_sequential"].get("run_count") != 10
        or sources["ten_sequential"].get("cross_run_hash_isolation") is not True
        or sources["performance"].get("identical_300_second_run_count") != 3
        or sources["performance"].get("identical_output_hashes") is not True
        or sources["performance"].get("unique_terminal_per_run") is not True
        or sources["performance"].get("new_windows_error_1000_1001_count") != 0
        or sources["performance"].get("residual_process_count") != 0
        or sources["process_faults"].get("case_count") != 6
        or sources["offline"].get("terminal_type") != "completed"
        or sources["offline"].get("terminal_count") != 1
        or sources["offline"].get("token_is_appcontainer") is not True
        or sources["offline"].get("capability_count") != 0
        or sources["offline"].get("network_before_blocked") is not True
        or sources["offline"].get("network_after_blocked") is not True
        or sources["offline"].get("residual_process_count") != 0
        or sources["parent_kill"].get("residual_process_count") != 0
        or sources["duplicate_request"].get("second_helper_started") is not False
        or sources["duplicate_request"].get("duplicate_error") != "MOSS_DUPLICATE_REQUEST"
        or sources["duplicate_request"].get("residual_process_count") != 0
        or sources["second_chunk_failure"].get("partial_success_returned") is not False
        or sources["second_chunk_failure"].get("retry_terminal") != "completed"
        or sources["second_chunk_failure"].get("residual_process_count") != 0
        or sources["qwen"].get("health_probe") != "pong"
        or sources["qwen"].get("model_generate_health") != "response_without_error"
        or sources["qwen"].get("residual_process_count") != 0
    ):
        raise AssertionError("a final acceptance invariant did not pass")


def command_finalize(arguments: argparse.Namespace) -> None:
    binding, binding_sha256 = load_acceptance_binding(arguments.binding)
    sources: dict[str, dict[str, Any]] = {}
    source_hashes: dict[str, dict[str, str]] = {}
    try:
        for name, stage in FINAL_GATE_STAGES.items():
            path = getattr(arguments, name).resolve(strict=True)
            verify_sha_sidecar(path)
            value = json.loads(path.read_text(encoding="utf-8"))
            validate_final_gate_source(name, value, binding, binding_sha256)
            sources[name] = value
            sidecar = path.with_name(path.name + ".sha256")
            source_hashes[name] = {
                "json_sha256": sha256_file(path),
                "sidecar_sha256": sha256_file(sidecar),
            }
        validate_final_gate_invariants(sources)
    except BaseException as error:
        value = {
            "schema_version": 1,
            "stage": "MOSS_V3_P2C_FINAL_VALIDATION",
            "status": "BLOCKED",
            "source_commit": binding["commit"],
            "code_validation": "BLOCKED",
            "real_gates": "BLOCKED",
            "traceability": "BLOCKED",
            "blocked_gates": ["P2D_ACCEPTANCE_SOURCE_INVALID"],
            "binding_sha256": binding_sha256,
        }
        write_evidence(arguments.private, arguments.public, value)
        raise AssertionError("P2-D acceptance is blocked") from error
    value = {
        "schema_version": 1,
        "stage": "MOSS_V3_P2C_FINAL_VALIDATION",
        "status": "PASS",
        "source_commit": binding["commit"],
        "code_validation": "PASS",
        "real_gates": {name: "PASS" for name in FINAL_GATE_STAGES if name != "code_validation"},
        "traceability": source_hashes,
        "blocked_gates": [],
        "binding_sha256": binding_sha256,
    }
    write_evidence(arguments.private, arguments.public, value)


PERFORMANCE_SUPPORT_SPECS = {
    "single_first": ("MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm"),
    "single_second": ("MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm"),
    "single_third": ("MOSS_V3_P2C_300S_SINGLE_CHILD", "three_hundred_pcm"),
    "four_eighty": ("MOSS_V3_P2C_480S_PERFORMANCE", "four_eighty_pcm"),
    "six_hundred": ("MOSS_V3_P2C_600S_PERFORMANCE", "six_hundred_pcm"),
}


def validate_final_acceptance_value(
    final: dict[str, Any], binding: dict[str, Any], binding_sha256: str
) -> None:
    assert_public_safe(final)
    require_all_finite_numbers(final)
    expected_real_gates = set(FINAL_GATE_STAGES) - {"code_validation"}
    if (
        final.get("schema_version") != 1
        or final.get("stage") != "MOSS_V3_P2C_FINAL_VALIDATION"
        or final.get("status") != "PASS"
        or final.get("source_commit") != binding["commit"]
        or final.get("binding_sha256") != binding_sha256
        or final.get("code_validation") != "PASS"
        or final.get("blocked_gates") != []
        or not isinstance(final.get("real_gates"), dict)
        or set(final["real_gates"]) != expected_real_gates
        or any(value != "PASS" for value in final["real_gates"].values())
        or not isinstance(final.get("traceability"), dict)
        or set(final["traceability"]) != set(FINAL_GATE_STAGES)
    ):
        raise AssertionError("final acceptance evidence is incomplete")
    for hashes in final["traceability"].values():
        if not isinstance(hashes, dict) or set(hashes) != {"json_sha256", "sidecar_sha256"}:
            raise AssertionError("final acceptance traceability is incomplete")


def validate_performance_support_records(
    records: list[dict[str, Any]],
    performance: dict[str, Any],
    binding: dict[str, Any],
    binding_sha256: str,
) -> None:
    evidence_hashes = performance.get("source_evidence_sha256")
    sidecar_hashes = performance.get("source_sidecar_sha256")
    if (
        not isinstance(evidence_hashes, dict)
        or set(evidence_hashes) != set(PERFORMANCE_SUPPORT_SPECS)
        or not isinstance(sidecar_hashes, dict)
        or set(sidecar_hashes) != set(PERFORMANCE_SUPPORT_SPECS)
    ):
        raise AssertionError("performance source traceability is incomplete")
    support_stages = {stage for stage, _ in PERFORMANCE_SUPPORT_SPECS.values()}
    support_paths = {
        record["path"] for record in records if record["value"].get("stage") in support_stages
    }
    matched_paths: set[Path] = set()
    matched_values: dict[str, dict[str, Any]] = {}
    for name, (expected_stage, fixture_name) in PERFORMANCE_SUPPORT_SPECS.items():
        matches = [
            record
            for record in records
            if record["json_sha256"] == evidence_hashes[name]
            and record["sidecar_sha256"] == sidecar_hashes[name]
        ]
        if len(matches) != 1:
            raise AssertionError("a performance source hash does not identify exactly one file")
        record = matches[0]
        if record["path"] in matched_paths:
            raise AssertionError("one performance source was reused for multiple frozen runs")
        value = validate_bound_source_value(
            record["value"], expected_stage, binding, binding_sha256, fixture_name
        )
        if expected_stage == "MOSS_V3_P2C_300S_SINGLE_CHILD":
            validate_helper_runs(value.get("helper_runs"), [0], [300_000])
            if (
                value.get("helper_process_id") != value["helper_runs"][0]["processId"]
                or value.get("residual_process_count") != 0
            ):
                raise AssertionError("a 300 second source has inconsistent process accounting")
        elif expected_stage == "MOSS_V3_P2C_480S_PERFORMANCE":
            validate_helper_runs(value.get("helper_runs"), [0, 300_000], [300_000, 180_000])
            if value.get("helper_process_id") != 0 or value.get("residual_process_count") != 0:
                raise AssertionError("the 480 second source has inconsistent process accounting")
        else:
            validate_helper_runs(value.get("helper_runs"), [0, 300_000], [300_000, 300_000])
            if value.get("helper_process_id") != 0 or value.get("residual_process_count") != 0:
                raise AssertionError("the 600 second source has inconsistent process accounting")
        matched_paths.add(record["path"])
        matched_values[name] = value
    if matched_paths != support_paths:
        raise AssertionError("performance support files are missing, duplicated, or unreferenced")
    single_runs = [matched_values[name]["helper_runs"][0] for name in ("single_first", "single_second", "single_third")]
    if (
        len({run["rawTextSha256"] for run in single_runs}) != 1
        or len({run["cleanTextSha256"] for run in single_runs}) != 1
    ):
        raise AssertionError("the three frozen 300 second runs are not repeatable")


def collect_evidence_records(directory: Path) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    records: list[dict[str, Any]] = []
    entries: list[dict[str, Any]] = []
    for path in sorted(directory.glob("*.json")):
        if path.name == "manifest.json":
            continue
        verify_sha_sidecar(path)
        value = json.loads(path.read_text(encoding="utf-8"))
        assert_public_safe(value)
        require_all_finite_numbers(value)
        sidecar = path.with_name(path.name + ".sha256")
        json_sha256 = sha256_file(path)
        records.append(
            {
                "path": path,
                "value": value,
                "json_sha256": json_sha256,
                "sidecar_sha256": sha256_file(sidecar),
            }
        )
        entries.append(
            {"file": path.name, "bytes": path.stat().st_size, "sha256": json_sha256}
        )
    return records, entries


def validate_public_directory_inventory(directory: Path, records: list[dict[str, Any]]) -> None:
    allowed_names = {"manifest.json", "manifest.json.sha256"}
    for record in records:
        name = record["path"].name
        allowed_names.add(name)
        allowed_names.add(name + ".sha256")
    for entry in directory.iterdir():
        if entry.is_symlink() or not entry.is_file() or entry.name not in allowed_names:
            raise AssertionError("public evidence directory contains an unexpected entry")


def validate_complete_evidence_set(records: list[dict[str, Any]]) -> None:
    expected_stage_counts: dict[str, int] = {
        "MOSS_V3_P2D_BINDING": 1,
        "MOSS_V3_P2C_FINAL_VALIDATION": 1,
        "MOSS_V3_P2C_300S_SINGLE_CHILD": 3,
        "MOSS_V3_P2C_480S_PERFORMANCE": 1,
        "MOSS_V3_P2C_600S_PERFORMANCE": 1,
    }
    expected_stage_counts.update({stage: 1 for stage in FINAL_GATE_STAGES.values()})
    actual_stage_counts: dict[str, int] = {}
    records_by_stage: dict[str, list[dict[str, Any]]] = {}
    for record in records:
        stage = record["value"]["stage"]
        actual_stage_counts[stage] = actual_stage_counts.get(stage, 0) + 1
        records_by_stage.setdefault(stage, []).append(record)
    if actual_stage_counts != expected_stage_counts:
        raise AssertionError("evidence stage inventory is incomplete or contains duplicates")

    binding_record = records_by_stage["MOSS_V3_P2D_BINDING"][0]
    binding = binding_record["value"]
    validate_binding(binding)
    binding_sha256 = binding_record["json_sha256"]
    final_record = records_by_stage["MOSS_V3_P2C_FINAL_VALIDATION"][0]
    final = final_record["value"]
    validate_final_acceptance_value(final, binding, binding_sha256)

    sources: dict[str, dict[str, Any]] = {}
    for name, stage in FINAL_GATE_STAGES.items():
        record = records_by_stage[stage][0]
        value = record["value"]
        validate_final_gate_source(name, value, binding, binding_sha256)
        expected_hashes = final["traceability"][name]
        if (
            record["json_sha256"] != expected_hashes["json_sha256"]
            or record["sidecar_sha256"] != expected_hashes["sidecar_sha256"]
        ):
            raise AssertionError("final traceability does not match the current gate file")
        sources[name] = value
    validate_final_gate_invariants(sources)
    validate_performance_support_records(
        records, sources["performance"], binding, binding_sha256
    )


def command_evidence_manifest(directory: Path, manifest_stage: str) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    manifest_path = directory / "manifest.json"
    try:
        records, entries = collect_evidence_records(directory)
        if manifest_stage == "MOSS_V3_P2C_PUBLIC_MANIFEST":
            validate_public_directory_inventory(directory, records)
        validate_complete_evidence_set(records)
        value = {
            "schema_version": 1,
            "stage": manifest_stage,
            "status": "PASS",
            "integrity_status": "PASS",
            "acceptance_status": "PASS",
            "files": entries,
        }
        assert_public_safe(value)
        atomic_json(manifest_path, value)
    except BaseException as error:
        blocked = {
            "schema_version": 1,
            "stage": manifest_stage,
            "status": "BLOCKED",
            "integrity_status": "FAIL",
            "acceptance_status": "BLOCKED",
            "files": [],
        }
        assert_public_safe(blocked)
        atomic_json(manifest_path, blocked)
        raise AssertionError("P2 evidence manifest is blocked") from error


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser()
    commands = result.add_subparsers(dest="command", required=True)
    faults = commands.add_parser("faults")
    faults.add_argument("--binding", type=Path, required=True)
    faults.add_argument("--helper", type=Path, required=True)
    faults.add_argument("--runtime", type=Path, required=True)
    faults.add_argument("--model", type=Path, required=True)
    faults.add_argument("--audio", type=Path, required=True)
    faults.add_argument("--truncation-evidence", type=Path, required=True)
    faults.add_argument("--private", type=Path, required=True)
    faults.add_argument("--public", type=Path, required=True)
    faults.set_defaults(function=command_faults)
    truncation = commands.add_parser("rust-truncation-gate")
    truncation.add_argument("--binding", type=Path, required=True)
    truncation.add_argument("--test-executable", type=Path, required=True)
    truncation.add_argument("--private", type=Path, required=True)
    truncation.add_argument("--public", type=Path, required=True)
    truncation.set_defaults(function=command_rust_truncation_gate)
    parent_kill = commands.add_parser("parent-kill")
    parent_kill.add_argument("--binding", type=Path, required=True)
    parent_kill.add_argument("--test-executable", type=Path, required=True)
    parent_kill.add_argument("--moss-helper", type=Path, required=True)
    parent_kill.add_argument("--private", type=Path, required=True)
    parent_kill.add_argument("--public", type=Path, required=True)
    parent_kill.set_defaults(function=command_parent_kill)
    qwen = commands.add_parser("qwen")
    qwen.add_argument("--binding", type=Path, required=True)
    qwen.add_argument("--helper", type=Path, required=True)
    qwen.add_argument("--moss-helper", type=Path, required=True)
    qwen.add_argument("--model", type=Path, required=True)
    qwen.add_argument("--model-bytes", type=int, required=True)
    qwen.add_argument("--handoff-evidence", type=Path, required=True)
    qwen.add_argument("--private", type=Path, required=True)
    qwen.add_argument("--public", type=Path, required=True)
    qwen.set_defaults(function=command_qwen)
    offline = commands.add_parser("offline")
    offline.add_argument("--binding", type=Path, required=True)
    offline.add_argument("--profile", required=True)
    offline.add_argument("--python", type=Path, required=True)
    offline.add_argument("--helper", type=Path, required=True)
    offline.add_argument("--runtime", type=Path, required=True)
    offline.add_argument("--model", type=Path, required=True)
    offline.add_argument("--audio", type=Path, required=True)
    offline.add_argument("--private", type=Path, required=True)
    offline.add_argument("--public", type=Path, required=True)
    offline.set_defaults(function=command_offline)
    performance = commands.add_parser("performance")
    performance.add_argument("--binding", type=Path, required=True)
    performance.add_argument("--single-first", type=Path, required=True)
    performance.add_argument("--single-second", type=Path, required=True)
    performance.add_argument("--single-third", type=Path, required=True)
    performance.add_argument("--four-eighty", type=Path, required=True)
    performance.add_argument("--six-hundred", type=Path, required=True)
    performance.add_argument("--fixture-sha256", required=True)
    performance.add_argument("--wer-start", required=True)
    performance.add_argument("--private", type=Path, required=True)
    performance.add_argument("--public", type=Path, required=True)
    performance.set_defaults(function=command_performance)
    code_validation = commands.add_parser("code-validation")
    code_validation.add_argument("--binding", type=Path, required=True)
    code_validation.add_argument("--repo", type=Path, required=True)
    code_validation.add_argument("--python", type=Path, required=True)
    code_validation.add_argument("--test-executable", type=Path, required=True)
    code_validation.add_argument("--private", type=Path, required=True)
    code_validation.add_argument("--public", type=Path, required=True)
    code_validation.set_defaults(function=command_code_validation)
    binding = commands.add_parser("binding")
    binding.add_argument("--repo", type=Path, required=True)
    binding.add_argument("--test-executable", type=Path, required=True)
    binding.add_argument("--moss-helper", type=Path, required=True)
    binding.add_argument("--qwen-helper", type=Path, required=True)
    binding.add_argument("--runtime-manifest", type=Path, required=True)
    binding.add_argument("--q8-model", type=Path, required=True)
    binding.add_argument("--qwen-model", type=Path, required=True)
    binding.add_argument("--short-pcm", type=Path, required=True)
    binding.add_argument("--three-hundred-pcm", type=Path, required=True)
    binding.add_argument("--four-eighty-pcm", type=Path, required=True)
    binding.add_argument("--six-hundred-pcm", type=Path, required=True)
    binding.add_argument("--private", type=Path, required=True)
    binding.add_argument("--public", type=Path, required=True)
    binding.set_defaults(function=command_binding)
    finalize = commands.add_parser("finalize")
    finalize.add_argument("--binding", type=Path, required=True)
    for gate_name in FINAL_GATE_STAGES:
        finalize.add_argument(f"--{gate_name.replace('_', '-')}", dest=gate_name, type=Path, required=True)
    finalize.add_argument("--private", type=Path, required=True)
    finalize.add_argument("--public", type=Path, required=True)
    finalize.set_defaults(function=command_finalize)
    manifest = commands.add_parser("manifest")
    manifest.add_argument("--public-dir", type=Path, required=True)
    manifest.set_defaults(function=command_manifest)
    private_manifest = commands.add_parser("private-manifest")
    private_manifest.add_argument("--private-dir", type=Path, required=True)
    private_manifest.set_defaults(function=command_private_manifest)
    publish_safe = commands.add_parser("publish-safe")
    publish_safe.add_argument("--source", type=Path, required=True)
    publish_safe.add_argument("--public", type=Path, required=True)
    publish_safe.set_defaults(function=command_publish_safe)
    return result


def main() -> int:
    arguments = parser().parse_args()
    arguments.function(arguments)
    return 0


if __name__ == "__main__":
    sys.exit(main())
