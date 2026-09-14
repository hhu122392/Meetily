#!/usr/bin/env python3
"""Create a read-only D-12 summary baseline diagnostic checkpoint.

This tool does not launch Meetily, open a user database, or generate a summary.
It only inspects explicitly named files, audits the checked-out source tree, and
records current Windows memory limits. Missing evidence stays missing.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import subprocess
import sys
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable, Sequence


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


SCHEMA_VERSION = 1
RESERVE_BYTES = 2 * 1024 * 1024 * 1024
TIMED_ENDPOINT_ID = "click_to_page_display_complete"
EXPECTED_WINDOW_DURATION_MS = 226_440
EXPECTED_WINDOW_BYTES = 7_246_124
EXPECTED_WINDOW_SHA256 = "5128D52F41A6FA58BBD9387C6C68D09E076E9E782C8C13B8BF0CF5BB8655AD22"
EXPECTED_Q00_MANIFEST_BYTES = 2_854
EXPECTED_Q00_MANIFEST_SHA256 = "1C9639F4E0AF6FA6C11BA3F5DC4674F6B227FC47A1323417E5D6846F5DDDCEB3"
EXPECTED_NEGATIVE_TRUTH_BYTES = 1_605
EXPECTED_NEGATIVE_TRUTH_SHA256 = "41A3FA60A6A2F6775C14E58D9E37E58480D9BA64D5541F4CF5412FA3697D83F7"
EXPECTED_SUMMARY_MODEL_BYTES = 1_280_835_840
EXPECTED_SUMMARY_MODEL_SHA256 = "AAF42C8B7C3CAB2BF3D69C355048D4A0EE9973D48F16C731C0520EE914699223"
ISOLATION_MARKER = ".meetily-d12-isolated.json"
ISOLATION_PURPOSE = "D12_SUMMARY_BASELINE"
PROTECTED_PATH_FRAGMENT = "com.meetily.ai"


SOURCE_FILES = (
    "frontend/src/hooks/meeting-details/useSummaryGeneration.ts",
    "frontend/src/components/Sidebar/SidebarProvider.tsx",
    "frontend/src-tauri/src/summary/commands.rs",
    "frontend/src-tauri/src/summary/processor.rs",
    "frontend/src-tauri/src/summary/service.rs",
    "frontend/src-tauri/src/summary/summary_engine/client.rs",
    "frontend/src-tauri/src/summary/summary_engine/sidecar.rs",
    "frontend/src-tauri/src/moss_helper/windows_job.rs",
    "frontend/src-tauri/src/database/repositories/summary.rs",
    "llama-helper/src/main.rs",
)


@dataclass(frozen=True)
class Marker:
    file: str
    contains: str


STAGES = (
    (
        "wait_for_transcription",
        Marker("frontend/src/hooks/meeting-details/useSummaryGeneration.ts", "fetchAllTranscripts(meeting.id)"),
        "The click path fetches the current transcript, but there is no generation-correlated wait-stage start/end timestamp.",
    ),
    (
        "prepare_input",
        Marker("frontend/src-tauri/src/summary/commands.rs", "resolve_active_summary_input(&pool, &m_id)"),
        "Source resolution, template capture, and transcript persistence exist, but they share no explicit prepare-input timer.",
    ),
    (
        "load_model",
        Marker("llama-helper/src/main.rs", "Model loaded successfully"),
        "The helper writes a human stderr line after loading; it does not emit the required structured stdout model_loaded event.",
    ),
    (
        "chunk_summaries",
        Marker("frontend/src-tauri/src/summary/processor.rs", "Processing chunk {}/{}"),
        "Chunk calls have progress log text but no per-chunk monotonic start/end timestamps tied to generation_id.",
    ),
    (
        "combine",
        Marker("frontend/src-tauri/src/summary/processor.rs", "Combining {} chunk summaries into cohesive summary"),
        "The combine call exists but has no independent start/end timing point.",
    ),
    (
        "final_template",
        Marker("frontend/src-tauri/src/summary/processor.rs", "Generating final markdown report with template"),
        "The final-template call exists but has no independent start/end timing point.",
    ),
    (
        "translation",
        Marker("frontend/src-tauri/src/summary/processor.rs", "translate_markdown("),
        "Translation is a separate call in control flow but has no independent start/end timing point.",
    ),
    (
        "save",
        Marker("frontend/src-tauri/src/summary/service.rs", "persist_completed_summary("),
        "Persistence exists, but the only background duration is calculated before validation and persistence.",
    ),
    (
        "page_display_complete",
        Marker("frontend/src/hooks/meeting-details/useSummaryGeneration.ts", "setAiSummary({"),
        "The page updates after five-second polling, but click_at/page_display_complete_at and the displayed body hash are not recorded.",
    ),
)
REQUIRED_STAGE_IDS = tuple(item[0] for item in STAGES)
REQUIRED_SHA256_FIELDS = (
    "input_text_sha256",
    "page_displayed_body_sha256",
    "database_completed_body_sha256",
    "load_timeline_sha256",
    "build_wrapper_sha256",
    "cargo_lock_sha256",
    "pnpm_lock_sha256",
)
REQUIRED_EVENT_COUNT_FIELDS = (
    "helper_process_start_count",
    "model_loaded_event_count",
    "cleanup_entry_count",
    "graceful_shutdown_request_count",
    "job_confirm_zero_count",
    "helper_process_count_after_5s",
)


def now_iso() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def read_lines(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines()


def locate_marker(repo: Path, marker: Marker) -> dict[str, Any]:
    path = repo / marker.file
    if not path.is_file():
        return {
            "file": marker.file,
            "line": None,
            "contains": marker.contains,
            "found": False,
            "excerpt": None,
        }
    for number, line in enumerate(read_lines(path), start=1):
        if marker.contains in line:
            return {
                "file": marker.file,
                "line": number,
                "contains": marker.contains,
                "found": True,
                "excerpt": line.strip(),
            }
    return {
        "file": marker.file,
        "line": None,
        "contains": marker.contains,
        "found": False,
        "excerpt": None,
    }


def source_contains(repo: Path, files: Iterable[str], needles: Iterable[str]) -> bool:
    lowered_needles = tuple(needle.lower() for needle in needles)
    for relative in files:
        path = repo / relative
        if not path.is_file():
            continue
        contents = path.read_text(encoding="utf-8").lower()
        if any(needle in contents for needle in lowered_needles):
            return True
    return False


def source_file_inventory(repo: Path) -> list[dict[str, Any]]:
    inventory: list[dict[str, Any]] = []
    for relative in SOURCE_FILES:
        path = repo / relative
        inventory.append(
            {
                "path": relative,
                "exists": path.is_file(),
                "bytes": path.stat().st_size if path.is_file() else None,
                "sha256": sha256_file(path) if path.is_file() else None,
            }
        )
    return inventory


def audit_source(repo: Path, commit: str, branch: str) -> dict[str, Any]:
    stages: list[dict[str, Any]] = []
    for stage_id, marker, finding in STAGES:
        flow = locate_marker(repo, marker)
        stages.append(
            {
                "stage_id": stage_id,
                "flow_present": flow["found"],
                "flow_evidence": flow,
                "independent_monotonic_timing_point_present": False,
                "generation_correlated_timing_event_present": False,
                "status": "MISSING_TIMING_POINT" if flow["found"] else "FLOW_NOT_FOUND",
                "finding": finding,
            }
        )

    commands = "frontend/src-tauri/src/summary/commands.rs"
    service = "frontend/src-tauri/src/summary/service.rs"
    repository = "frontend/src-tauri/src/database/repositories/summary.rs"
    frontend = "frontend/src/hooks/meeting-details/useSummaryGeneration.ts"
    sidecar = "frontend/src-tauri/src/summary/summary_engine/sidecar.rs"
    job = "frontend/src-tauri/src/moss_helper/windows_job.rs"
    helper = "llama-helper/src/main.rs"

    identity_and_hashes = {
        "generation_id": {
            "status": "PRESENT",
            "evidence": locate_marker(repo, Marker(commands, 'let generation_id = format!("gen_{}"')),
        },
        "input_transcript_sha256": {
            "status": "PRESENT",
            "evidence": locate_marker(repo, Marker(commands, "transcript_sha256: &source_binding.transcript_sha256")),
        },
        "database_completed_body_sha256": {
            "status": "MISSING",
            "reason": "Completed summary rows persist JSON/Markdown and timestamps, but no completed body SHA-256 field is written.",
            "hash_field_found": source_contains(repo, (service, repository), ("markdown_sha256", "summary_body_sha256", "result_sha256")),
        },
        "page_displayed_body_sha256": {
            "status": "MISSING",
            "reason": "The UI stores the returned body in React state without hashing the displayed body.",
            "hash_field_found": source_contains(repo, (frontend,), ("page_body_sha256", "displayed_summary_sha256")),
        },
    }

    process_events = {
        "job_pid_inventory": {
            "status": "PRIMITIVE_ONLY",
            "reason": "JobControl can return process_id/process_ids, but the production summary path does not emit a generation-correlated PID inventory.",
            "primitive_evidence": locate_marker(repo, Marker(job, "pub fn process_ids(&self)")),
        },
        "model_loaded": {
            "status": "MISSING_STRUCTURED_EVENT",
            "reason": "The helper writes a human stderr message, while the sidecar drains stderr as log text. No stdout model_loaded event is parsed.",
            "helper_stderr_evidence": locate_marker(repo, Marker(helper, "Model loaded successfully")),
            "stderr_drain_evidence": locate_marker(repo, Marker(sidecar, "llama-helper stderr:")),
        },
        "cleanup_entry": {
            "status": "CODE_PATH_ONLY",
            "reason": "shutdown() is called, but there is no generation-correlated cleanup_entry_count event.",
            "evidence": locate_marker(repo, Marker(sidecar, "pub async fn shutdown(&self)")),
        },
        "graceful_shutdown_request": {
            "status": "CODE_PATH_ONLY",
            "reason": "A shutdown JSON request is attempted, but no generation-correlated graceful_shutdown_request_count event is recorded.",
            "evidence": locate_marker(repo, Marker(sidecar, 'serde_json::json!({"type": "shutdown"})')),
        },
        "job_confirm_zero": {
            "status": "ENFORCED_BUT_NOT_RECORDED",
            "reason": "JobControl::confirm_zero is enforced, but there is no generation-correlated job_confirm_zero_count event.",
            "evidence": locate_marker(repo, Marker(sidecar, ".confirm_zero(Duration::from_secs(3))")),
        },
    }

    total_timer = {
        "status": "PARTIAL_WRONG_ENDPOINT",
        "start": locate_marker(repo, Marker(service, "let start_time = Instant::now();")),
        "end": locate_marker(repo, Marker(service, "let duration = start_time.elapsed().as_secs_f64();")),
        "reason": "This timer starts inside the background worker and stops before validation and database persistence; it excludes click/preflight/page polling and cannot prove click-to-page completion.",
    }

    return {
        "schema_version": SCHEMA_VERSION,
        "stage": "D12_STATIC_SUMMARY_TIMING_AUDIT",
        "generated_at": now_iso(),
        "source": {"commit": commit, "branch": branch},
        "production_logic_modified": False,
        "required_timed_endpoint_id": TIMED_ENDPOINT_ID,
        "source_files": source_file_inventory(repo),
        "stage_audit": stages,
        "identity_and_hash_audit": identity_and_hashes,
        "helper_process_event_audit": process_events,
        "existing_total_timer": total_timer,
        "conclusion": {
            "all_required_stage_timing_points_present": False,
            "page_and_database_body_hash_pair_present": False,
            "generation_correlated_helper_lifecycle_events_present": False,
            "full_run_load_timeline_present": False,
            "status": "BLOCKED",
        },
    }


class PerformanceInformation(ctypes.Structure):
    _fields_ = [
        ("cb", ctypes.c_uint32),
        ("CommitTotal", ctypes.c_size_t),
        ("CommitLimit", ctypes.c_size_t),
        ("CommitPeak", ctypes.c_size_t),
        ("PhysicalTotal", ctypes.c_size_t),
        ("PhysicalAvailable", ctypes.c_size_t),
        ("SystemCache", ctypes.c_size_t),
        ("KernelTotal", ctypes.c_size_t),
        ("KernelPaged", ctypes.c_size_t),
        ("KernelNonpaged", ctypes.c_size_t),
        ("PageSize", ctypes.c_size_t),
        ("HandleCount", ctypes.c_uint32),
        ("ProcessCount", ctypes.c_uint32),
        ("ThreadCount", ctypes.c_uint32),
    ]


def windows_memory_snapshot() -> dict[str, Any]:
    if os.name != "nt":
        return {
            "status": "UNAVAILABLE",
            "reason": "GetPerformanceInfo is only available on Windows.",
            "physical_memory_bytes": None,
            "commit_limit_bytes": None,
        }
    info = PerformanceInformation()
    info.cb = ctypes.sizeof(info)
    get_performance_info = ctypes.windll.psapi.GetPerformanceInfo
    get_performance_info.argtypes = [ctypes.POINTER(PerformanceInformation), ctypes.c_uint32]
    get_performance_info.restype = ctypes.c_int
    if not get_performance_info(ctypes.byref(info), info.cb):
        raise ctypes.WinError()
    page_size = int(info.PageSize)
    return {
        "status": "CAPTURED_FROM_WINDOWS_GETPERFORMANCEINFO",
        "captured_at": now_iso(),
        "page_size_bytes": page_size,
        "physical_memory_bytes": int(info.PhysicalTotal) * page_size,
        "physical_available_bytes": int(info.PhysicalAvailable) * page_size,
        "commit_total_bytes_at_capture": int(info.CommitTotal) * page_size,
        "commit_limit_bytes": int(info.CommitLimit) * page_size,
        "system_commit_peak_bytes_at_capture": int(info.CommitPeak) * page_size,
        "process_count_at_capture": int(info.ProcessCount),
        "thread_count_at_capture": int(info.ThreadCount),
    }


def calculate_peak_commit_ceiling(
    physical_memory_bytes: int | None,
    commit_limit_bytes: int | None,
    baseline_non_moss_commit_peak_bytes: int | None,
) -> int | None:
    if (
        physical_memory_bytes is None
        or commit_limit_bytes is None
        or baseline_non_moss_commit_peak_bytes is None
    ):
        return None
    ceiling = min(
        physical_memory_bytes * 70 // 100,
        commit_limit_bytes - baseline_non_moss_commit_peak_bytes - RESERVE_BYTES,
    )
    return ceiling if ceiling > 0 else None


def asset_record(
    *,
    role: str,
    path: Path | None,
    expected_bytes: int | None = None,
    expected_sha256: str | None = None,
    required: bool = True,
    note: str,
) -> dict[str, Any]:
    exists = path is not None and path.is_file()
    actual_bytes = path.stat().st_size if exists and path is not None else None
    actual_sha256 = sha256_file(path) if exists and path is not None else None
    bytes_match = (
        actual_bytes == expected_bytes if exists and expected_bytes is not None else None
    )
    sha256_match = (
        actual_sha256 == expected_sha256.upper()
        if exists and expected_sha256 is not None and actual_sha256 is not None
        else None
    )
    checks_pass = bool(
        exists
        and (bytes_match is not False)
        and (sha256_match is not False)
        and actual_sha256
    )
    return {
        "role": role,
        "path": str(path.resolve()) if path is not None else None,
        "required": required,
        "exists": bool(exists),
        "expected_bytes": expected_bytes,
        "actual_bytes": actual_bytes,
        "bytes_match": bytes_match,
        "expected_sha256": expected_sha256,
        "actual_sha256": actual_sha256,
        "sha256_match": sha256_match,
        "freeze_status": "FROZEN_VERIFIED" if checks_pass else "PENDING",
        "note": note,
    }


def is_sha256(value: Any) -> bool:
    return isinstance(value, str) and len(value) == 64 and all(
        character in "0123456789abcdefABCDEF" for character in value
    )


def load_baseline_run(path: Path) -> dict[str, Any]:
    result: dict[str, Any] = {
        "path": str(path.resolve()),
        "exists": path.is_file(),
        "sha256": sha256_file(path) if path.is_file() else None,
        "valid": False,
        "errors": [],
        "baseline_non_moss_commit_peak_bytes": None,
    }
    if not path.is_file():
        result["errors"].append("file_missing")
        return result
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        result["errors"].append(f"invalid_json:{error}")
        return result
    required_keys = (
        "baseline_snapshot_id",
        "generation_id",
        "input_text_sha256",
        "model_id",
        "language",
        "template_version",
        "hardware_id",
        "timed_endpoint_id",
        "click_at",
        "processing_visible_at",
        "database_save_complete_at",
        "page_display_complete_at",
        "page_displayed_body_sha256",
        "database_completed_body_sha256",
        "load_timeline_path",
        "load_timeline_sha256",
        "build_wrapper_sha256",
        "release_build_parameters",
        "cargo_lock_sha256",
        "pnpm_lock_sha256",
        "job_pids",
        *REQUIRED_EVENT_COUNT_FIELDS,
        "stage_timings",
        "chunk_timings",
        "baseline_non_moss_commit_peak_bytes",
        "outcome",
    )
    for key in required_keys:
        if data.get(key) in (None, ""):
            result["errors"].append(f"missing:{key}")
    if data.get("timed_endpoint_id") != TIMED_ENDPOINT_ID:
        result["errors"].append("wrong:timed_endpoint_id")
    if data.get("outcome") != "completed":
        result["errors"].append("wrong:outcome")

    for key in REQUIRED_SHA256_FIELDS:
        if not is_sha256(data.get(key)):
            result["errors"].append(f"invalid:{key}")
    page_hash = data.get("page_displayed_body_sha256")
    database_hash = data.get("database_completed_body_sha256")
    if is_sha256(page_hash) and is_sha256(database_hash) and page_hash.upper() != database_hash.upper():
        result["errors"].append("mismatch:page_and_database_body_sha256")

    timeline_value = data.get("load_timeline_path")
    timeline_path = None
    if isinstance(timeline_value, str) and timeline_value:
        timeline_path = Path(timeline_value)
        if not timeline_path.is_absolute():
            timeline_path = path.parent / timeline_path
    if timeline_path is None or not timeline_path.is_file():
        result["errors"].append("missing:load_timeline_file")
    elif is_sha256(data.get("load_timeline_sha256")):
        if sha256_file(timeline_path) != str(data["load_timeline_sha256"]).upper():
            result["errors"].append("mismatch:load_timeline_sha256")

    for key in REQUIRED_EVENT_COUNT_FIELDS:
        count = data.get(key)
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            result["errors"].append(f"invalid:{key}")
    if data.get("helper_process_count_after_5s") != 0:
        result["errors"].append("wrong:helper_process_count_after_5s")

    job_pids = data.get("job_pids")
    if not isinstance(job_pids, list) or not job_pids or any(
        not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0 for pid in job_pids
    ):
        result["errors"].append("invalid:job_pids")
    elif len(set(job_pids)) != len(job_pids):
        result["errors"].append("invalid:job_pids_not_unique")
    elif data.get("helper_process_start_count") != len(job_pids):
        result["errors"].append("mismatch:helper_process_start_count_and_job_pids")
    if (
        isinstance(data.get("helper_process_start_count"), int)
        and data.get("model_loaded_event_count") != data.get("helper_process_start_count")
    ):
        result["errors"].append("mismatch:model_loaded_event_count_and_process_start_count")

    generation_id = data.get("generation_id")
    stage_timings = data.get("stage_timings")
    if not isinstance(stage_timings, dict):
        result["errors"].append("invalid:stage_timings")
    else:
        for stage_id in REQUIRED_STAGE_IDS:
            timing = stage_timings.get(stage_id)
            if not isinstance(timing, dict):
                result["errors"].append(f"missing:stage_timings.{stage_id}")
                continue
            start = timing.get("start_monotonic_ns")
            end = timing.get("end_monotonic_ns")
            duration = timing.get("duration_ns")
            if timing.get("generation_id") != generation_id:
                result["errors"].append(f"mismatch:stage_timings.{stage_id}.generation_id")
            if (
                not isinstance(start, int)
                or isinstance(start, bool)
                or not isinstance(end, int)
                or isinstance(end, bool)
                or not isinstance(duration, int)
                or isinstance(duration, bool)
                or end < start
                or duration != end - start
            ):
                result["errors"].append(f"invalid:stage_timings.{stage_id}.monotonic_range")
        for stage_id in ("combine", "final_template", "translation"):
            timing = stage_timings.get(stage_id)
            if isinstance(timing, dict):
                call_count = timing.get("call_count")
                if not isinstance(call_count, int) or isinstance(call_count, bool) or call_count < 0:
                    result["errors"].append(f"invalid:stage_timings.{stage_id}.call_count")

    chunk_timings = data.get("chunk_timings")
    if not isinstance(chunk_timings, list) or not chunk_timings:
        result["errors"].append("invalid:chunk_timings")
    else:
        for index, chunk in enumerate(chunk_timings):
            if not isinstance(chunk, dict):
                result["errors"].append(f"invalid:chunk_timings.{index}")
                continue
            start = chunk.get("start_monotonic_ns")
            end = chunk.get("end_monotonic_ns")
            duration = chunk.get("duration_ns")
            output_bytes = chunk.get("output_bytes")
            if chunk.get("generation_id") != generation_id:
                result["errors"].append(f"mismatch:chunk_timings.{index}.generation_id")
            if (
                not isinstance(start, int)
                or isinstance(start, bool)
                or not isinstance(end, int)
                or isinstance(end, bool)
                or not isinstance(duration, int)
                or isinstance(duration, bool)
                or end < start
                or duration != end - start
            ):
                result["errors"].append(f"invalid:chunk_timings.{index}.monotonic_range")
            if not isinstance(output_bytes, int) or isinstance(output_bytes, bool) or output_bytes < 0:
                result["errors"].append(f"invalid:chunk_timings.{index}.output_bytes")

    value = data.get("baseline_non_moss_commit_peak_bytes")
    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
        result["baseline_non_moss_commit_peak_bytes"] = value
    else:
        result["errors"].append("invalid:baseline_non_moss_commit_peak_bytes")
    result["valid"] = not result["errors"]
    return result


def git_value(repo: Path, *arguments: str) -> str:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    return completed.stdout.strip()


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def build_manifest(
    repo: Path,
    commit: str,
    branch: str,
    args: argparse.Namespace,
    memory: dict[str, Any],
) -> dict[str, Any]:
    plan_root = repo / "target/release/docs/方案/MOSS功能修复计划-20260902"
    assets = [
        asset_record(
            role="q00_frozen_truth_contract",
            path=plan_root / "Q00-FROZEN-TRUTH-MANIFEST.json",
            expected_bytes=EXPECTED_Q00_MANIFEST_BYTES,
            expected_sha256=EXPECTED_Q00_MANIFEST_SHA256,
            note="Existing tracked contract for the 226.440-second window; this is not the audio bytes.",
        ),
        asset_record(
            role="q00_negative_truth_contract",
            path=plan_root / "Q00-NEGATIVE-TRUTH.json",
            expected_bytes=EXPECTED_NEGATIVE_TRUTH_BYTES,
            expected_sha256=EXPECTED_NEGATIVE_TRUTH_SHA256,
            note="Existing tracked Q-00 negative truth contract; it is not the D-12 fact rule file.",
        ),
        asset_record(
            role="fixed_226_440_second_window_audio",
            path=args.fixed_window_audio,
            expected_bytes=EXPECTED_WINDOW_BYTES,
            expected_sha256=EXPECTED_WINDOW_SHA256,
            note="Required actual WAV. No path means only the tracked hash contract was found, so the input is not frozen here.",
        ),
        asset_record(
            role="fixed_summary_input_transcript",
            path=args.summary_input,
            note="Required exact transcript text used by all three summary baseline runs.",
        ),
        asset_record(
            role="summary_fact_list",
            path=args.fact_list,
            note="Required private fact list with count metadata; no substitute is inferred from Q-00 truth files.",
        ),
        asset_record(
            role="ab_fact_judgement_rule",
            path=args.fact_rule,
            note="Required D-12 A/B fact judgement rule file.",
        ),
        asset_record(
            role="ab_language_judgement_rule",
            path=args.language_rule,
            note="Required D-12 A/B language judgement rule file with detector/version/threshold/confidence contract.",
        ),
        asset_record(
            role="qwen_2b_summary_model",
            path=args.summary_model,
            expected_bytes=EXPECTED_SUMMARY_MODEL_BYTES,
            expected_sha256=EXPECTED_SUMMARY_MODEL_SHA256,
            note="The old E-02 public baseline contains this expected identity, but D-12 requires the actual current file to be rechecked.",
        ),
        asset_record(
            role="b01_build_wrapper",
            path=repo / "scripts/qa/build-moss-functional-candidate.ps1",
            note="Existing build wrapper bytes hashed at the D-12 checkpoint.",
        ),
        asset_record(
            role="cargo_dependency_lock",
            path=repo / "Cargo.lock",
            note="Existing Rust dependency lock bytes hashed at the D-12 checkpoint.",
        ),
        asset_record(
            role="pnpm_dependency_lock",
            path=repo / "frontend/pnpm-lock.yaml",
            note="Existing frontend dependency lock bytes hashed at the D-12 checkpoint.",
        ),
    ]

    runs = [load_baseline_run(path) for path in args.baseline_run]
    valid_runs = [run for run in runs if run["valid"]]
    snapshot_ids: set[str] = set()
    for run, path in zip(runs, args.baseline_run):
        if run["valid"]:
            data = json.loads(path.read_text(encoding="utf-8"))
            snapshot_ids.add(str(data["baseline_snapshot_id"]))
    three_runs_same_snapshot = len(valid_runs) == 3 and len(snapshot_ids) == 1
    baseline_peak = (
        max(int(run["baseline_non_moss_commit_peak_bytes"]) for run in valid_runs)
        if three_runs_same_snapshot
        else None
    )
    peak_ceiling = calculate_peak_commit_ceiling(
        memory.get("physical_memory_bytes"),
        memory.get("commit_limit_bytes"),
        baseline_peak,
    )

    required_assets_ready = all(
        asset["freeze_status"] == "FROZEN_VERIFIED"
        for asset in assets
        if asset["required"]
    )
    blockers: list[str] = []
    for asset in assets:
        if asset["required"] and asset["freeze_status"] != "FROZEN_VERIFIED":
            blockers.append(f"asset_pending:{asset['role']}")
    if not three_runs_same_snapshot:
        blockers.append("three_valid_real_baseline_runs_with_one_baseline_snapshot_id_missing")
    if baseline_peak is None:
        blockers.append("baseline_non_moss_commit_peak_bytes_missing")
    if peak_ceiling is None:
        blockers.append("peak_commit_ceiling_bytes_not_computable")

    return {
        "schema_version": SCHEMA_VERSION,
        "stage": "D12_FROZEN_ASSETS_MANIFEST_DRAFT",
        "generated_at": now_iso(),
        "manifest_state": "DRAFT_BLOCKED" if blockers else "FROZEN",
        "source": {
            "starting_branch": "codex/d10a-evidence-20260906",
            "starting_commit": "4f4737575fff220ed92c2ed8f4ac56994749733e",
            "checkpoint_branch": branch,
            "checkpoint_commit_at_capture": commit,
        },
        "timing_contract": {
            "timed_endpoint_id": TIMED_ENDPOINT_ID,
            "fixed_window_duration_ms": EXPECTED_WINDOW_DURATION_MS,
            "required_real_run_count": 3,
            "valid_real_run_count": len(valid_runs),
            "three_runs_share_one_baseline_snapshot_id": three_runs_same_snapshot,
        },
        "assets": assets,
        "all_required_assets_frozen": required_assets_ready,
        "baseline_runs": runs,
        "machine_memory": {
            "capture_status": memory.get("status"),
            "physical_memory_bytes": memory.get("physical_memory_bytes"),
            "commit_limit_bytes": memory.get("commit_limit_bytes"),
            "baseline_non_moss_commit_peak_bytes": baseline_peak,
            "system_reserve_bytes": RESERVE_BYTES,
            "fixed_formula": "min(floor(physical_memory_bytes * 0.70), commit_limit_bytes - baseline_non_moss_commit_peak_bytes - 2147483648)",
            "peak_commit_ceiling_bytes": peak_ceiling,
            "formula_status": "FROZEN_COMPUTED" if peak_ceiling is not None else "BLOCKED_INPUT_MISSING",
            "rule": "Never derive the ceiling from a D-12/V-12 measured MOSS peak.",
        },
        "blocking_reasons": blockers,
        "status": "BLOCKED" if blockers else "PASS",
    }


def build_report(manifest: dict[str, Any], source_audit: dict[str, Any]) -> str:
    memory = manifest["machine_memory"]
    frozen_assets = [
        item["role"] for item in manifest["assets"] if item["freeze_status"] == "FROZEN_VERIFIED"
    ]
    pending_assets = [
        item["role"] for item in manifest["assets"] if item["freeze_status"] != "FROZEN_VERIFIED"
    ]
    missing_timing = [
        item["stage_id"]
        for item in source_audit["stage_audit"]
        if not item["independent_monotonic_timing_point_present"]
    ]
    return f"""# D-12 摘要修改前诊断检查点

> 生成时间：`{manifest['generated_at']}`
> 起点：`{manifest['source']['starting_branch']}` / `{manifest['source']['starting_commit']}`
> 当前状态：`BLOCKED`
> 边界：只做 D-12 静态审计和资产清点；没有启动 Meetily、没有运行摘要、没有修改生产摘要逻辑，也没有进入 I-12。

## 结论

当前代码不能产出 D-12 要求的真实分阶段基线。等待转写、准备输入、加载模型、分块小结、合并、最终模板、翻译、保存和页面完成这 9 个阶段都没有独立、单调时钟、带 `generation_id` 的开始/结束事件。现有总计时从后台任务开始，到模型流程返回时结束；它排除了点击前后的准备、保存和页面五秒轮询，不能代替固定的“点击到页面显示完成”端点。

`generation_id` 和输入转写 SHA-256 已存在。页面正文 SHA-256、数据库完成正文 SHA-256、本次 Job PID 清单、结构化 `model_loaded`、清理入口、优雅关闭请求、Job 清零计数以及整段 CPU/GPU/内存时间线均不能从当前生产证据机械取得。

## 已核对并可冻结的现有字节资产

{chr(10).join(f'- `{role}`' for role in frozen_assets)}

这些条目都由脚本重新读取实际文件、计算字节数和 SHA-256。Q-00 清单只证明固定窗口合同存在，不等于 226.440 秒 WAV 和摘要输入正文已经在本检查点中存在。

## 阻断项

{chr(10).join(f'- `{role}`' for role in pending_assets)}
- 三次真实无缓存摘要运行：`0/3`。
- 缺少的独立计时点：`{', '.join(missing_timing)}`。
- A/B 事实规则和语言规则文件没有提供，因此没有生成、猜测或冻结规则散列。

## 峰值提交内存公式

- `physical_memory_bytes={memory['physical_memory_bytes']}`
- `commit_limit_bytes={memory['commit_limit_bytes']}`
- `baseline_non_moss_commit_peak_bytes=null`
- `system_reserve_bytes={memory['system_reserve_bytes']}`
- 固定公式：`{memory['fixed_formula']}`
- `peak_commit_ceiling_bytes=null`

物理内存和提交上限来自本机 Windows `GetPerformanceInfo`。因为缺少同一 `baseline_snapshot_id` 的三次真实运行，非 MOSS 基线提交峰值没有数值，所以上限不能计算；没有使用本次实测峰值倒推。

## D-12 状态

D-12 保持 `BLOCKED`。本检查点只把真实存在的合同、构建包装器和锁文件登记为已核对资产；缺失的输入、规则、模型、三次运行和阶段事件全部保持 `PENDING/BLOCKED`，没有补造数值。
"""


def parse_path(value: str | None) -> Path | None:
    return Path(value).resolve() if value else None


def validate_isolated_asset_boundary(
    asset_root: Path | None,
    output_dir: Path,
    input_paths: Sequence[Path | None],
) -> dict[str, Any] | None:
    supplied_paths = [path.resolve() for path in input_paths if path is not None]
    if not supplied_paths:
        return None
    if asset_root is None:
        raise ValueError(
            "--asset-root is required when any private asset or baseline run is supplied"
        )

    root = asset_root.resolve()
    protected = PROTECTED_PATH_FRAGMENT.casefold()
    paths_to_check = [root, output_dir.resolve(), *supplied_paths]
    if any(protected in str(path).casefold() for path in paths_to_check):
        raise ValueError("protected com.meetily.ai paths are forbidden")

    marker_path = root / ISOLATION_MARKER
    if not marker_path.is_file():
        raise ValueError(f"isolation marker missing: {marker_path}")
    try:
        marker_path.resolve().relative_to(root)
    except ValueError as error:
        raise ValueError("isolation marker escapes isolated asset root") from error
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    if marker.get("schema_version") != 1 or marker.get("purpose") != ISOLATION_PURPOSE:
        raise ValueError("isolation marker schema_version or purpose is invalid")
    identity = marker.get("isolated_data_identity")
    if not isinstance(identity, str) or not identity.strip():
        raise ValueError("isolation marker isolated_data_identity is missing")

    for path in [output_dir.resolve(), *supplied_paths]:
        try:
            path.relative_to(root)
        except ValueError as error:
            raise ValueError(f"path escapes isolated asset root: {path}") from error
    return {
        "asset_root": str(root),
        "marker": str(marker_path),
        "isolated_data_identity": identity,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--asset-root", type=parse_path)
    parser.add_argument("--fixed-window-audio", type=parse_path)
    parser.add_argument("--summary-input", type=parse_path)
    parser.add_argument("--fact-list", type=parse_path)
    parser.add_argument("--fact-rule", type=parse_path)
    parser.add_argument("--language-rule", type=parse_path)
    parser.add_argument("--summary-model", type=parse_path)
    parser.add_argument("--baseline-run", type=Path, action="append", default=[])
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo = args.repo.resolve()
    output_dir = args.output_dir.resolve()
    supplied_inputs = [
        args.fixed_window_audio,
        args.summary_input,
        args.fact_list,
        args.fact_rule,
        args.language_rule,
        args.summary_model,
        *args.baseline_run,
    ]
    try:
        isolation = validate_isolated_asset_boundary(
            args.asset_root, output_dir, supplied_inputs
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"D-12 isolation boundary rejected the request: {error}", file=sys.stderr)
        return 2
    if not (repo / ".git").exists():
        # Worktrees use a .git file, while ordinary clones use a directory.
        if not (repo / ".git").is_file():
            print(f"Repository marker missing: {repo}", file=sys.stderr)
            return 2
    if output_dir.exists() and any(output_dir.iterdir()):
        print(f"Refusing to overwrite non-empty evidence directory: {output_dir}", file=sys.stderr)
        return 2
    output_dir.mkdir(parents=True, exist_ok=True)

    commit = git_value(repo, "rev-parse", "HEAD")
    branch = git_value(repo, "branch", "--show-current") or "DETACHED"
    memory = windows_memory_snapshot()
    source_audit = audit_source(repo, commit, branch)
    manifest = build_manifest(repo, commit, branch, args, memory)
    manifest["isolation_boundary"] = isolation

    write_json(output_dir / "machine-memory.json", memory)
    write_json(output_dir / "source-timing-audit.json", source_audit)
    write_json(output_dir / "frozen_assets_manifest.draft.json", manifest)
    (output_dir / "D12-diagnostic-report.md").write_text(
        build_report(manifest, source_audit), encoding="utf-8"
    )

    evidence_files = []
    for path in sorted(output_dir.iterdir(), key=lambda item: item.name):
        if path.is_file():
            evidence_files.append(
                {"name": path.name, "bytes": path.stat().st_size, "sha256": sha256_file(path)}
            )
    evidence_manifest = {
        "schema_version": SCHEMA_VERSION,
        "stage": "D12_DIAGNOSTIC_EVIDENCE_MANIFEST",
        "generated_at": now_iso(),
        "status": manifest["status"],
        "files": evidence_files,
        "file_count": len(evidence_files),
    }
    write_json(output_dir / "evidence-manifest.json", evidence_manifest)

    print(
        json.dumps(
            {
                "status": manifest["status"],
                "output_dir": str(output_dir),
                "valid_baseline_runs": manifest["timing_contract"]["valid_real_run_count"],
                "frozen_asset_count": len(
                    [a for a in manifest["assets"] if a["freeze_status"] == "FROZEN_VERIFIED"]
                ),
                "pending_asset_count": len(
                    [a for a in manifest["assets"] if a["freeze_status"] != "FROZEN_VERIFIED"]
                ),
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
