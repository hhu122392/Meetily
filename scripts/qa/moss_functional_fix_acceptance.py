#!/usr/bin/env python3
"""Materialize, validate, run, and verify the MOSS functional-fix acceptance plan.

This tool is deliberately standard-library-only.  Commands are always passed
to ``subprocess`` as argument arrays; it never uses a shell and never evaluates
an expression from the acceptance file.  Exact commands, working directories,
private paths, and command output are written only to the private report.
"""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import ctypes
from datetime import datetime, timezone
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any, Iterable, Mapping, Sequence

from moss_v3_p6_common import (
    FAIL,
    NOT_RUN,
    PASS,
    REQUIRED_P6_SCENARIOS,
    GateError,
    atomic_write_json,
    canonical_json_bytes,
    file_record,
    hash_file,
    normalize_relative_path,
    read_json,
    reject_symlink,
    require_git_commit,
    require_list,
    require_mapping,
    require_sha256,
    require_string,
    resolve_under,
    sha256_bytes,
    sha256_file,
    status_exit_code,
    utc_now,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


SCHEMA_VERSION = 1
CONFIG_STAGE = "MOSS_FUNCTIONAL_FIX_ACCEPTANCE_CONFIG"
PUBLIC_STAGE = "MOSS_FUNCTIONAL_FIX_ACCEPTANCE"
PRIVATE_STAGE = "MOSS_FUNCTIONAL_FIX_ACCEPTANCE_PRIVATE"
VERIFY_STAGE = "MOSS_FUNCTIONAL_FIX_ACCEPTANCE_VERIFY"
BUILD_MANIFEST_STAGE = "MOSS_FUNCTIONAL_INSTALL_BUILD_MANIFEST"
BUILD_ATTESTATION_STAGE = "MOSS_FUNCTIONAL_CANDIDATE_BUILD"

FORMAL_TEMPLATE_RELATIVE_PATH = (
    "target/release/docs/方案/MOSS功能修复计划-20260902/acceptance.template.json"
)
ACCEPTANCE_SCHEMA_RELATIVE_PATH = (
    "target/release/docs/方案/MOSS功能修复计划-20260902/acceptance.schema.json"
)
CUA_SCHEMA_RELATIVE_PATH = (
    "target/release/docs/方案/MOSS功能修复计划-20260902/cua.schema.json"
)
FORMAL_TRACKED_SCRIPT_PATHS = (
    "scripts/qa/moss_functional_fix_acceptance.py",
    "scripts/qa/moss_v3_p6_common.py",
    "scripts/qa/run-moss-functional-ft.ps1",
    "scripts/qa/moss-functional-ft-cdp.mjs",
    "scripts/qa/windows-native-arguments.ps1",
    "scripts/qa/run-install-lifecycle.ps1",
    "scripts/qa/moss_functional_fix_quality_gate.py",
    ACCEPTANCE_SCHEMA_RELATIVE_PATH,
    CUA_SCHEMA_RELATIVE_PATH,
    FORMAL_TEMPLATE_RELATIVE_PATH,
)
APPROVED_PRODUCT_NAME = "meetily-p6-lifecycle"
APPROVED_BUNDLE_ID = "com.meetily.ai.p6lifecycle"
APPROVED_BUILD_COMMANDS = (
    "pnpm sidecars:prepare",
    "pnpm exec tauri build --config src-tauri/tauri.lifecycle.conf.json -- --features vulkan",
)
APPROVED_BUILD_ENVIRONMENT = {
    "LIBCLANG_PATH": r"D:\MeetilyBuildTools\clang+llvm-19.1.5-x86_64-pc-windows-msvc\bin",
    "VULKAN_SDK": r"D:\VulkanSDK\1.4.357.0",
    "CMAKE_GENERATOR": "NMake Makefiles",
    "MEETILY_WEBVIEW2_CACHE_ROOT": r"D:\MeetilyData\build-deps\webview2-fixed",
}
INSTALLED_ROLE_PATHS = {
    "main_executable": "meetily.exe",
    "llama_helper": "llama-helper.exe",
    "moss_helper": "moss-helper.exe",
    "ffmpeg": "ffmpeg.exe",
    "directml": "DirectML.dll",
    "webview2": "runtime/webview2-fixed/msedgewebview2.exe",
    "uninstaller": "uninstall.exe",
}
BUILD_ARTIFACT_ROLE_PATHS = {
    "main_executable": "target/release/meetily.exe",
    "llama_helper": "frontend/src-tauri/binaries/llama-helper-x86_64-pc-windows-msvc.exe",
    "moss_helper": "frontend/src-tauri/binaries/moss-helper-x86_64-pc-windows-msvc.exe",
    "ffmpeg": "frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe",
    "directml": "frontend/src-tauri/runtime/windows-x64/nsis/DirectML.dll",
    "webview2": "frontend/src-tauri/runtime/webview2-fixed/msedgewebview2.exe",
}
BUILD_PRODUCER_RELATIVE_PATH = "scripts/qa/build-moss-functional-candidate.ps1"
BUILD_MANIFEST_PRODUCER_RELATIVE_PATH = "scripts/qa/new-install-build-manifest.ps1"
ROLLBACK_TOOL_RELATIVE_PATH = "frontend/src-tauri/scripts/meetily-versioned-data.ps1"

FORMAL_CASE_IDS = tuple(f"FT-{index:02d}" for index in range(1, 29))
FORMAL_CASE_NAMES = dict(zip(FORMAL_CASE_IDS, REQUIRED_P6_SCENARIOS, strict=True))
FORMAL_RUN_KEY_BY_ID = {
    **{case_id: "ft01-03-live-and-persistence" for case_id in FORMAL_CASE_IDS[0:3]},
    **{case_id: "ft04-15-moss-chain" for case_id in FORMAL_CASE_IDS[3:15]},
    **{case_id: "ft16-21-fault-chain" for case_id in FORMAL_CASE_IDS[15:21]},
    **{case_id: "ft22-25-install-lifecycle" for case_id in FORMAL_CASE_IDS[21:25]},
    "FT-26": "ft26-data-drive-placement",
    "FT-27": "ft27-long-audio",
    "FT-28": "ft28-business-chain",
}
FORMAL_RUN_KEYS = tuple(dict.fromkeys(FORMAL_RUN_KEY_BY_ID.values()))
CUA_TASK_IDS = tuple(f"AT-{index:02d}" for index in range(22))
CUA_RECORD_STAGE = "MOSS_FUNCTIONAL_CUA_PRIVATE"
CUA_HASH_MANIFEST_STAGE = "MOSS_FUNCTIONAL_CUA_EVIDENCE_SHA256"
RUN_REGISTRATION_STAGE = "MOSS_FUNCTIONAL_RUN_REGISTRATION"
CUA_RECORD_PATH_BY_TASK = {
    task_id: f"CUA/{task_id}/cua.private.json" for task_id in CUA_TASK_IDS
}
CUA_HASH_MANIFEST_PATH_BY_TASK = {
    task_id: f"CUA/{task_id}/evidence-sha256.json" for task_id in CUA_TASK_IDS
}


def _ft_private_paths(first: int, last: int) -> list[str]:
    return [f"FT-{number:02d}/result.private.json" for number in range(first, last + 1)]


CUA_MACHINE_EVIDENCE_ALLOWLIST = [
    {
        "scope_id": "at00-clean-environment",
        "paths": ["AT-00/result.private.json"],
        "task_ids": ["AT-00"],
        "producer_key": None,
    },
    {
        "scope_id": "at01-final-candidate",
        "paths": ["AT-01/result.private.json"],
        "task_ids": ["AT-01"],
        "producer_key": None,
    },
    {
        "scope_id": "at02-user-data-baseline",
        "paths": ["AT-02/result.private.json"],
        "task_ids": ["AT-02"],
        "producer_key": None,
    },
    {
        "scope_id": "ft01-03-live-and-persistence",
        "paths": _ft_private_paths(1, 3),
        "task_ids": ["AT-03", "AT-09", "AT-10"],
        "producer_key": "ft01-03-live-and-persistence",
    },
    {
        "scope_id": "ft04-15-moss-chain",
        "paths": _ft_private_paths(4, 15),
        "task_ids": ["AT-04", "AT-05", "AT-11", "AT-12", "AT-13", "AT-14"],
        "producer_key": "ft04-15-moss-chain",
    },
    {
        "scope_id": "ft16-21-fault-chain",
        "paths": _ft_private_paths(16, 21),
        "task_ids": ["AT-05", "AT-11", "AT-15"],
        "producer_key": "ft16-21-fault-chain",
    },
    {
        "scope_id": "ft22-25-install-lifecycle",
        "paths": _ft_private_paths(22, 25),
        "task_ids": ["AT-07"],
        "producer_key": "ft22-25-install-lifecycle",
    },
    {
        "scope_id": "ft26-data-drive-placement",
        "paths": _ft_private_paths(26, 26),
        "task_ids": ["AT-06"],
        "producer_key": "ft26-data-drive-placement",
    },
    {
        "scope_id": "at08-q00-quality-gate",
        "paths": ["AT-08/result.private.json"],
        "task_ids": ["AT-08"],
        "producer_key": None,
    },
    {
        "scope_id": "ft27-long-audio",
        "paths": _ft_private_paths(27, 27),
        "task_ids": ["AT-16"],
        "producer_key": "ft27-long-audio",
    },
    {
        "scope_id": "ft28-business-chain",
        "paths": _ft_private_paths(28, 28),
        "task_ids": ["AT-17"],
        "producer_key": "ft28-business-chain",
    },
    {
        "scope_id": "at18-user-data-final",
        "paths": ["AT-18/result.private.json"],
        "task_ids": ["AT-18"],
        "producer_key": None,
    },
    {
        "scope_id": "at19-final-report",
        "paths": ["AT-19/result.private.json"],
        "task_ids": ["AT-19"],
        "producer_key": None,
    },
    {
        "scope_id": "at20-post-merge-smoke",
        "paths": ["AT-20/result.private.json"],
        "task_ids": ["AT-20"],
        "producer_key": None,
    },
    {
        "scope_id": "at21-cleanup",
        "paths": ["AT-21/result.private.json"],
        "task_ids": ["AT-21"],
        "producer_key": None,
    },
]
CUA_ALLOWED_MACHINE_PATHS_BY_TASK = {
    task_id: tuple(
        path
        for scope in CUA_MACHINE_EVIDENCE_ALLOWLIST
        if task_id in scope["task_ids"]
        for path in scope["paths"]
    )
    for task_id in CUA_TASK_IDS
}
CUA_PRODUCER_KEY_BY_MACHINE_PATH = {
    path: scope["producer_key"]
    for scope in CUA_MACHINE_EVIDENCE_ALLOWLIST
    for path in scope["paths"]
}
FORMAL_CUA_CONTRACT = {
    "schema_relative_path": CUA_SCHEMA_RELATIVE_PATH,
    "record_count": len(CUA_TASK_IDS),
    "records": [
        {
            "task_id": task_id,
            "record_path": CUA_RECORD_PATH_BY_TASK[task_id],
            "hash_manifest_path": CUA_HASH_MANIFEST_PATH_BY_TASK[task_id],
        }
        for task_id in CUA_TASK_IDS
    ],
    "machine_evidence_allowlist": CUA_MACHINE_EVIDENCE_ALLOWLIST,
    "rules": {
        "one_record_per_at": True,
        "ui_run_ids_unique": True,
        "machine_and_ui_run_ids_must_differ": True,
        "candidate_identity_exact": True,
        "machine_event_binding_required": True,
        "undeclared_machine_evidence_reuse_forbidden": True,
        "producer_and_cua_sessions_must_not_overlap": True,
        "record_hash_manifest_required": True,
    },
}
FORMAL_DEPENDENCIES_BY_ID = {
    **{case_id: [] for case_id in FORMAL_CASE_IDS[0:3]},
    **{case_id: list(FORMAL_CASE_IDS[0:3]) for case_id in FORMAL_CASE_IDS[3:15]},
    **{case_id: list(FORMAL_CASE_IDS[3:15]) for case_id in FORMAL_CASE_IDS[15:21]},
    **{case_id: list(FORMAL_CASE_IDS[15:21]) for case_id in FORMAL_CASE_IDS[21:25]},
    "FT-26": list(FORMAL_CASE_IDS[21:25]),
    "FT-27": ["FT-26"],
    "FT-28": ["FT-27"],
}
GROUP_RERUN_SCOPE = "WHOLE_RUN_ONCE_GROUP"
FORMAL_FAILURE_RERUN_SCOPE = (
    "NEW_MACHINE_RUN_ID_NEW_EVIDENCE_ROOTS_REREGISTER_AT00_THROUGH_AT02_Q00_AND_FT01_THROUGH_FT28"
)
FORMAL_EXECUTION_CONTRACT = {
    "formal_order": [
        "CANDIDATE_LOCKED",
        "MACHINE_RUN_ID_REGISTERED",
        "AT00_THROUGH_AT02_PASS",
        "Q00_PASS",
        "MATERIALIZE",
        "FT01_THROUGH_FT28",
    ],
    "formal_run_scope": "ALL_28_CASES",
    "formal_failure_rerun_scope": FORMAL_FAILURE_RERUN_SCOPE,
    "debug_rerun_scope": GROUP_RERUN_SCOPE,
    "debug_results_releasable": False,
    "at09_priority": "P0",
    "ft26_dependencies": ["FT-22", "FT-23", "FT-24", "FT-25"],
    "priority_definitions": {
        "P0": {
            "blocks_formal_high_cost_tests": True,
            "blocks_merge": True,
            "blocks_project_completion": True,
        },
        "P1": {
            "blocks_formal_high_cost_tests": False,
            "blocks_merge": True,
            "blocks_project_completion": True,
        },
        "P2": {
            "blocks_formal_high_cost_tests": False,
            "blocks_merge": False,
            "blocks_project_completion": True,
        },
    },
    "run_identity": {
        "machine_run_id_field": "machine_run_id",
        "legacy_machine_run_id_field": "run_id",
        "machine_run_id_must_equal_legacy_run_id": True,
        "ui_run_id_field": "ui_run_id",
        "ui_run_id_scope": "ONE_UNIQUE_ID_PER_AT",
        "machine_and_ui_run_ids_must_differ": True,
        "machine_and_ui_sessions_must_not_overlap": True,
    },
}
FORMAL_GROUP_CONTRACTS = {
    "ft01-03-live-and-persistence": {"timeout_seconds": 3600, "input_count": 2},
    "ft04-15-moss-chain": {"timeout_seconds": 3600, "input_count": 2},
    "ft16-21-fault-chain": {"timeout_seconds": 3600, "input_count": 2},
    "ft22-25-install-lifecycle": {"timeout_seconds": 7200, "input_count": 2},
    "ft26-data-drive-placement": {"timeout_seconds": 3600, "input_count": 1},
    "ft27-long-audio": {"timeout_seconds": 10800, "input_count": 2},
    "ft28-business-chain": {"timeout_seconds": 7200, "input_count": 4},
}
FORMAL_CLEANUP_TIMEOUT_SECONDS = 600
FORMAL_FORCE_STOP_SECONDS = 30
FORMAL_INPUT_RECORDS = {
    "short_audio": (7_246_124, "5128D52F41A6FA58BBD9387C6C68D09E076E9E782C8C13B8BF0CF5BB8655AD22"),
    "short_human_verbatim": (8_136, "7B81C3D80043BD2CFA85EE892058BC4E432F3BA67DC18FD92FF0A0B8D97DD4FC"),
    "baseline_installer": (386_218_356, "1C151B1534A66927FFA5B50DE58D05EE27247B933797441A59C747510C32483C"),
    "lifecycle_fixture_manifest": (1_099, "3FA6AF05E04FDDC7930A397FD2AFDF44D263BC6051DBEA4200C21E073AE30ECB"),
    "long_audio": (99_215_438, "2C293CAE418ACC6FA8CB5A0620B966FDF0C1E5629D730ABB5A249D1DEC850FE6"),
    "long_chunk_manifest": (6_976, "2B77448B160221BDCC5C320B59F61D1503521EC9EC702F5FF2DFEBC31274B546"),
    "business_audio": (23_607_374, "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"),
    "positive_truth": (6_363, "0F9AC1B5FE8EBD75995EC70118D32BAD2889C28C62803C0D031B489D072549B5"),
    "negative_truth": (1_605, "41A3FA60A6A2F6775C14E58D9E37E58480D9BA64D5541F4CF5412FA3697D83F7"),
}
FORMAL_GROUP_INPUT_ROLES = {
    "ft01-03-live-and-persistence": ("short_audio", "short_human_verbatim"),
    "ft04-15-moss-chain": ("short_audio", "short_human_verbatim"),
    "ft16-21-fault-chain": ("short_audio", "short_human_verbatim"),
    "ft22-25-install-lifecycle": ("baseline_installer", "lifecycle_fixture_manifest"),
    "ft26-data-drive-placement": ("short_audio",),
    "ft27-long-audio": ("long_audio", "long_chunk_manifest"),
    "ft28-business-chain": (
        "business_audio",
        "short_human_verbatim",
        "positive_truth",
        "negative_truth",
    ),
}
FORMAL_TOKEN_NAMES = {
    "SHORT_INPUT_PATH",
    "SHORT_REFERENCE_PATH",
    "BASELINE_INSTALLER_PATH",
    "LIFECYCLE_FIXTURE_MANIFEST_PATH",
    "LONG_INPUT_PATH",
    "LONG_CHUNK_MANIFEST_PATH",
    "BUSINESS_INPUT_PATH",
    "BUSINESS_REFERENCE_PATH",
    "TERM_ALLOWLIST_PATH",
    "NEGATIVE_TERM_SOURCE_PATH",
}

# These are the business facts emitted by run-moss-functional-ft.ps1.  A formal
# case is decided by these facts, never by copying the producer's ``verdict``.
FORMAL_COMMON_CHECK_KEYS = (
    "candidate_process_identity_bound",
    "evidence_created_after_run_registration",
    "machine_event_ledger_complete",
)
FORMAL_CHECK_KEYS_BY_ID = {
    "FT-01": (
        "real_candidate_bootstrap",
        "fixed_audio_played_complete",
        "multiple_nonempty_live_changes",
        "final_transcript_nonempty",
        "timestamps_parseable",
        "timestamps_legal",
        "timestamps_monotonic",
        "pause_observed",
        "transcript_continues_after_resume",
        "tail_marker_present_exactly_once",
        "finalization_completed_after_queue_drained",
    ),
    "FT-02": (
        "backend_stopped",
        "finalization_created_meeting",
        "transcript_stable_after_five_seconds",
        "segment_count_stable",
        "state_idle",
        "stop_feedback_within_one_second",
        "page_unlocked_within_ten_seconds",
        "automatic_full_retranscription_count_zero",
        "finalization_completed_after_queue_drained",
    ),
    "FT-03": (
        "title_edited_through_ui",
        "transcript_edited_through_ui",
        "first_qwen_summary_completed",
        "second_qwen_summary_completed",
        "title_exact_after_restart",
        "transcript_exact_after_restart",
        "updated_at_exact_after_restart",
        "two_completed_summaries_persist",
    ),
    "FT-04": (
        "completed",
        "progress_monotonic",
        "progress_samples_present",
        "candidate_stored",
        "enhance_feedback_within_one_second",
        "whisper_enhance_completed_within_300_seconds",
        "automatic_batch_enhancement_count_zero",
    ),
    "FT-05": (
        "cancelled",
        "cancelled_run_has_no_candidate",
        "cancel_feedback_within_one_second",
        "process_tree_evidence_complete",
        "residual_processes_zero",
    ),
    "FT-06": ("candidate_not_auto_activated", "manual_candidate_edit_persisted"),
    "FT-07": ("positive_truth_nonempty", "correction_records_present", "all_positive_terms_traceable"),
    "FT-08": ("negative_truth_nonempty", "negative_insertions_zero"),
    "FT-09": (
        "participants_present",
        "speaker_labels_present",
        "binding_applied",
        "segment_override_wins",
        "correction_round_trip",
    ),
    "FT-10": ("revision_exact", "edit_exact", "binding_exact", "override_exact", "correction_applied"),
    "FT-11": ("candidate_activated", "active_run_matches", "activation_id_present"),
    "FT-12": (
        "real_qwen_summary_completed",
        "source_binding_present",
        "source_is_moss",
        "helper_process_count_zero",
        "summary_feedback_within_one_second",
        "summary_completed_within_180_seconds",
        "process_tree_evidence_complete",
        "residual_processes_zero",
    ),
    "FT-13": ("stale_revision_rejected", "exact_conflict_code"),
    "FT-14": ("active_run_cleared", "candidate_inactive", "whisper_summary_completed", "summary_source_is_whisper"),
    "FT-15": ("same_candidate_reactivated", "active_run_matches", "summary_completed", "summary_source_is_moss", "helper_process_count_zero"),
    **{
        f"FT-{number:02d}": (
            "sandbox_launcher_exit_zero",
            "windows_sandbox_proven",
            "sandbox_admin_proven",
            "fixed_worker_hash_bound",
            "host_firewall_untouched",
            "sandbox_cleanup_pass",
            "exact_case_facts_all_true",
            "residual_moss_helpers_zero",
            "residual_llama_helpers_zero",
            "residual_qwen_helpers_zero",
            "process_tree_evidence_complete",
            "moss_and_qwen_never_overlapped",
        )
        for number in range(16, 22)
    },
    **{
        f"FT-{number:02d}": (
            "lifecycle_process_exit_zero",
            "lifecycle_case_pass",
            "lifecycle_functional_pass",
            "lifecycle_cleanup_pass",
            "lifecycle_residual_processes_zero",
            "candidate_manifest_bound",
            "baseline_manifest_bound",
        )
        for number in range(22, 26)
    },
    "FT-26": (
        "selected_root_saved",
        "selected_root_survived_restart",
        "selected_root_contains_recording",
        "default_root_manifest_unchanged",
        "invalid_root_explicitly_rejected",
        "preference_unchanged_after_invalid",
        "no_invalid_half_product",
    ),
    "FT-27": (
        "full_audio_played_once",
        "moss_completed",
        "all_timestamps_parseable",
        "all_timestamps_legal",
        "all_timestamps_monotonic",
        "candidate_segments_nonempty",
        "tail_difference_at_most_half_second",
        "helper_process_count_zero",
        "tail_marker_present_exactly_once",
        "finalization_completed_after_queue_drained",
        "process_tree_evidence_complete",
        "residual_processes_zero",
        "moss_and_qwen_never_overlapped",
    ),
    "FT-28": (
        "full_business_audio_played",
        "product_moss_completed",
        "strict_corrections_and_speaker_override_completed",
        "candidate_activated",
        "real_qwen_summary_completed",
        "summary_source_is_active_moss",
        "q00_verify_exit_zero",
        "q00_status_pass",
        "q00_truth_and_source_match_ft28_inputs",
        "q00_window_duration_is_226_440_seconds",
        "q00_current_artifacts_bound_to_candidate_files",
        "moss_cer_at_most_20_percent",
        "moss_not_worse_than_whisper",
        "corrected_cer_at_most_15_percent",
        "positive_term_accuracy_at_least_95_percent",
        "negative_term_insertions_zero",
        "moss_rtf_at_most_one",
        "manual_speaker_coverage_error_zero",
        "helper_process_count_zero",
        "tail_marker_present_exactly_once",
        "finalization_completed_after_queue_drained",
        "process_tree_evidence_complete",
        "residual_processes_zero",
        "moss_and_qwen_never_overlapped",
    ),
}

OLD_WORKTREE_MARKER = "meetily-moss-ft-1b9c29a-20260902"
OLD_RUN_ID = "FT-1B9C29A-20260902-01"
OLD_CANDIDATE_SHA256 = "27EB95ED81A6E6DA5C37B33B36639B7BA7D592018DF6065531C6A8221D16381F"
TOKEN_RE = re.compile(r"__[A-Z][A-Z0-9_]*__")
TOKEN_NAME_RE = re.compile(r"^[A-Z][A-Z0-9_]*$")
SAFE_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
GENERIC_PLACEHOLDER_RE = re.compile(
    r"(?:\bTODO\b|\bTBD\b|\bREPLACE[_ -]?ME\b|\bPLACEHOLDER\b)", re.IGNORECASE
)
ASSERTION_OPS = {
    "eq",
    "ne",
    "in",
    "not_in",
    "exists",
    "absent",
    "truthy",
    "falsy",
    "gt",
    "ge",
    "lt",
    "le",
    "length_eq",
}
SHARED_FIELDS = (
    "source_commit",
    "candidate_sha256",
    "argv",
    "cwd",
    "expected_exit_codes",
    "inputs",
    "timeout_seconds",
    "force_stop_seconds",
    "dependencies",
    "cleanup",
)


def _require_exact_keys(
    value: Mapping[str, Any],
    expected: Iterable[str],
    label: str,
) -> None:
    expected_set = set(expected)
    actual_set = set(value)
    missing = sorted(expected_set - actual_set)
    extra = sorted(actual_set - expected_set)
    if missing or extra:
        raise GateError(f"{label} fields are not exact; missing={missing}, extra={extra}")


def _require_execution_contract(value: Any) -> dict[str, Any]:
    contract = require_mapping(value, "execution_contract")
    if contract != FORMAL_EXECUTION_CONTRACT:
        raise GateError("acceptance execution_contract does not match the fixed formal policy")
    return dict(contract)


def formal_schema_template_bindings(repo: Path | None = None) -> dict[str, dict[str, Any]]:
    root = (repo or Path(__file__).resolve().parents[2]).resolve()
    result: dict[str, dict[str, Any]] = {}
    for name, relative in (
        ("acceptance", ACCEPTANCE_SCHEMA_RELATIVE_PATH),
        ("cua", CUA_SCHEMA_RELATIVE_PATH),
    ):
        path = root / relative
        raw = file_record(path, relative_path=relative)
        result[name] = {
            "relative_path": raw["path"],
            "bytes": raw["bytes"],
            "sha256": raw["sha256"],
        }
    return result


def _require_schema_bindings(value: Any, repo: Path) -> dict[str, dict[str, Any]]:
    bindings = require_mapping(value, "schema_bindings")
    expected = formal_schema_template_bindings(repo)
    if bindings != expected:
        raise GateError("formal acceptance schema bindings do not match the tracked schemas")
    for name, relative in (
        ("acceptance", ACCEPTANCE_SCHEMA_RELATIVE_PATH),
        ("cua", CUA_SCHEMA_RELATIVE_PATH),
    ):
        schema = require_mapping(read_json(repo / relative, f"formal {name} schema"), name)
        if (
            schema.get("$schema") != "https://json-schema.org/draft/2020-12/schema"
            or schema.get("type") != "object"
        ):
            raise GateError(f"formal {name} schema header is invalid")
    return expected


def _require_cua_contract(value: Any) -> dict[str, Any]:
    contract = require_mapping(value, "cua_contract")
    if contract != FORMAL_CUA_CONTRACT:
        raise GateError("cua_contract does not match the fixed 22-task policy")
    return dict(contract)


def _parse_timestamp(value: Any, label: str) -> datetime:
    text = require_string(value, label)
    normalized = text[:-1] + "+00:00" if text.endswith("Z") else text
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as exc:
        raise GateError(f"{label} must be an ISO-8601 timestamp") from exc
    if parsed.tzinfo is None:
        raise GateError(f"{label} must include a time-zone offset")
    return parsed.astimezone(timezone.utc)


def _validate_producer_nonces(value: Any) -> dict[str, str]:
    raw = require_mapping(value, "producer_nonces")
    if list(raw) != list(FORMAL_RUN_KEYS):
        raise GateError("producer_nonces must contain the seven producer keys in formal order")
    normalized = {
        key: require_sha256(raw.get(key), f"producer nonce {key}")
        for key in FORMAL_RUN_KEYS
    }
    if len(set(normalized.values())) != len(FORMAL_RUN_KEYS):
        raise GateError("all seven producer nonces must be unique")
    return normalized


def _run_registration_entry_path(registry_root: Path, run_id: str) -> Path:
    digest = sha256_bytes(run_id.encode("utf-8")).lower()
    return registry_root / f"{digest}.registration.json"


def _exclusive_write_json(path: Path, value: Mapping[str, Any], label: str) -> None:
    encoded = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode("utf-8") + b"\n"
    descriptor: int | None = None
    try:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as stream:
            descriptor = None
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
    except FileExistsError as exc:
        raise GateError(f"{label} already exists; the run id cannot be reused") from exc
    finally:
        if descriptor is not None:
            os.close(descriptor)


def _create_run_registration(
    *,
    registry_root: Path,
    run_id: str,
    source_commit: str,
    candidate_sha256: str,
    build_manifest_sha256: str,
    public_root: Path,
    private_root: Path,
    registered_at: str | None = None,
    registration_nonce: str | None = None,
) -> dict[str, Any]:
    machine_run_id = _safe_id(run_id, "machine_run_id")
    if machine_run_id.casefold() == OLD_RUN_ID.casefold():
        raise GateError("obsolete machine_run_id cannot be registered")
    source = require_git_commit(source_commit, "run registration source_commit")
    candidate_digest = require_sha256(candidate_sha256, "run registration candidate_sha256")
    manifest_digest = require_sha256(
        build_manifest_sha256, "run registration build_manifest_sha256"
    )
    nonce = require_sha256(
        registration_nonce or secrets.token_hex(32), "run registration nonce"
    )
    timestamp = registered_at or utc_now()
    _parse_timestamp(timestamp, "run registration registered_at")

    registry_root = Path(os.path.abspath(registry_root))
    public_root = Path(os.path.abspath(public_root))
    private_root = Path(os.path.abspath(private_root))
    registry_root.mkdir(parents=True, exist_ok=True)
    _reject_link_chain(registry_root)
    for root, label in ((public_root, "public evidence root"), (private_root, "private evidence root")):
        root.mkdir(parents=True, exist_ok=True)
        _reject_link_chain(root)
        if any(root.iterdir()):
            raise GateError(f"{label} must be empty when the formal run id is registered")
    if (
        public_root == private_root
        or _is_under(public_root, private_root)
        or _is_under(private_root, public_root)
    ):
        raise GateError("public and private evidence roots must be separate non-nested directories")
    if any(
        registry_root == root or _is_under(registry_root, root) or _is_under(root, registry_root)
        for root in (public_root, private_root)
    ):
        raise GateError("run registry must be separate from both evidence roots")

    document = {
        "schema_version": SCHEMA_VERSION,
        "stage": RUN_REGISTRATION_STAGE,
        "registered_at": timestamp,
        "machine_run_id": machine_run_id,
        "source_commit": source,
        "candidate_sha256": candidate_digest,
        "build_manifest_sha256": manifest_digest,
        "public_root": str(public_root),
        "private_root": str(private_root),
        "registration_nonce": nonce,
    }
    entry_path = _run_registration_entry_path(registry_root, machine_run_id)
    _exclusive_write_json(entry_path, document, "formal run registration")
    record = file_record(entry_path, relative_path=entry_path.name)
    return {**record, "directory": registry_root, "path": entry_path, "document": document}


def register_run_id(
    *,
    repo: Path,
    registry_root: Path,
    run_id: str,
    candidate_path: Path,
    build_manifest_path: Path,
    public_root: Path,
    private_root: Path,
) -> dict[str, Any]:
    repo = repo.resolve()
    for root, label in (
        (registry_root, "run registry"),
        (public_root, "public evidence root"),
        (private_root, "private evidence root"),
    ):
        if _is_under(Path(os.path.abspath(root)), repo):
            raise GateError(f"formal {label} must be outside the repository")
    binding = _assert_repository_clean(repo, "formal run registration")
    candidate_record = file_record(candidate_path.resolve(), relative_path=candidate_path.name)
    manifest_record = file_record(
        build_manifest_path.resolve(), relative_path=build_manifest_path.name
    )
    _validate_build_manifest(
        {**manifest_record, "path": build_manifest_path.resolve()},
        {**candidate_record, "path": candidate_path.resolve()},
        repo,
        binding["head"],
    )
    return _create_run_registration(
        registry_root=registry_root,
        run_id=run_id,
        source_commit=binding["head"],
        candidate_sha256=candidate_record["sha256"],
        build_manifest_sha256=manifest_record["sha256"],
        public_root=public_root,
        private_root=private_root,
    )


def _validate_run_registry(
    value: Any,
    *,
    repo: Path,
    run_id: str,
    source_commit: str,
    candidate_sha256: str,
    build_manifest_sha256: str,
    public_root: Path,
    private_root: Path,
) -> dict[str, Any]:
    raw = require_mapping(value, "run_registry")
    _require_exact_keys(raw, {"directory", "registration"}, "run_registry")
    directory = _absolute_path(raw.get("directory"), "run registry directory")
    _reject_link_chain(directory)
    if not directory.is_dir() or _is_under(directory, repo):
        raise GateError("run registry must be an existing directory outside the repository")
    if any(
        directory == root or _is_under(directory, root) or _is_under(root, directory)
        for root in (public_root, private_root)
    ):
        raise GateError("run registry must be separate from both evidence roots")
    registration = _bound_file(raw.get("registration"), "run registration")
    expected_path = _run_registration_entry_path(directory, run_id).resolve()
    if registration["path"] != expected_path:
        raise GateError("run registration path does not match the machine_run_id")
    matching_registry_entries = 0
    for candidate_entry in directory.glob("*.registration.json"):
        candidate_document = require_mapping(
            read_json(candidate_entry, "run registry entry"), "run registry entry"
        )
        if candidate_document.get("machine_run_id") == run_id:
            matching_registry_entries += 1
    if matching_registry_entries != 1:
        raise GateError("machine_run_id must appear exactly once in the run registry")
    document, document_sha, document_bytes = _stable_json(expected_path, "run registration")
    _require_exact_keys(
        document,
        {
            "schema_version",
            "stage",
            "registered_at",
            "machine_run_id",
            "source_commit",
            "candidate_sha256",
            "build_manifest_sha256",
            "public_root",
            "private_root",
            "registration_nonce",
        },
        "run registration",
    )
    if document.get("schema_version") != SCHEMA_VERSION or document.get("stage") != RUN_REGISTRATION_STAGE:
        raise GateError("run registration schema/stage is invalid")
    _parse_timestamp(document.get("registered_at"), "run registration registered_at")
    require_sha256(document.get("registration_nonce"), "run registration nonce")
    if (
        document.get("machine_run_id") != run_id
        or require_git_commit(document.get("source_commit"), "run registration source_commit")
        != source_commit
        or require_sha256(document.get("candidate_sha256"), "run registration candidate_sha256")
        != candidate_sha256
        or require_sha256(
            document.get("build_manifest_sha256"), "run registration build_manifest_sha256"
        )
        != build_manifest_sha256
        or document.get("public_root") != str(public_root)
        or document.get("private_root") != str(private_root)
    ):
        raise GateError("run registration identity or evidence roots do not match acceptance")
    if document_sha != registration["sha256"] or document_bytes != registration["bytes"]:
        raise GateError("run registration binding changed while it was validated")
    return {**registration, "document": document, "directory": directory}


def _run_git(repo: Path, arguments: Sequence[str], label: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(repo), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise GateError(f"{label} failed: {detail}")
    return completed.stdout.strip()


def _tracked_workspace_binding(repo: Path, relative_path: str, label: str) -> dict[str, Any]:
    normalized = normalize_relative_path(relative_path, f"{label} relative path")
    path, _ = resolve_under(repo, normalized, f"{label} path")
    _reject_link_chain(path)
    if not path.is_file():
        raise GateError(f"{label} is missing: {normalized}")
    tracked = _run_git(
        repo,
        [
            "-c",
            "core.quotePath=false",
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            normalized,
        ],
        label,
    )
    if tracked.replace("\\", "/") != normalized:
        raise GateError(f"{label} is not tracked at the fixed path")
    head_blob = _run_git(repo, ["rev-parse", f"HEAD:{normalized}"], f"{label} HEAD blob")
    workspace_blob = _run_git(repo, ["hash-object", "--", normalized], f"{label} workspace blob")
    if workspace_blob != head_blob:
        raise GateError(f"{label} workspace bytes do not match the current HEAD blob")
    byte_count, digest = hash_file(path)
    return {
        "relative_path": normalized,
        "git_blob": head_blob,
        "workspace_bytes": byte_count,
        "workspace_sha256": digest,
    }


def _assert_repository_clean(
    repo: Path,
    label: str,
    *,
    required_paths: Sequence[str] = FORMAL_TRACKED_SCRIPT_PATHS,
) -> dict[str, Any]:
    repo = repo.resolve()
    top = Path(_run_git(repo, ["rev-parse", "--show-toplevel"], f"{label} repository root")).resolve()
    if top != repo:
        raise GateError(f"{label} repository path is not the Git top level")
    status = _run_git(
        repo,
        ["status", "--porcelain=v1", "--untracked-files=all"],
        f"{label} repository status",
    )
    if status:
        raise GateError(f"{label} requires a completely clean HEAD, including untracked files")
    head = _git_head(repo)
    bindings = {
        relative: _tracked_workspace_binding(repo, relative, f"{label} tracked file")
        for relative in required_paths
    }
    return {"head": head, "tracked_files": bindings}


def _is_formal_contract(
    required_ids: Sequence[str],
    required_names: Mapping[str, str],
    expected_run_keys: Mapping[str, str],
) -> bool:
    return (
        tuple(required_ids) == FORMAL_CASE_IDS
        and dict(required_names) == FORMAL_CASE_NAMES
        and dict(expected_run_keys) == FORMAL_RUN_KEY_BY_ID
    )


def _formal_template_path(repo: Path) -> Path:
    return (repo.resolve() / FORMAL_TEMPLATE_RELATIVE_PATH).resolve()


def _require_formal_template_path(repo: Path, template_path: Path) -> Path:
    expected = _formal_template_path(repo)
    supplied = template_path.resolve()
    if supplied != expected:
        raise GateError(
            "formal acceptance can only use the repository's fixed acceptance.template.json"
        )
    return supplied


def _formal_command_argv(run_key: str, mode: str, config_path: Path) -> list[str]:
    return [
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "scripts/qa/run-moss-functional-ft.ps1",
        "-Mode",
        mode,
        "-ProducerKey",
        run_key,
        "-AcceptanceConfig",
        str(config_path.resolve()),
    ]


def _formal_template_command_argv(run_key: str, mode: str) -> list[str]:
    return [
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "scripts/qa/run-moss-functional-ft.ps1",
        "-Mode",
        mode,
        "-ProducerKey",
        run_key,
        "-AcceptanceConfig",
        "__ACCEPTANCE_CONFIG__",
    ]


def _formal_cleanup_assertions(run_key: str) -> list[dict[str, Any]]:
    path = f"cleanup/{run_key}.public.json"
    return [
        {
            "id": "cleanup-pass",
            "source": "public",
            "path": path,
            "pointer": "/status",
            "op": "eq",
            "value": PASS,
        },
        {
            "id": "zero-residual-processes",
            "source": "public",
            "path": path,
            "pointer": "/residual_process_count",
            "op": "eq",
            "value": 0,
        },
    ]


def _require_formal_scenario_contract(
    raw: Mapping[str, Any],
    *,
    case_id: str,
    run_key: str,
    config_path: Path,
    repo: Path,
    source_commit: str,
    candidate_sha256: str,
    build_manifest_sha256: str,
    run_id: str,
    producer_nonce: str,
    run_registration_sha256: str,
) -> None:
    contract = FORMAL_GROUP_CONTRACTS[run_key]
    expected_pass, expected_fail = _formal_assertion_contract(
        case_id,
        run_key,
        source_commit=source_commit,
        candidate_sha256=candidate_sha256,
        build_manifest_sha256=build_manifest_sha256,
        run_id=run_id,
        producer_nonce=producer_nonce,
        run_registration_sha256=run_registration_sha256,
    )
    if raw.get("pass_assertions") != expected_pass or raw.get("fail_assertions") != expected_fail:
        raise GateError(f"{case_id} does not contain the exact formal business-fact assertions")
    if raw.get("argv") != _formal_command_argv(run_key, "Run", config_path):
        raise GateError(f"{case_id} formal producer command is not exact")
    if Path(str(raw.get("cwd", ""))).resolve() != repo.resolve():
        raise GateError(f"{case_id} formal producer cwd is not the repository root")
    if raw.get("expected_exit_codes") != [0]:
        raise GateError(f"{case_id} formal producer must accept only exit code 0")
    if raw.get("timeout_seconds") != contract["timeout_seconds"]:
        raise GateError(f"{case_id} formal timeout is not exact")
    if raw.get("force_stop_seconds") != FORMAL_FORCE_STOP_SECONDS:
        raise GateError(f"{case_id} formal force-stop timeout is not exact")
    inputs = require_list(raw.get("inputs"), f"{case_id} formal inputs")
    if len(inputs) != contract["input_count"]:
        raise GateError(f"{case_id} formal input count is not exact")
    expected_public = [
        f"{case_id}/result.public.json",
        f"cleanup/{run_key}.public.json",
    ]
    expected_private = [
        f"{case_id}/result.private.json",
        f"cleanup/{run_key}.private.json",
    ]
    evidence = require_mapping(raw.get("evidence"), f"{case_id} formal evidence")
    if evidence.get("public") != expected_public or evidence.get("private") != expected_private:
        raise GateError(f"{case_id} formal evidence paths are not exact")
    cleanup = require_mapping(raw.get("cleanup"), f"{case_id} formal cleanup")
    if cleanup.get("argv") != _formal_command_argv(run_key, "Cleanup", config_path):
        raise GateError(f"{case_id} formal cleanup command is not exact")
    if Path(str(cleanup.get("cwd", ""))).resolve() != repo.resolve():
        raise GateError(f"{case_id} formal cleanup cwd is not the repository root")
    if cleanup.get("expected_exit_codes") != [0]:
        raise GateError(f"{case_id} formal cleanup must accept only exit code 0")
    if cleanup.get("timeout_seconds") != FORMAL_CLEANUP_TIMEOUT_SECONDS:
        raise GateError(f"{case_id} formal cleanup timeout is not exact")
    if cleanup.get("force_stop_seconds") != FORMAL_FORCE_STOP_SECONDS:
        raise GateError(f"{case_id} formal cleanup force-stop timeout is not exact")
    if cleanup.get("checks") != _formal_cleanup_assertions(run_key):
        raise GateError(f"{case_id} formal cleanup assertions are not exact")


@contextmanager
def _formal_repository_guard(
    repo: Path,
    enabled: bool,
    label: str,
) -> Iterable[dict[str, Any] | None]:
    before = _assert_repository_clean(repo, f"{label} before") if enabled else None
    try:
        yield before
    finally:
        if enabled:
            after = _assert_repository_clean(repo, f"{label} after")
            if before is not None and after["head"] != before["head"]:
                raise GateError(f"{label} changed repository HEAD")


def _safe_id(value: Any, label: str) -> str:
    text = require_string(value, label)
    if not SAFE_ID_RE.fullmatch(text) or text in {".", ".."}:
        raise GateError(f"{label} must be a filesystem-safe identifier")
    return text


def _git_head(repo: Path) -> str:
    repo = repo.resolve()
    completed = subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--verify", "HEAD"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        raise GateError("repository HEAD cannot be read")
    return require_git_commit(completed.stdout.strip(), "repository HEAD")


def _is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def _reject_link_chain(path: Path) -> None:
    absolute = Path(os.path.abspath(path))
    boundary = Path(absolute.anchor) if absolute.anchor else None
    reject_symlink(absolute, boundary=boundary)


def _absolute_path(value: Any, label: str) -> Path:
    text = require_string(value, label)
    windows = PureWindowsPath(text)
    candidate = Path(text)
    if not candidate.is_absolute() and not windows.is_absolute():
        raise GateError(f"{label} must be an absolute path")
    if "\x00" in text:
        raise GateError(f"{label} contains a NUL character")
    return Path(os.path.abspath(candidate))


def _stable_json(path: Path, label: str) -> tuple[dict[str, Any], str, int]:
    before_bytes, before_hash = hash_file(path)
    value = require_mapping(read_json(path, label), label)
    after_bytes, after_hash = hash_file(path)
    if (before_bytes, before_hash) != (after_bytes, after_hash):
        raise GateError(f"{label} changed while it was being read")
    return value, before_hash, before_bytes


def _bound_file(value: Any, label: str) -> dict[str, Any]:
    record = require_mapping(value, label)
    _require_exact_keys(record, {"path", "bytes", "sha256"}, label)
    path = _absolute_path(record.get("path"), f"{label} path")
    _reject_link_chain(path)
    if not path.is_file():
        raise GateError(f"{label} is missing: {path}")
    expected_bytes = record.get("bytes")
    if isinstance(expected_bytes, bool) or not isinstance(expected_bytes, int) or expected_bytes < 0:
        raise GateError(f"{label} bytes must be a non-negative integer")
    expected_sha = require_sha256(record.get("sha256"), f"{label} sha256")
    actual_bytes, actual_sha = hash_file(path)
    if expected_bytes != actual_bytes:
        raise GateError(f"{label} byte count does not match the real file")
    if expected_sha != actual_sha:
        raise GateError(f"{label} sha256 does not match the real file")
    return {"path": path, "bytes": actual_bytes, "sha256": actual_sha}


def _file_evidence(
    value: Any,
    label: str,
    *,
    expected_relative_path: str | None = None,
    actual_path: Path | None = None,
    allow_path: bool = False,
) -> dict[str, Any]:
    record = require_mapping(value, label)
    expected_keys = {"relative_path", "bytes", "sha256"}
    if allow_path and "path" in record:
        expected_keys.add("path")
    _require_exact_keys(record, expected_keys, label)
    relative = normalize_relative_path(record.get("relative_path"), f"{label} relative_path")
    if expected_relative_path is not None and relative != expected_relative_path:
        raise GateError(f"{label} relative_path is not the fixed path")
    byte_count = record.get("bytes")
    if isinstance(byte_count, bool) or not isinstance(byte_count, int) or byte_count < 0:
        raise GateError(f"{label} bytes must be a non-negative integer")
    digest = require_sha256(record.get("sha256"), f"{label} sha256")
    result: dict[str, Any] = {
        "relative_path": relative,
        "bytes": byte_count,
        "sha256": digest,
    }
    if "path" in record:
        path = _absolute_path(record.get("path"), f"{label} path")
        result["path"] = path
        if actual_path is not None and path.resolve() != actual_path.resolve():
            raise GateError(f"{label} path does not match the required file")
    if actual_path is not None:
        _reject_link_chain(actual_path)
        if not actual_path.is_file():
            raise GateError(f"{label} file is missing: {actual_path}")
        actual_bytes, actual_sha = hash_file(actual_path)
        if (byte_count, digest) != (actual_bytes, actual_sha):
            raise GateError(f"{label} bytes or sha256 do not match the real file")
    return result


def _binding_snapshot(path: Path) -> dict[str, Any]:
    byte_count, digest = hash_file(path)
    stat = path.stat()
    return {
        "bytes": byte_count,
        "sha256": digest,
        "mtime_ns": stat.st_mtime_ns,
        "inode": getattr(stat, "st_ino", 0),
    }


def _validate_build_manifest(
    bound_manifest: Mapping[str, Any],
    candidate: Mapping[str, Any],
    repo: Path,
    source_commit: str,
) -> dict[str, Any]:
    document, document_sha, document_bytes = _stable_json(
        Path(bound_manifest["path"]), "candidate build manifest"
    )
    _require_exact_keys(
        document,
        {
            "schema_version",
            "stage",
            "status",
            "role",
            "generated_at",
            "source_commit",
            "product_name",
            "bundle_id",
            "version",
            "installer",
            "installed_files",
            "producer",
            "rollback_tool",
            "build_provenance",
        },
        "candidate build manifest",
    )
    if (
        document.get("schema_version") != 1
        or document.get("stage") != BUILD_MANIFEST_STAGE
        or document.get("status") != PASS
        or document.get("role") != "candidate"
    ):
        raise GateError("candidate build manifest schema/stage/status/role is invalid")
    manifest_commit = require_git_commit(document.get("source_commit"), "build manifest source_commit")
    if manifest_commit != source_commit:
        raise GateError("candidate build manifest source_commit does not match acceptance")
    if (
        document.get("product_name") != APPROVED_PRODUCT_NAME
        or document.get("bundle_id") != APPROVED_BUNDLE_ID
        or not isinstance(document.get("version"), str)
        or not document["version"]
    ):
        raise GateError("candidate build manifest product identity is invalid")
    installer = _file_evidence(
        document.get("installer"),
        "candidate build manifest installer",
        expected_relative_path=Path(candidate["path"]).name,
    )
    if (installer["bytes"], installer["sha256"]) != (
        candidate["bytes"],
        candidate["sha256"],
    ):
        raise GateError("candidate build manifest installer is not the acceptance candidate")

    installed_raw = require_list(document.get("installed_files"), "installed_files")
    if len(installed_raw) != len(INSTALLED_ROLE_PATHS):
        raise GateError("candidate build manifest must contain exactly the fixed installed roles")
    installed: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(installed_raw):
        item = require_mapping(raw, f"installed_files[{index}]")
        _require_exact_keys(item, {"role", "relative_path", "bytes", "sha256"}, f"installed_files[{index}]")
        role = require_string(item.get("role"), f"installed_files[{index}] role")
        if role in installed or role not in INSTALLED_ROLE_PATHS:
            raise GateError("candidate build manifest has a duplicate or unknown installed role")
        evidence = _file_evidence(
            {key: item[key] for key in ("relative_path", "bytes", "sha256")},
            f"installed role {role}",
            expected_relative_path=INSTALLED_ROLE_PATHS[role],
        )
        if evidence["bytes"] <= 0:
            raise GateError(f"installed role {role} must not be empty")
        installed[role] = evidence
    if set(installed) != set(INSTALLED_ROLE_PATHS):
        raise GateError("candidate build manifest installed-role set is incomplete")

    manifest_producer_path = repo / BUILD_MANIFEST_PRODUCER_RELATIVE_PATH
    manifest_producer = _file_evidence(
        document.get("producer"),
        "candidate build manifest producer",
        expected_relative_path=BUILD_MANIFEST_PRODUCER_RELATIVE_PATH,
        actual_path=manifest_producer_path,
    )
    _tracked_workspace_binding(
        repo, BUILD_MANIFEST_PRODUCER_RELATIVE_PATH, "candidate build manifest producer"
    )

    rollback_path = repo / ROLLBACK_TOOL_RELATIVE_PATH
    rollback = _file_evidence(
        document.get("rollback_tool"),
        "candidate build manifest rollback tool",
        expected_relative_path=ROLLBACK_TOOL_RELATIVE_PATH,
        actual_path=rollback_path,
    )
    _tracked_workspace_binding(repo, ROLLBACK_TOOL_RELATIVE_PATH, "rollback tool")

    provenance = require_mapping(document.get("build_provenance"), "build_provenance")
    _require_exact_keys(
        provenance,
        {"evidence", "build_log", "commands", "artifact_count"},
        "build_provenance",
    )
    if provenance.get("commands") != list(APPROVED_BUILD_COMMANDS) or provenance.get("artifact_count") != 7:
        raise GateError("build_provenance does not contain the approved build commands/artifact count")
    manifest_parent = Path(bound_manifest["path"]).parent
    provenance_evidence_raw = require_mapping(provenance.get("evidence"), "build provenance evidence")
    evidence_relative = normalize_relative_path(
        provenance_evidence_raw.get("relative_path"), "build provenance evidence relative_path"
    )
    attestation_path, _ = resolve_under(manifest_parent, evidence_relative, "build attestation path")
    if "path" not in provenance_evidence_raw:
        raise GateError("build provenance evidence path is required")
    provenance_evidence = _file_evidence(
        provenance_evidence_raw,
        "build provenance evidence",
        expected_relative_path="candidate-build-attestation.private.json",
        actual_path=attestation_path,
        allow_path=True,
    )
    provenance_log_raw = require_mapping(provenance.get("build_log"), "build provenance log")
    log_relative = normalize_relative_path(
        provenance_log_raw.get("relative_path"), "build provenance log relative_path"
    )
    log_path, _ = resolve_under(manifest_parent, log_relative, "build log path")
    if "path" not in provenance_log_raw:
        raise GateError("build provenance log path is required")
    provenance_log = _file_evidence(
        provenance_log_raw,
        "build provenance log",
        expected_relative_path="candidate-build.log",
        actual_path=log_path,
        allow_path=True,
    )

    attestation, _, _ = _stable_json(attestation_path, "candidate build attestation")
    _require_exact_keys(
        attestation,
        {
            "schema_version",
            "stage",
            "status",
            "source_commit",
            "repository_head_before",
            "repository_head_after",
            "worktree_clean_before",
            "worktree_clean_after",
            "product_name",
            "bundle_id",
            "version",
            "started_at",
            "completed_at",
            "commands",
            "executable",
            "environment",
            "producer",
            "build_log",
            "artifacts",
        },
        "candidate build attestation",
    )
    if (
        attestation.get("schema_version") != 1
        or attestation.get("stage") != BUILD_ATTESTATION_STAGE
        or attestation.get("status") != PASS
    ):
        raise GateError("candidate build attestation schema/stage/status is invalid")
    for field in ("source_commit", "repository_head_before", "repository_head_after"):
        if require_git_commit(attestation.get(field), f"build attestation {field}") != source_commit:
            raise GateError("candidate build attestation is not bound to one source commit")
    if attestation.get("worktree_clean_before") is not True or attestation.get("worktree_clean_after") is not True:
        raise GateError("candidate build attestation does not prove a clean worktree before and after")
    if (
        attestation.get("product_name") != APPROVED_PRODUCT_NAME
        or attestation.get("bundle_id") != APPROVED_BUNDLE_ID
        or attestation.get("version") != document["version"]
        or attestation.get("commands") != list(APPROVED_BUILD_COMMANDS)
    ):
        raise GateError("candidate build attestation identity or commands are invalid")
    environment = require_mapping(attestation.get("environment"), "build attestation environment")
    _require_exact_keys(environment, APPROVED_BUILD_ENVIRONMENT, "build attestation environment")
    if dict(environment) != APPROVED_BUILD_ENVIRONMENT:
        raise GateError("candidate build attestation environment is not approved")
    executable = _absolute_path(attestation.get("executable"), "build attestation executable")
    _reject_link_chain(executable)
    if not executable.is_file():
        raise GateError("build attestation executable is missing")

    producer_path = repo / BUILD_PRODUCER_RELATIVE_PATH
    producer = _file_evidence(
        attestation.get("producer"),
        "build attestation producer",
        expected_relative_path=BUILD_PRODUCER_RELATIVE_PATH,
        actual_path=producer_path,
    )
    _tracked_workspace_binding(repo, BUILD_PRODUCER_RELATIVE_PATH, "build attestation producer")
    attestation_log = _file_evidence(
        attestation.get("build_log"),
        "build attestation log",
        expected_relative_path="candidate-build.log",
        actual_path=log_path,
        allow_path=True,
    )
    if (attestation_log["bytes"], attestation_log["sha256"]) != (
        provenance_log["bytes"],
        provenance_log["sha256"],
    ):
        raise GateError("build provenance log is not bound to the attestation log")

    artifacts_raw = require_list(attestation.get("artifacts"), "build attestation artifacts")
    if len(artifacts_raw) != 7:
        raise GateError("build attestation must contain exactly seven approved artifacts")
    artifacts: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(artifacts_raw):
        item = require_mapping(raw, f"build artifact[{index}]")
        _require_exact_keys(item, {"role", "relative_path", "bytes", "sha256"}, f"build artifact[{index}]")
        role = require_string(item.get("role"), f"build artifact[{index}] role")
        if role in artifacts:
            raise GateError("build attestation contains a duplicate artifact role")
        if role == "nsis_installer":
            candidate_path = Path(candidate["path"])
            try:
                expected_relative = candidate_path.resolve().relative_to(repo.resolve()).as_posix()
            except ValueError as exc:
                raise GateError("candidate installer must be below the clean release repository") from exc
            evidence = _file_evidence(
                {key: item[key] for key in ("relative_path", "bytes", "sha256")},
                "build artifact nsis_installer",
                expected_relative_path=expected_relative,
                actual_path=candidate_path,
            )
        else:
            if role not in BUILD_ARTIFACT_ROLE_PATHS:
                raise GateError("build attestation contains an unapproved artifact role")
            expected_relative = BUILD_ARTIFACT_ROLE_PATHS[role]
            evidence = _file_evidence(
                {key: item[key] for key in ("relative_path", "bytes", "sha256")},
                f"build artifact {role}",
                expected_relative_path=expected_relative,
                actual_path=repo / expected_relative,
            )
        if evidence["bytes"] <= 0:
            raise GateError(f"build artifact {role} must not be empty")
        artifacts[role] = evidence
    if set(artifacts) != {*BUILD_ARTIFACT_ROLE_PATHS, "nsis_installer"}:
        raise GateError("build attestation artifact-role set is incomplete")
    if (artifacts["nsis_installer"]["bytes"], artifacts["nsis_installer"]["sha256"]) != (
        candidate["bytes"],
        candidate["sha256"],
    ):
        raise GateError("build attestation installer is not the acceptance candidate")

    return {
        "document": document,
        "sha256": document_sha,
        "bytes": document_bytes,
        "installer": installer,
        "installed_files": installed,
        "producer": manifest_producer,
        "rollback_tool": rollback,
        "build_provenance": {
            "evidence": provenance_evidence,
            "build_log": provenance_log,
            "producer": producer,
            "artifacts": artifacts,
        },
    }


def _validate_duration(value: Any, label: str, maximum: float) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise GateError(f"{label} must be a number")
    result = float(value)
    if not 0.05 <= result <= maximum:
        raise GateError(f"{label} must be between 0.05 and {maximum:g} seconds")
    return result


def _validate_exit_codes(value: Any, label: str) -> list[int]:
    raw = require_list(value, label)
    if not raw or any(isinstance(item, bool) or not isinstance(item, int) for item in raw):
        raise GateError(f"{label} must be a non-empty integer array")
    if len(raw) != len(set(raw)):
        raise GateError(f"{label} contains duplicate exit codes")
    return list(raw)


def _validate_argv(value: Any, label: str) -> list[str]:
    raw = require_list(value, label)
    if not raw or any(not isinstance(item, str) or not item or "\x00" in item for item in raw):
        raise GateError(f"{label} must be a non-empty array of non-empty strings")
    return list(raw)


def _resolve_executable(argv: Sequence[str], cwd: Path) -> Path:
    first = Path(argv[0])
    if first.is_absolute() or first.parent != Path("."):
        executable = first if first.is_absolute() else cwd / first
        executable = executable.resolve()
        if not executable.is_file():
            raise GateError("configured command executable is missing")
        _reject_link_chain(executable)
        return executable
    found = shutil.which(argv[0])
    if not found:
        raise GateError("configured command executable is missing from PATH")
    executable = Path(found).resolve()
    if not executable.is_file():
        raise GateError("configured command executable is not a file")
    return executable


def _validate_command(
    raw_argv: Any,
    raw_cwd: Any,
    raw_exit_codes: Any,
    raw_timeout: Any,
    raw_force_stop: Any,
    *,
    label: str,
    repo: Path,
) -> dict[str, Any]:
    argv = _validate_argv(raw_argv, f"{label} argv")
    cwd = _absolute_path(raw_cwd, f"{label} cwd")
    _reject_link_chain(cwd)
    if not cwd.is_dir():
        raise GateError(f"{label} cwd is missing")
    if not _is_under(cwd, repo):
        raise GateError(f"{label} cwd escapes the release checkout")
    executable = _resolve_executable(argv, cwd)
    lowered = [item.casefold() for item in argv]
    if "-file" in lowered:
        index = lowered.index("-file")
        if index + 1 >= len(argv):
            raise GateError(f"{label} -File is missing its script path")
        script = Path(argv[index + 1])
        script = (script if script.is_absolute() else cwd / script).resolve()
        if not _is_under(script, repo):
            raise GateError(f"{label} script path escapes the release checkout")
        _reject_link_chain(script)
        if not script.is_file():
            raise GateError(f"{label} script is missing")
    return {
        "argv": argv,
        "cwd": cwd,
        "executable": executable,
        "expected_exit_codes": _validate_exit_codes(raw_exit_codes, f"{label} expected_exit_codes"),
        "timeout_seconds": _validate_duration(raw_timeout, f"{label} timeout_seconds", 86_400),
        "force_stop_seconds": _validate_duration(
            raw_force_stop, f"{label} force_stop_seconds", 300
        ),
    }


def _walk_strings(value: Any, location: str = "$") -> Iterable[tuple[str, str]]:
    if isinstance(value, str):
        yield location, value
    elif isinstance(value, list):
        for index, item in enumerate(value):
            yield from _walk_strings(item, f"{location}[{index}]")
    elif isinstance(value, dict):
        for key, item in value.items():
            yield f"{location}.<key>", str(key)
            yield from _walk_strings(item, f"{location}.{key}")


def _reject_forbidden_content(value: Any) -> None:
    for location, text in _walk_strings(value):
        folded = text.replace("\\", "/").casefold()
        if TOKEN_RE.search(text) or GENERIC_PLACEHOLDER_RE.search(text):
            raise GateError(f"unresolved placeholder found at {location}")
        if OLD_WORKTREE_MARKER in folded:
            raise GateError(f"obsolete worktree path found at {location}")
        if OLD_RUN_ID.casefold() in folded:
            raise GateError(f"obsolete run id found at {location}")
        if OLD_CANDIDATE_SHA256.casefold() in folded:
            raise GateError(f"obsolete candidate sha256 found at {location}")


def _validate_pointer(pointer: Any, label: str) -> str:
    text = require_string(pointer, label)
    if not text.startswith("/"):
        raise GateError(f"{label} must be an RFC 6901 JSON Pointer")
    for part in text.split("/")[1:]:
        if re.search(r"~(?:[^01]|$)", part):
            raise GateError(f"{label} contains an invalid JSON Pointer escape")
    return text


def _validate_assertions(
    value: Any,
    label: str,
    evidence: Mapping[str, set[str]],
) -> list[dict[str, Any]]:
    raw = require_list(value, label)
    if not raw:
        raise GateError(f"{label} must not be empty")
    result: list[dict[str, Any]] = []
    ids: list[str] = []
    for index, item in enumerate(raw):
        assertion = require_mapping(item, f"{label}[{index}]")
        op_value = assertion.get("op")
        expected_fields = {"id", "source", "path", "pointer", "op"}
        if op_value not in {"exists", "absent", "truthy", "falsy"}:
            expected_fields.add("value")
        _require_exact_keys(assertion, expected_fields, f"{label}[{index}]")
        assertion_id = _safe_id(assertion.get("id"), f"{label}[{index}] id")
        source = require_string(assertion.get("source"), f"{label}[{index}] source")
        if source not in {"public", "private"}:
            raise GateError(f"{label}[{index}] source must be public or private")
        path = normalize_relative_path(assertion.get("path"), f"{label}[{index}] path")
        if path not in evidence[source]:
            raise GateError(f"{label}[{index}] path is not declared evidence")
        pointer = _validate_pointer(assertion.get("pointer"), f"{label}[{index}] pointer")
        op = require_string(assertion.get("op"), f"{label}[{index}] op")
        if op not in ASSERTION_OPS:
            raise GateError(f"{label}[{index}] uses unsupported assertion op {op!r}")
        if op not in {"exists", "absent", "truthy", "falsy"} and "value" not in assertion:
            raise GateError(f"{label}[{index}] requires a value")
        result.append(
            {
                "id": assertion_id,
                "source": source,
                "path": path,
                "pointer": pointer,
                "op": op,
                **({"value": assertion.get("value")} if "value" in assertion else {}),
            }
        )
        ids.append(assertion_id)
    if len(ids) != len(set(ids)):
        raise GateError(f"{label} contains duplicate assertion ids")
    return result


def _dependency_cycles(dependencies: Mapping[str, Sequence[str]]) -> None:
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(case_id: str) -> None:
        if case_id in visiting:
            raise GateError("acceptance dependencies contain a cycle")
        if case_id in visited:
            return
        visiting.add(case_id)
        for dependency in dependencies[case_id]:
            visit(dependency)
        visiting.remove(case_id)
        visited.add(case_id)

    for case_id in dependencies:
        visit(case_id)


def _formal_assertion_contract(
    case_id: str,
    run_key: str,
    *,
    source_commit: str,
    candidate_sha256: str,
    build_manifest_sha256: str,
    run_id: str,
    producer_nonce: str,
    run_registration_sha256: str,
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    path = f"{case_id}/result.public.json"
    pass_assertions: list[dict[str, Any]] = [
        {"id": "schema-version", "source": "public", "path": path, "pointer": "/schema_version", "op": "eq", "value": 1},
        {"id": "result-stage", "source": "public", "path": path, "pointer": "/stage", "op": "eq", "value": "MOSS_FUNCTIONAL_FT_RESULT"},
        {"id": "case-id", "source": "public", "path": path, "pointer": "/id", "op": "eq", "value": case_id},
        {"id": "producer-key", "source": "public", "path": path, "pointer": "/producer_key", "op": "eq", "value": run_key},
        {"id": "run-id", "source": "public", "path": path, "pointer": "/run_id", "op": "eq", "value": run_id},
        {"id": "machine-run-id", "source": "public", "path": path, "pointer": "/machine_run_id", "op": "eq", "value": run_id},
        {"id": "source-commit", "source": "public", "path": path, "pointer": "/source_commit", "op": "eq", "value": source_commit},
        {"id": "candidate-sha256", "source": "public", "path": path, "pointer": "/candidate_sha256", "op": "eq", "value": candidate_sha256},
        {"id": "build-manifest-sha256", "source": "public", "path": path, "pointer": "/build_manifest_sha256", "op": "eq", "value": build_manifest_sha256},
        {"id": "producer-nonce", "source": "public", "path": path, "pointer": "/producer_nonce", "op": "eq", "value": producer_nonce},
        {"id": "run-registration", "source": "public", "path": path, "pointer": "/run_registration_sha256", "op": "eq", "value": run_registration_sha256},
        {"id": "produced-at", "source": "public", "path": path, "pointer": "/produced_at", "op": "exists"},
    ]
    fail_assertions: list[dict[str, Any]] = []
    for check in (*FORMAL_COMMON_CHECK_KEYS, *FORMAL_CHECK_KEYS_BY_ID[case_id]):
        pointer = f"/checks/{check}"
        pass_assertions.append(
            {"id": f"fact-{check}", "source": "public", "path": path, "pointer": pointer, "op": "eq", "value": True}
        )
        fail_assertions.append(
            {"id": f"failed-{check}", "source": "public", "path": path, "pointer": pointer, "op": "eq", "value": False}
        )
    extra: dict[str, list[dict[str, Any]]] = {
        "FT-01": [
            {"id": "metric-live-changes", "source": "public", "path": path, "pointer": "/metrics/nonempty_live_change_count", "op": "ge", "value": 2},
            {"id": "metric-final-segments", "source": "public", "path": path, "pointer": "/metrics/final_segment_count", "op": "gt", "value": 0},
            {"id": "metric-tail-coverage", "source": "public", "path": path, "pointer": "/metrics/tail_coverage_ratio", "op": "ge", "value": 0.98},
            {"id": "metric-finalization-queue-empty", "source": "public", "path": path, "pointer": "/metrics/chunks_in_queue_at_finalization", "op": "eq", "value": 0},
        ],
        "FT-02": [
            {"id": "metric-stop-feedback", "source": "public", "path": path, "pointer": "/metrics/stop_feedback_seconds", "op": "le", "value": 1.0},
            {"id": "metric-page-unlock", "source": "public", "path": path, "pointer": "/metrics/page_unlock_seconds", "op": "le", "value": 10.0},
            {"id": "metric-auto-full-retranscription", "source": "public", "path": path, "pointer": "/metrics/automatic_full_retranscription_count", "op": "eq", "value": 0},
            {"id": "metric-finalization-queue-empty", "source": "public", "path": path, "pointer": "/metrics/chunks_in_queue_at_finalization", "op": "eq", "value": 0},
        ],
        "FT-03": [
            {"id": "metric-two-summaries", "source": "public", "path": path, "pointer": "/metrics/completed_summary_count", "op": "ge", "value": 2}
        ],
        "FT-04": [
            {"id": "metric-enhance-feedback", "source": "public", "path": path, "pointer": "/metrics/enhance_feedback_seconds", "op": "le", "value": 1.0},
            {"id": "metric-whisper-enhance-complete", "source": "public", "path": path, "pointer": "/metrics/whisper_enhance_elapsed_seconds", "op": "le", "value": 300.0},
            {"id": "metric-auto-batch-enhancement", "source": "public", "path": path, "pointer": "/metrics/automatic_batch_enhancement_count", "op": "eq", "value": 0},
        ],
        "FT-05": [
            {"id": "metric-cancel-feedback", "source": "public", "path": path, "pointer": "/metrics/cancel_feedback_seconds", "op": "le", "value": 1.0},
            {"id": "metric-residual-processes", "source": "public", "path": path, "pointer": "/metrics/residual_process_count", "op": "eq", "value": 0},
            {"id": "metric-helper-exit", "source": "public", "path": path, "pointer": "/metrics/helper_exit_seconds", "op": "le", "value": 5.0},
        ],
        "FT-12": [
            {"id": "metric-summary-feedback", "source": "public", "path": path, "pointer": "/metrics/summary_feedback_seconds", "op": "le", "value": 1.0},
            {"id": "metric-summary-complete", "source": "public", "path": path, "pointer": "/metrics/summary_elapsed_seconds", "op": "le", "value": 180.0},
            {"id": "metric-residual-processes", "source": "public", "path": path, "pointer": "/metrics/residual_process_count", "op": "eq", "value": 0},
            {"id": "metric-helper-exit", "source": "public", "path": path, "pointer": "/metrics/helper_exit_seconds", "op": "le", "value": 5.0},
        ],
        "FT-26": [
            {"id": "metric-selected-files", "source": "public", "path": path, "pointer": "/metrics/selected_file_count", "op": "gt", "value": 0}
        ],
        "FT-27": [
            {"id": "metric-duration", "source": "public", "path": path, "pointer": "/metrics/expected_duration_seconds", "op": "eq", "value": 3096.62},
            {"id": "metric-segments", "source": "public", "path": path, "pointer": "/metrics/candidate_segment_count", "op": "gt", "value": 0},
            {"id": "metric-tail", "source": "public", "path": path, "pointer": "/metrics/tail_difference_seconds", "op": "le", "value": 0.5},
            {"id": "metric-tail-coverage", "source": "public", "path": path, "pointer": "/metrics/tail_coverage_ratio", "op": "ge", "value": 0.98},
            {"id": "metric-finalization-queue-empty", "source": "public", "path": path, "pointer": "/metrics/chunks_in_queue_at_finalization", "op": "eq", "value": 0},
            {"id": "metric-residual-processes", "source": "public", "path": path, "pointer": "/metrics/residual_process_count", "op": "eq", "value": 0},
            {"id": "metric-moss-qwen-overlap", "source": "public", "path": path, "pointer": "/metrics/moss_and_qwen_overlap_seconds", "op": "eq", "value": 0},
        ],
        "FT-28": [
            {"id": "metric-business-duration", "source": "public", "path": path, "pointer": "/metrics/business_audio_duration_seconds", "op": "eq", "value": 737.728},
            {"id": "metric-quality-window-duration", "source": "public", "path": path, "pointer": "/metrics/q00_window_duration_seconds", "op": "eq", "value": 226.44},
            {"id": "metric-moss-cer", "source": "public", "path": path, "pointer": "/metrics/q00_moss_raw_cer", "op": "le", "value": 0.20},
            {"id": "metric-corrected-cer", "source": "public", "path": path, "pointer": "/metrics/q00_corrected_cer", "op": "le", "value": 0.15},
            {"id": "metric-positive-terms", "source": "public", "path": path, "pointer": "/metrics/q00_positive_term_accuracy", "op": "ge", "value": 0.95},
            {"id": "metric-negative-insertions", "source": "public", "path": path, "pointer": "/metrics/q00_negative_term_insertions", "op": "eq", "value": 0},
            {"id": "metric-rtf", "source": "public", "path": path, "pointer": "/metrics/q00_moss_rtf", "op": "le", "value": 1.0},
            {"id": "metric-speaker-coverage", "source": "public", "path": path, "pointer": "/metrics/q00_manual_single_segment_coverage_error_count", "op": "eq", "value": 0},
            {"id": "metric-tail-coverage", "source": "public", "path": path, "pointer": "/metrics/tail_coverage_ratio", "op": "ge", "value": 0.98},
            {"id": "metric-finalization-queue-empty", "source": "public", "path": path, "pointer": "/metrics/chunks_in_queue_at_finalization", "op": "eq", "value": 0},
            {"id": "metric-residual-processes", "source": "public", "path": path, "pointer": "/metrics/residual_process_count", "op": "eq", "value": 0},
            {"id": "metric-moss-qwen-overlap", "source": "public", "path": path, "pointer": "/metrics/moss_and_qwen_overlap_seconds", "op": "eq", "value": 0},
        ],
    }
    if case_id in {f"FT-{number:02d}" for number in range(16, 22)}:
        extra[case_id] = [
            {"id": "metric-fault-case", "source": "public", "path": path, "pointer": "/metrics/case_metrics", "op": "truthy"},
            {"id": "metric-residual-processes", "source": "public", "path": path, "pointer": "/metrics/residual_process_count", "op": "eq", "value": 0},
            {"id": "metric-helper-exit", "source": "public", "path": path, "pointer": "/metrics/helper_exit_seconds", "op": "le", "value": 5.0},
            {"id": "metric-moss-qwen-overlap", "source": "public", "path": path, "pointer": "/metrics/moss_and_qwen_overlap_seconds", "op": "eq", "value": 0},
        ]
    if case_id in {f"FT-{number:02d}" for number in range(22, 26)}:
        extra[case_id] = [
            {"id": "metric-lifecycle-schema", "source": "public", "path": path, "pointer": "/metrics/lifecycle_schema_version", "op": "eq", "value": 1},
            {"id": "metric-lifecycle-case", "source": "public", "path": path, "pointer": "/metrics/lifecycle_case/status", "op": "eq", "value": PASS},
        ]
    pass_assertions.extend(extra.get(case_id, []))
    return pass_assertions, fail_assertions


def _template_shape(
    template: Mapping[str, Any],
    required_ids: Sequence[str],
    required_names: Mapping[str, str],
    expected_run_keys: Mapping[str, str],
    *,
    repo: Path | None = None,
) -> None:
    formal = _is_formal_contract(required_ids, required_names, expected_run_keys)
    expected_template_fields = {
        "schema_version",
        "stage",
        "template_only",
        "source_commit",
        "run_id",
        "machine_run_id",
        "execution_contract",
        "candidate",
        "build_manifest",
        "evidence_roots",
        "statistics",
        "scenarios",
    }
    if formal:
        expected_template_fields.update(
            {"schema_bindings", "run_registry", "producer_nonces", "cua_contract"}
        )
    _require_exact_keys(
        template,
        expected_template_fields,
        "acceptance template",
    )
    if template.get("schema_version") != SCHEMA_VERSION or template.get("stage") != CONFIG_STAGE:
        raise GateError("acceptance template schema/stage is invalid")
    if template.get("template_only") is not True:
        raise GateError("acceptance template must set template_only=true")
    if template.get("machine_run_id") != template.get("run_id"):
        raise GateError("acceptance template machine_run_id must equal run_id")
    _require_execution_contract(template.get("execution_contract"))
    if formal:
        template_repo = (repo or Path(__file__).resolve().parents[2]).resolve()
        _require_schema_bindings(template.get("schema_bindings"), template_repo)
        _require_cua_contract(template.get("cua_contract"))
        template_nonces = require_mapping(template.get("producer_nonces"), "template producer_nonces")
        if list(template_nonces) != list(FORMAL_RUN_KEYS) or any(
            value != "MATERIALIZED_AT_RUNTIME" for value in template_nonces.values()
        ):
            raise GateError("acceptance template producer nonce placeholders are not exact")
        registry = require_mapping(template.get("run_registry"), "template run_registry")
        _require_exact_keys(registry, {"directory", "registration"}, "template run_registry")
        if registry.get("directory") != "MATERIALIZED_AT_RUNTIME":
            raise GateError("acceptance template run registry placeholder is not exact")
        registration = require_mapping(
            registry.get("registration"), "template run registration"
        )
        if registration != {
            "path": "MATERIALIZED_AT_RUNTIME",
            "bytes": 0,
            "sha256": "0" * 64,
        }:
            raise GateError("acceptance template run registration placeholder is not exact")
    scenarios = [
        require_mapping(item, "acceptance template scenario")
        for item in require_list(template.get("scenarios"), "acceptance template scenarios")
    ]
    ids = [item.get("id") for item in scenarios]
    if ids != list(required_ids):
        raise GateError("acceptance template must contain the required FT ids once in order")
    required_fields = {
        "id",
        "name",
        "run_once_key",
        "source_commit",
        "candidate_sha256",
        "argv",
        "cwd",
        "expected_exit_codes",
        "inputs",
        "timeout_seconds",
        "force_stop_seconds",
        "dependencies",
        "pass_assertions",
        "fail_assertions",
        "evidence",
        "cleanup",
    }
    candidate = require_mapping(template.get("candidate"), "acceptance template candidate")
    _require_exact_keys(candidate, {"path", "bytes", "sha256"}, "acceptance template candidate")
    build_manifest = require_mapping(template.get("build_manifest"), "acceptance template build manifest")
    _require_exact_keys(build_manifest, {"path", "bytes", "sha256"}, "acceptance template build manifest")
    roots = require_mapping(template.get("evidence_roots"), "acceptance template evidence roots")
    _require_exact_keys(roots, {"public", "private"}, "acceptance template evidence roots")
    statistics = require_mapping(template.get("statistics"), "acceptance template statistics")
    _require_exact_keys(statistics, {"expected_total", "run_once_total"}, "acceptance template statistics")
    for scenario in scenarios:
        case_id = str(scenario["id"])
        if scenario.get("name") != required_names[case_id]:
            raise GateError(f"acceptance template name mismatch for {case_id}")
        if scenario.get("run_once_key") != expected_run_keys[case_id]:
            raise GateError(f"acceptance template run_once_key mismatch for {case_id}")
        _require_exact_keys(scenario, required_fields, f"acceptance template {case_id}")
        evidence = require_mapping(scenario.get("evidence"), f"acceptance template {case_id} evidence")
        _require_exact_keys(evidence, {"public", "private"}, f"acceptance template {case_id} evidence")
        cleanup = require_mapping(scenario.get("cleanup"), f"acceptance template {case_id} cleanup")
        _require_exact_keys(
            cleanup,
            {"argv", "cwd", "expected_exit_codes", "timeout_seconds", "force_stop_seconds", "checks"},
            f"acceptance template {case_id} cleanup",
        )
        if formal:
            run_key = expected_run_keys[case_id]
            contract = FORMAL_GROUP_CONTRACTS[run_key]
            if scenario.get("dependencies") != FORMAL_DEPENDENCIES_BY_ID[case_id]:
                raise GateError(f"acceptance template {case_id} dependencies are not exact")
            if scenario.get("argv") != _formal_template_command_argv(run_key, "Run"):
                raise GateError(f"acceptance template {case_id} producer command is not exact")
            if scenario.get("cwd") != "__REPO_ROOT__":
                raise GateError(f"acceptance template {case_id} cwd is not fixed")
            if scenario.get("expected_exit_codes") != [0]:
                raise GateError(f"acceptance template {case_id} exit-code contract is not exact")
            if scenario.get("timeout_seconds") != contract["timeout_seconds"]:
                raise GateError(f"acceptance template {case_id} timeout is not exact")
            if scenario.get("force_stop_seconds") != FORMAL_FORCE_STOP_SECONDS:
                raise GateError(f"acceptance template {case_id} force-stop timeout is not exact")
            if len(require_list(scenario.get("inputs"), f"acceptance template {case_id} inputs")) != contract["input_count"]:
                raise GateError(f"acceptance template {case_id} input count is not exact")
            if cleanup.get("argv") != _formal_template_command_argv(run_key, "Cleanup"):
                raise GateError(f"acceptance template {case_id} cleanup command is not exact")
            if cleanup.get("cwd") != "__REPO_ROOT__":
                raise GateError(f"acceptance template {case_id} cleanup cwd is not fixed")
            if cleanup.get("expected_exit_codes") != [0]:
                raise GateError(f"acceptance template {case_id} cleanup exit-code contract is not exact")
            if cleanup.get("timeout_seconds") != FORMAL_CLEANUP_TIMEOUT_SECONDS:
                raise GateError(f"acceptance template {case_id} cleanup timeout is not exact")
            if cleanup.get("force_stop_seconds") != FORMAL_FORCE_STOP_SECONDS:
                raise GateError(f"acceptance template {case_id} cleanup force-stop timeout is not exact")
            if cleanup.get("checks") != _formal_cleanup_assertions(run_key):
                raise GateError(f"acceptance template {case_id} cleanup assertions are not exact")


def validate_config(
    config_path: Path,
    repo: Path,
    *,
    required_ids: Sequence[str] = FORMAL_CASE_IDS,
    required_names: Mapping[str, str] | None = None,
    expected_run_keys: Mapping[str, str] | None = None,
    require_external_config: bool = True,
) -> dict[str, Any]:
    """Validate and normalize a materialized acceptance config.

    The optional contracts exist only so the unit tests can use tiny fixtures.
    All CLI commands call this function with the formal 28-case contract.
    """

    repo = repo.resolve()
    config_path = config_path.resolve()
    required_names = required_names or FORMAL_CASE_NAMES
    expected_run_keys = expected_run_keys or FORMAL_RUN_KEY_BY_ID
    formal = _is_formal_contract(required_ids, required_names, expected_run_keys)
    repository_binding = (
        _assert_repository_clean(repo, "formal acceptance validation") if formal else None
    )
    document, config_sha256, config_bytes = _stable_json(config_path, "acceptance config")
    _reject_forbidden_content(document)
    if formal:
        _require_exact_keys(
            document,
            {
                "schema_version",
                "stage",
                "template_only",
                "generated_at",
                "materialized_from_template_sha256",
                "source_commit",
                "run_id",
                "machine_run_id",
                "execution_contract",
                "schema_bindings",
                "run_registry",
                "producer_nonces",
                "cua_contract",
                "candidate",
                "build_manifest",
                "evidence_roots",
                "statistics",
                "scenarios",
            },
            "formal acceptance config",
        )
        require_string(document.get("generated_at"), "formal acceptance generated_at")
        template_path = _formal_template_path(repo)
        template_binding = _tracked_workspace_binding(
            repo, FORMAL_TEMPLATE_RELATIVE_PATH, "formal acceptance template"
        )
        if (
            require_sha256(
                document.get("materialized_from_template_sha256"),
                "materialized_from_template_sha256",
            )
            != template_binding["workspace_sha256"]
        ):
            raise GateError("formal acceptance config was not materialized from the current fixed template")
        _stable_json(template_path, "formal acceptance template")
    if require_external_config and _is_under(config_path, repo):
        raise GateError("materialized acceptance.json must be stored outside the repository")
    if document.get("schema_version") != SCHEMA_VERSION or document.get("stage") != CONFIG_STAGE:
        raise GateError("acceptance config schema/stage is invalid")
    if document.get("template_only") is not False:
        raise GateError("acceptance template cannot be executed; materialize it first")
    head = _git_head(repo)
    source_commit = require_git_commit(document.get("source_commit"), "source_commit")
    if source_commit != head:
        raise GateError("acceptance source_commit does not match current repository HEAD")
    run_id = _safe_id(document.get("run_id"), "run_id")
    machine_run_id = _safe_id(document.get("machine_run_id"), "machine_run_id")
    if machine_run_id != run_id:
        raise GateError("machine_run_id must equal run_id for the formal machine session")
    execution_contract = _require_execution_contract(document.get("execution_contract"))
    schema_bindings = (
        _require_schema_bindings(document.get("schema_bindings"), repo) if formal else None
    )
    cua_contract = _require_cua_contract(document.get("cua_contract")) if formal else None
    producer_nonces = (
        _validate_producer_nonces(document.get("producer_nonces")) if formal else {}
    )
    candidate = _bound_file(document.get("candidate"), "candidate installer")
    if candidate["sha256"] == OLD_CANDIDATE_SHA256:
        raise GateError("obsolete candidate installer cannot be accepted")
    build_manifest = _bound_file(document.get("build_manifest"), "build manifest")
    build_manifest_details = (
        _validate_build_manifest(build_manifest, candidate, repo, source_commit)
        if formal
        else None
    )
    statistics = require_mapping(document.get("statistics"), "statistics")
    if statistics.get("expected_total") != len(required_ids):
        raise GateError("acceptance statistics expected_total is incorrect")
    expected_key_count = len(set(expected_run_keys.values()))
    if statistics.get("run_once_total") != expected_key_count:
        raise GateError("acceptance statistics run_once_total is incorrect")

    roots = require_mapping(document.get("evidence_roots"), "evidence_roots")
    public_root = _absolute_path(roots.get("public"), "public evidence root")
    private_root = _absolute_path(roots.get("private"), "private evidence root")
    for root, label in ((public_root, "public evidence root"), (private_root, "private evidence root")):
        _reject_link_chain(root)
        if not root.is_dir():
            raise GateError(f"{label} is missing")
    if public_root == private_root or _is_under(public_root, private_root) or _is_under(
        private_root, public_root
    ):
        raise GateError("public and private evidence roots must be separate non-nested directories")
    if formal and (
        _is_under(public_root, repo) or _is_under(private_root, repo)
    ):
        raise GateError("formal evidence roots must be outside the repository")
    run_registration = (
        _validate_run_registry(
            document.get("run_registry"),
            repo=repo,
            run_id=run_id,
            source_commit=source_commit,
            candidate_sha256=candidate["sha256"],
            build_manifest_sha256=build_manifest["sha256"],
            public_root=public_root,
            private_root=private_root,
        )
        if formal
        else None
    )

    raw_scenarios = [
        require_mapping(item, "acceptance scenario")
        for item in require_list(document.get("scenarios"), "acceptance scenarios")
    ]
    ids = [require_string(item.get("id"), "scenario id") for item in raw_scenarios]
    if len(ids) != len(set(ids)):
        raise GateError("acceptance scenario ids contain duplicates")
    if set(ids) != set(required_ids) or len(ids) != len(required_ids):
        raise GateError("acceptance scenario numbering is incomplete or unexpected")
    if ids != list(required_ids):
        raise GateError("acceptance scenarios must be ordered by FT number")

    normalized: list[dict[str, Any]] = []
    case_position = {case_id: index for index, case_id in enumerate(required_ids)}
    dependencies: dict[str, list[str]] = {}
    shared_by_key: dict[str, bytes] = {}
    evidence_owner: dict[tuple[str, str], str] = {}
    for raw in raw_scenarios:
        case_id = require_string(raw.get("id"), "scenario id")
        name = require_string(raw.get("name"), f"{case_id} name")
        if required_names.get(case_id) != name:
            raise GateError(f"{case_id} name does not match REQUIRED_P6_SCENARIOS")
        run_key = _safe_id(raw.get("run_once_key"), f"{case_id} run_once_key")
        if expected_run_keys.get(case_id) != run_key:
            raise GateError(f"{case_id} has the wrong run_once_key")
        scenario_commit = require_git_commit(raw.get("source_commit"), f"{case_id} source_commit")
        if scenario_commit != source_commit:
            raise GateError(f"{case_id} source_commit does not match the config")
        scenario_candidate = require_sha256(
            raw.get("candidate_sha256"), f"{case_id} candidate_sha256"
        )
        if scenario_candidate != candidate["sha256"]:
            raise GateError(f"{case_id} candidate_sha256 does not match the real candidate")
        if formal:
            _require_formal_scenario_contract(
                raw,
                case_id=case_id,
                run_key=run_key,
                config_path=config_path,
                repo=repo,
                source_commit=source_commit,
                candidate_sha256=candidate["sha256"],
                build_manifest_sha256=build_manifest["sha256"],
                run_id=run_id,
                producer_nonce=producer_nonces[run_key],
                run_registration_sha256=run_registration["sha256"],
            )

        evidence_raw = require_mapping(raw.get("evidence"), f"{case_id} evidence")
        evidence: dict[str, set[str]] = {}
        evidence_paths: dict[str, list[tuple[Path, str]]] = {}
        for source, root in (("public", public_root), ("private", private_root)):
            raw_paths = require_list(evidence_raw.get(source), f"{case_id} {source} evidence")
            if not raw_paths:
                raise GateError(f"{case_id} must declare at least one {source} evidence file")
            normalized_paths = [
                normalize_relative_path(item, f"{case_id} {source} evidence path")
                for item in raw_paths
            ]
            if len(normalized_paths) != len(set(normalized_paths)):
                raise GateError(f"{case_id} has duplicate {source} evidence paths")
            evidence[source] = set(normalized_paths)
            evidence_paths[source] = []
            for relative in normalized_paths:
                path, normalized_relative = resolve_under(
                    root, relative, f"{case_id} {source} evidence path"
                )
                evidence_paths[source].append((path, normalized_relative))
                owner_key = evidence_owner.setdefault((source, normalized_relative), run_key)
                if owner_key != run_key:
                    raise GateError("an evidence path is shared by different run_once groups")

        command = _validate_command(
            raw.get("argv"),
            raw.get("cwd"),
            raw.get("expected_exit_codes"),
            raw.get("timeout_seconds"),
            raw.get("force_stop_seconds"),
            label=case_id,
            repo=repo,
        )
        raw_inputs = require_list(raw.get("inputs"), f"{case_id} inputs")
        if not raw_inputs:
            raise GateError(f"{case_id} inputs must not be empty")
        inputs = [_bound_file(item, f"{case_id} input[{index}]") for index, item in enumerate(raw_inputs)]
        input_paths = [str(item["path"]).casefold() for item in inputs]
        if len(input_paths) != len(set(input_paths)):
            raise GateError(f"{case_id} contains duplicate input files")
        if formal:
            expected_inputs = [
                FORMAL_INPUT_RECORDS[role] for role in FORMAL_GROUP_INPUT_ROLES[run_key]
            ]
            actual_inputs = [(item["bytes"], item["sha256"]) for item in inputs]
            if actual_inputs != expected_inputs:
                raise GateError(f"{case_id} inputs do not match the frozen formal input set")

        raw_dependencies = require_list(raw.get("dependencies"), f"{case_id} dependencies")
        if any(not isinstance(item, str) or not item for item in raw_dependencies):
            raise GateError(f"{case_id} dependencies must be strings")
        scenario_dependencies = list(raw_dependencies)
        if formal and scenario_dependencies != FORMAL_DEPENDENCIES_BY_ID[case_id]:
            raise GateError(f"{case_id} dependencies do not match the fixed formal order")
        if len(scenario_dependencies) != len(set(scenario_dependencies)):
            raise GateError(f"{case_id} contains duplicate dependencies")
        if case_id in scenario_dependencies:
            raise GateError(f"{case_id} cannot depend on itself")
        unknown = set(scenario_dependencies) - set(required_ids)
        if unknown:
            raise GateError(f"{case_id} contains missing dependencies: {sorted(unknown)}")
        if any(case_position[item] >= case_position[case_id] for item in scenario_dependencies):
            raise GateError(f"{case_id} dependencies must refer to earlier FT cases")
        dependencies[case_id] = scenario_dependencies

        pass_assertions = _validate_assertions(
            raw.get("pass_assertions"), f"{case_id} pass_assertions", evidence
        )
        fail_assertions = _validate_assertions(
            raw.get("fail_assertions"), f"{case_id} fail_assertions", evidence
        )
        cleanup_raw = require_mapping(raw.get("cleanup"), f"{case_id} cleanup")
        cleanup = _validate_command(
            cleanup_raw.get("argv"),
            cleanup_raw.get("cwd"),
            cleanup_raw.get("expected_exit_codes"),
            cleanup_raw.get("timeout_seconds"),
            cleanup_raw.get("force_stop_seconds"),
            label=f"{case_id} cleanup",
            repo=repo,
        )
        cleanup_checks = _validate_assertions(
            cleanup_raw.get("checks"), f"{case_id} cleanup checks", evidence
        )
        cleanup["checks"] = cleanup_checks

        shared_value = {field: raw.get(field) for field in SHARED_FIELDS}
        shared_hash = canonical_json_bytes(shared_value)
        previous_shared = shared_by_key.setdefault(run_key, shared_hash)
        if previous_shared != shared_hash:
            raise GateError(f"run_once_key {run_key} has inconsistent shared definitions")
        member_ids = {item["id"] for item in normalized if item["run_once_key"] == run_key}
        if member_ids.intersection(scenario_dependencies):
            raise GateError(f"{case_id} cannot depend on another case in its run_once group")

        normalized.append(
            {
                "id": case_id,
                "name": name,
                "run_once_key": run_key,
                "source_commit": scenario_commit,
                "candidate_sha256": scenario_candidate,
                "command": command,
                "inputs": inputs,
                "dependencies": scenario_dependencies,
                "pass_assertions": pass_assertions,
                "fail_assertions": fail_assertions,
                "evidence": evidence_paths,
                "cleanup": cleanup,
                "raw": raw,
            }
        )

    _dependency_cycles(dependencies)
    group_members: dict[str, set[str]] = {}
    for item in normalized:
        group_members.setdefault(item["run_once_key"], set()).add(item["id"])
    for item in normalized:
        if group_members[item["run_once_key"]].intersection(item["dependencies"]):
            raise GateError(f"{item['id']} depends on its own run_once group")
    if set(shared_by_key) != set(expected_run_keys.values()):
        raise GateError("acceptance run_once groups do not match the required seven producers")

    return {
        "path": config_path,
        "document": document,
        "sha256": config_sha256,
        "bytes": config_bytes,
        "repo": repo,
        "source_commit": source_commit,
        "run_id": run_id,
        "machine_run_id": machine_run_id,
        "execution_contract": execution_contract,
        "schema_bindings": schema_bindings,
        "cua_contract": cua_contract,
        "producer_nonces": producer_nonces,
        "run_registration": run_registration,
        "formal": formal,
        "candidate": candidate,
        "build_manifest": build_manifest,
        "build_manifest_details": build_manifest_details,
        "public_root": public_root,
        "private_root": private_root,
        "scenarios": normalized,
        "scenario_by_id": {item["id"]: item for item in normalized},
        "required_ids": tuple(required_ids),
        "run_keys": tuple(dict.fromkeys(item["run_once_key"] for item in normalized)),
        "repository_binding": repository_binding,
    }


def _token_values(values: Sequence[str], token_json: Path | None) -> dict[str, str]:
    tokens: dict[str, str] = {}
    if token_json is not None:
        raw = require_mapping(read_json(token_json.resolve(), "token JSON"), "token JSON")
        for name, value in raw.items():
            if not TOKEN_NAME_RE.fullmatch(str(name)) or not isinstance(value, str) or not value:
                raise GateError("token JSON must map uppercase token names to non-empty strings")
            tokens[str(name)] = value
    for item in values:
        if "=" not in item:
            raise GateError("--token must use NAME=VALUE")
        name, value = item.split("=", 1)
        if not TOKEN_NAME_RE.fullmatch(name) or not value:
            raise GateError("--token must use an uppercase token name and non-empty value")
        if name in tokens:
            raise GateError(f"duplicate token {name}")
        tokens[name] = value
    return tokens


def _replace_tokens(value: Any, tokens: Mapping[str, str]) -> Any:
    if isinstance(value, str):
        result = value
        for name, replacement in tokens.items():
            result = result.replace(f"__{name}__", replacement)
        return result
    if isinstance(value, list):
        return [_replace_tokens(item, tokens) for item in value]
    if isinstance(value, dict):
        return {key: _replace_tokens(item, tokens) for key, item in value.items()}
    return value


def materialize_config(
    *,
    repo: Path,
    template_path: Path,
    output_path: Path,
    candidate_path: Path,
    build_manifest_path: Path,
    run_id: str,
    public_root: Path,
    private_root: Path,
    tokens: Mapping[str, str],
    run_registry_root: Path | None = None,
    required_ids: Sequence[str] = FORMAL_CASE_IDS,
    required_names: Mapping[str, str] | None = None,
    expected_run_keys: Mapping[str, str] | None = None,
    require_external_config: bool = True,
) -> dict[str, Any]:
    repo = repo.resolve()
    output_path = Path(os.path.abspath(output_path))
    required_names = required_names or FORMAL_CASE_NAMES
    expected_run_keys = expected_run_keys or FORMAL_RUN_KEY_BY_ID
    formal = _is_formal_contract(required_ids, required_names, expected_run_keys)
    if formal:
        _assert_repository_clean(repo, "formal acceptance materialization")
    if output_path.exists():
        raise GateError("materialized acceptance output already exists; use a new run file")
    if require_external_config and _is_under(output_path, repo):
        raise GateError("materialized acceptance.json must be written outside the repository")
    _safe_id(run_id, "run_id")
    if run_id.casefold() == OLD_RUN_ID.casefold():
        raise GateError("obsolete run id cannot be reused")
    if formal:
        template_path = _require_formal_template_path(repo, template_path)
        _tracked_workspace_binding(repo, FORMAL_TEMPLATE_RELATIVE_PATH, "formal acceptance template")
    template, template_sha, _ = _stable_json(template_path.resolve(), "acceptance template")
    _template_shape(
        template,
        required_ids,
        required_names,
        expected_run_keys,
        repo=repo,
    )
    head = _git_head(repo)
    candidate = file_record(candidate_path.resolve(), relative_path=candidate_path.name)
    manifest = file_record(build_manifest_path.resolve(), relative_path=build_manifest_path.name)
    if candidate["sha256"] == OLD_CANDIDATE_SHA256:
        raise GateError("obsolete candidate installer cannot be materialized")
    if formal:
        _validate_build_manifest(
            {"path": build_manifest_path.resolve(), "bytes": manifest["bytes"], "sha256": manifest["sha256"]},
            {"path": candidate_path.resolve(), "bytes": candidate["bytes"], "sha256": candidate["sha256"]},
            repo,
            head,
        )

    public_root = Path(os.path.abspath(public_root))
    private_root = Path(os.path.abspath(private_root))
    for root in (public_root, private_root):
        root.mkdir(parents=True, exist_ok=True)
        _reject_link_chain(root)
    if public_root == private_root or _is_under(public_root, private_root) or _is_under(
        private_root, public_root
    ):
        raise GateError("public and private evidence roots must be separate non-nested directories")

    producer_nonces: dict[str, str] = {}
    run_registration: dict[str, Any] | None = None
    if formal:
        if run_registry_root is None:
            raise GateError("formal materialization requires --run-registry")
        registry_root = Path(os.path.abspath(run_registry_root))
        registration_path = _run_registration_entry_path(registry_root, run_id)
        registration_record = file_record(
            registration_path, relative_path=registration_path.name
        )
        run_registry_value = {
            "directory": str(registry_root),
            "registration": {
                "path": str(registration_path.resolve()),
                "bytes": registration_record["bytes"],
                "sha256": registration_record["sha256"],
            },
        }
        run_registration = _validate_run_registry(
            run_registry_value,
            repo=repo,
            run_id=run_id,
            source_commit=head,
            candidate_sha256=candidate["sha256"],
            build_manifest_sha256=manifest["sha256"],
            public_root=public_root,
            private_root=private_root,
        )
        producer_nonces = _validate_producer_nonces(
            {key: secrets.token_hex(32) for key in FORMAL_RUN_KEYS}
        )

    builtins = {
        "REPO_ROOT": str(repo),
        "SOURCE_COMMIT": head,
        "CANDIDATE_PATH": str(candidate_path.resolve()),
        "CANDIDATE_SHA256": candidate["sha256"],
        "BUILD_MANIFEST_PATH": str(build_manifest_path.resolve()),
        "BUILD_MANIFEST_SHA256": manifest["sha256"],
        "RUN_ID": run_id,
        "PUBLIC_EVIDENCE_ROOT": str(public_root),
        "PRIVATE_EVIDENCE_ROOT": str(private_root),
        "ACCEPTANCE_CONFIG": str(output_path),
    }
    overlap = set(tokens).intersection(builtins)
    if overlap:
        raise GateError(f"built-in tokens cannot be overridden: {sorted(overlap)}")
    if formal and set(tokens) != FORMAL_TOKEN_NAMES:
        raise GateError(
            "formal acceptance token set is incomplete or contains unapproved extra values"
        )
    document = require_mapping(_replace_tokens(template, {**tokens, **builtins}), "materialized config")
    document["template_only"] = False
    document["generated_at"] = utc_now()
    document["materialized_from_template_sha256"] = template_sha
    document["source_commit"] = head
    document["run_id"] = run_id
    document["machine_run_id"] = run_id
    document["execution_contract"] = FORMAL_EXECUTION_CONTRACT
    if formal:
        if run_registration is None:
            raise GateError("formal run registration was not loaded")
        document["schema_bindings"] = formal_schema_template_bindings(repo)
        document["run_registry"] = {
            "directory": str(run_registration["directory"]),
            "registration": {
                "path": str(run_registration["path"]),
                "bytes": run_registration["bytes"],
                "sha256": run_registration["sha256"],
            },
        }
        document["producer_nonces"] = producer_nonces
        document["cua_contract"] = FORMAL_CUA_CONTRACT
    document["candidate"] = {
        "path": str(candidate_path.resolve()),
        "bytes": candidate["bytes"],
        "sha256": candidate["sha256"],
    }
    document["build_manifest"] = {
        "path": str(build_manifest_path.resolve()),
        "bytes": manifest["bytes"],
        "sha256": manifest["sha256"],
    }
    document["evidence_roots"] = {"public": str(public_root), "private": str(private_root)}
    document["statistics"] = {
        "expected_total": len(required_ids),
        "run_once_total": len(set(expected_run_keys.values())),
    }
    for raw_scenario in require_list(document.get("scenarios"), "materialized scenarios"):
        scenario = require_mapping(raw_scenario, "materialized scenario")
        scenario["source_commit"] = head
        scenario["candidate_sha256"] = candidate["sha256"]
        inputs = require_list(scenario.get("inputs"), f"{scenario.get('id')} inputs")
        for index, raw_input in enumerate(inputs):
            input_record = require_mapping(raw_input, f"{scenario.get('id')} input[{index}]")
            input_path = _absolute_path(input_record.get("path"), "materialized input path")
            actual = file_record(input_path, relative_path=input_path.name)
            input_record["path"] = str(input_path)
            input_record["bytes"] = actual["bytes"]
            input_record["sha256"] = actual["sha256"]
        if formal:
            case_id = require_string(scenario.get("id"), "materialized scenario id")
            run_key = expected_run_keys[case_id]
            formal_pass, formal_fail = _formal_assertion_contract(
                case_id,
                run_key,
                source_commit=head,
                candidate_sha256=candidate["sha256"],
                build_manifest_sha256=manifest["sha256"],
                run_id=run_id,
                producer_nonce=producer_nonces[run_key],
                run_registration_sha256=run_registration["sha256"],
            )
            scenario["pass_assertions"] = formal_pass
            scenario["fail_assertions"] = formal_fail
    _reject_forbidden_content(document)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    _reject_link_chain(output_path.parent)
    atomic_write_json(output_path, document)
    return validate_config(
        output_path,
        repo,
        required_ids=required_ids,
        required_names=required_names,
        expected_run_keys=expected_run_keys,
        require_external_config=require_external_config,
    )


def _json_pointer(document: Any, pointer: str) -> tuple[bool, Any]:
    current = document
    for encoded in pointer.split("/")[1:]:
        token = encoded.replace("~1", "/").replace("~0", "~")
        if isinstance(current, dict) and token in current:
            current = current[token]
        elif isinstance(current, list) and token.isdigit() and int(token) < len(current):
            current = current[int(token)]
        else:
            return False, None
    return True, current


def _assert_value(exists: bool, actual: Any, assertion: Mapping[str, Any]) -> bool:
    op = assertion["op"]
    expected = assertion.get("value")
    if op == "exists":
        return exists
    if op == "absent":
        return not exists
    if not exists:
        return False
    if op == "eq":
        return actual == expected
    if op == "ne":
        return actual != expected
    if op == "in":
        return isinstance(expected, list) and actual in expected
    if op == "not_in":
        return isinstance(expected, list) and actual not in expected
    if op == "truthy":
        return bool(actual)
    if op == "falsy":
        return not bool(actual)
    if op == "length_eq":
        return hasattr(actual, "__len__") and not isinstance(expected, bool) and len(actual) == expected
    try:
        if op == "gt":
            return actual > expected
        if op == "ge":
            return actual >= expected
        if op == "lt":
            return actual < expected
        if op == "le":
            return actual <= expected
    except (TypeError, ValueError):
        return False
    return False


def _evaluate_assertions(
    assertions: Sequence[Mapping[str, Any]],
    roots: Mapping[str, Path],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    public: list[dict[str, Any]] = []
    private: list[dict[str, Any]] = []
    cache: dict[tuple[str, str], tuple[bool, Any, str | None]] = {}
    for assertion in assertions:
        key = (str(assertion["source"]), str(assertion["path"]))
        if key not in cache:
            try:
                path, _ = resolve_under(roots[key[0]], key[1], "assertion evidence")
                document = read_json(path, "assertion evidence")
                cache[key] = (True, document, None)
            except GateError as exc:
                cache[key] = (False, None, str(exc))
        loaded, document, load_error = cache[key]
        exists, actual = _json_pointer(document, str(assertion["pointer"])) if loaded else (False, None)
        passed = loaded and _assert_value(exists, actual, assertion)
        public.append({"id": assertion["id"], "passed": bool(passed)})
        private.append(
            {
                **dict(assertion),
                "passed": bool(passed),
                "pointer_exists": exists,
                "actual": actual,
                "load_error": load_error,
            }
        )
    return public, private


def _terminate_process_tree(process: subprocess.Popen[bytes], force_stop_seconds: float) -> int:
    if process.poll() is not None:
        return 0
    deadline = time.monotonic() + max(0.1, force_stop_seconds)
    attempts = 0
    while process.poll() is None and time.monotonic() < deadline:
        attempts += 1
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
        remaining = max(0.05, deadline - time.monotonic())
        try:
            process.wait(timeout=min(2.0, remaining))
        except subprocess.TimeoutExpired:
            continue
    if process.poll() is None:
        attempts += 1
        process.kill()
        process.wait(timeout=max(1.0, force_stop_seconds))
    if process.poll() is None:
        raise GateError(f"timed-out process tree is still alive for exact PID {process.pid}")
    return attempts


def _not_run_command(reason: str) -> tuple[dict[str, Any], dict[str, Any]]:
    public = {"status": NOT_RUN, "reason_code": reason}
    return public, dict(public)


def _execute_command(command: Mapping[str, Any], label: str) -> tuple[dict[str, Any], dict[str, Any]]:
    executable = Path(command["executable"])
    argv = [str(executable), *list(command["argv"])[1:]]
    started_at = utc_now()
    started = time.perf_counter()
    creation_flags = subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0
    timed_out = False
    start_error: str | None = None
    stdout = b""
    stderr = b""
    exit_code: int | None = None
    termination_attempts = 0
    try:
        process = subprocess.Popen(
            argv,
            cwd=command["cwd"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            shell=False,
            creationflags=creation_flags,
            start_new_session=os.name != "nt",
        )
        try:
            stdout, stderr = process.communicate(timeout=command["timeout_seconds"])
        except subprocess.TimeoutExpired:
            timed_out = True
            termination_attempts = _terminate_process_tree(
                process, command["force_stop_seconds"]
            )
            stdout, stderr = process.communicate()
        exit_code = process.returncode
    except (OSError, subprocess.SubprocessError) as exc:
        start_error = f"{type(exc).__name__}: {exc}"
    elapsed = round(time.perf_counter() - started, 3)
    passed = (
        start_error is None
        and not timed_out
        and exit_code in command["expected_exit_codes"]
    )
    executable_bytes, executable_hash = hash_file(executable)
    common = {
        "label": label,
        "status": PASS if passed else FAIL,
        "started_at": started_at,
        "ended_at": utc_now(),
        "elapsed_seconds": elapsed,
        "exit_code": exit_code,
        "expected_exit_codes": list(command["expected_exit_codes"]),
        "timed_out": timed_out,
        "termination_attempts": termination_attempts,
        "start_error": start_error is not None,
        "executable": executable.name,
        "executable_bytes": executable_bytes,
        "executable_sha256": executable_hash,
        "argv_sha256": sha256_bytes(canonical_json_bytes(argv)),
        "cwd_sha256": sha256_bytes(str(command["cwd"]).encode("utf-8")),
        "stdout_bytes": len(stdout),
        "stdout_sha256": sha256_bytes(stdout),
        "stderr_bytes": len(stderr),
        "stderr_sha256": sha256_bytes(stderr),
    }
    private = {
        **common,
        "argv": argv,
        "cwd": str(command["cwd"]),
        "stdout_base64": base64.b64encode(stdout).decode("ascii"),
        "stderr_base64": base64.b64encode(stderr).decode("ascii"),
        "start_error_text": start_error,
    }
    return common, private


def _safe_execute_command(
    command: Mapping[str, Any], label: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Turn a late command/executable failure into evidence instead of skipping cleanup."""

    try:
        return _execute_command(command, label)
    except Exception as exc:  # The private report keeps the exact late failure.
        common = {
            "label": label,
            "status": FAIL,
            "started_at": utc_now(),
            "ended_at": utc_now(),
            "elapsed_seconds": 0.0,
            "exit_code": None,
            "expected_exit_codes": list(command.get("expected_exit_codes", [])),
            "timed_out": False,
            "termination_attempts": 0,
            "start_error": True,
            "error_code": "COMMAND_EXECUTION_EXCEPTION",
        }
        return common, {**common, "error_text": f"{type(exc).__name__}: {exc}"}


def _evidence_fingerprint(path: Path) -> tuple[int, int, int] | None:
    if not path.exists():
        return None
    _reject_link_chain(path)
    if not path.is_file():
        raise GateError("declared evidence path exists but is not a file")
    stat = path.stat()
    return stat.st_size, stat.st_mtime_ns, getattr(stat, "st_ino", 0)


def _capture_evidence(
    paths: Sequence[tuple[Path, str]],
    before: Mapping[str, tuple[int, int, int] | None],
) -> tuple[list[dict[str, Any]], list[str], list[str]]:
    records: list[dict[str, Any]] = []
    missing: list[str] = []
    stale: list[str] = []
    for path, relative in paths:
        if not path.is_file():
            missing.append(relative)
            continue
        record = file_record(path, relative_path=relative)
        current = _evidence_fingerprint(path)
        fresh = before.get(str(path)) != current
        record["fresh"] = fresh
        records.append(record)
        if not fresh:
            stale.append(relative)
    return records, missing, stale


def _split_group_evidence(
    paths: Sequence[tuple[Path, str]],
) -> tuple[list[tuple[Path, str]], list[tuple[Path, str]]]:
    business: list[tuple[Path, str]] = []
    cleanup: list[tuple[Path, str]] = []
    for item in paths:
        (cleanup if item[1].startswith("cleanup/") else business).append(item)
    return business, cleanup


def _changed_after_cleanup(
    paths: Sequence[tuple[Path, str]],
    frozen_records: Sequence[Mapping[str, Any]],
) -> list[str]:
    expected = {str(item.get("path")): item for item in frozen_records}
    changed: list[str] = []
    for path, relative in paths:
        frozen = expected.get(relative)
        if frozen is None:
            continue
        if not path.is_file():
            changed.append(relative)
            continue
        _reject_link_chain(path)
        current = file_record(path, relative_path=relative)
        if current["bytes"] != frozen.get("bytes") or current["sha256"] != frozen.get("sha256"):
            changed.append(relative)
    return changed


def _output_path(root: Path, requested: Path, label: str) -> Path:
    requested = Path(os.path.abspath(requested))
    if not _is_under(requested, root) or requested == root:
        raise GateError(f"{label} must be a file below its declared evidence root")
    relative = requested.resolve(strict=False).relative_to(root.resolve()).as_posix()
    resolved, _ = resolve_under(root, relative, label)
    if resolved.exists():
        raise GateError(f"{label} already exists; use a new run output")
    resolved.parent.mkdir(parents=True, exist_ok=True)
    _reject_link_chain(resolved.parent)
    return resolved


def _statistics(results: Sequence[Mapping[str, Any]], run_key_count: int) -> dict[str, int]:
    counts = {PASS: 0, FAIL: 0, NOT_RUN: 0}
    for item in results:
        counts[str(item["status"])] = counts.get(str(item["status"]), 0) + 1
    return {
        "total": len(results),
        "pass": counts[PASS],
        "fail": counts[FAIL],
        "not_run": counts[NOT_RUN],
        "run_once_total": run_key_count,
    }


def run_acceptance(
    config_path: Path,
    repo: Path,
    public_output: Path,
    private_output: Path,
    *,
    required_ids: Sequence[str] = FORMAL_CASE_IDS,
    required_names: Mapping[str, str] | None = None,
    expected_run_keys: Mapping[str, str] | None = None,
    require_external_config: bool = True,
) -> tuple[int, dict[str, Any], dict[str, Any]]:
    context = validate_config(
        config_path,
        repo,
        required_ids=required_ids,
        required_names=required_names,
        expected_run_keys=expected_run_keys,
        require_external_config=require_external_config,
    )
    public_output = _output_path(context["public_root"], public_output, "public report")
    private_output = _output_path(context["private_root"], private_output, "private report")
    if public_output == private_output or public_output == context["path"] or private_output == context["path"]:
        raise GateError("acceptance outputs must not overwrite each other or the config")

    bound_paths = {
        "config": context["path"],
        "candidate": context["candidate"]["path"],
        "build_manifest": context["build_manifest"]["path"],
    }
    for scenario in context["scenarios"]:
        for index, item in enumerate(scenario["inputs"]):
            bound_paths.setdefault(f"input:{scenario['id']}:{index}", item["path"])
    before_bindings = {name: _binding_snapshot(path) for name, path in bound_paths.items()}
    evidence_before: dict[str, tuple[int, int, int] | None] = {}
    for scenario in context["scenarios"]:
        for source in ("public", "private"):
            for path, _ in scenario["evidence"][source]:
                key = str(path)
                if key in evidence_before:
                    continue
                if path.exists() or path.is_symlink():
                    raise GateError(
                        "declared formal evidence already exists; use a new run directory"
                    )
                evidence_before[key] = None

    roots = {"public": context["public_root"], "private": context["private_root"]}
    public_results: list[dict[str, Any]] = []
    private_results: list[dict[str, Any]] = []
    status_by_id: dict[str, str] = {}
    producer_runs: dict[str, int] = {}
    private_group_runs: list[dict[str, Any]] = []
    for run_key in context["run_keys"]:
        members = [item for item in context["scenarios"] if item["run_once_key"] == run_key]
        representative = members[0]
        dependencies = representative["dependencies"]
        dependencies_pass = all(status_by_id.get(item) == PASS for item in dependencies)
        if dependencies_pass:
            producer_public, producer_private = _safe_execute_command(
                representative["command"], f"producer:{run_key}"
            )
            producer_runs[run_key] = 1
        else:
            producer_public, producer_private = _not_run_command("DEPENDENCY_NOT_PASS")
            producer_runs[run_key] = 0

        # Business evidence and business assertions are frozen before cleanup.
        # Cleanup is not allowed to create, delete, or rewrite the facts it is
        # supposed to clean up after.
        frozen_by_case: dict[str, dict[str, Any]] = {}
        for scenario in members:
            business_public_paths, cleanup_public_paths = _split_group_evidence(
                scenario["evidence"]["public"]
            )
            business_private_paths, cleanup_private_paths = _split_group_evidence(
                scenario["evidence"]["private"]
            )
            business_public, missing_business_public, stale_business_public = _capture_evidence(
                business_public_paths, evidence_before
            )
            business_private, missing_business_private, stale_business_private = _capture_evidence(
                business_private_paths, evidence_before
            )
            pass_public, pass_private = _evaluate_assertions(
                scenario["pass_assertions"], roots
            )
            fail_public, fail_private = _evaluate_assertions(
                scenario["fail_assertions"], roots
            )
            frozen_by_case[scenario["id"]] = {
                "business_public_paths": business_public_paths,
                "cleanup_public_paths": cleanup_public_paths,
                "business_private_paths": business_private_paths,
                "cleanup_private_paths": cleanup_private_paths,
                "business_public": business_public,
                "business_private": business_private,
                "missing_business_public": missing_business_public,
                "missing_business_private": missing_business_private,
                "stale_business_public": stale_business_public,
                "stale_business_private": stale_business_private,
                "pass_public": pass_public,
                "pass_private": pass_private,
                "fail_public": fail_public,
                "fail_private": fail_private,
            }

        # Cleanup deliberately runs even when dependencies blocked or producer start failed.
        cleanup_public, cleanup_private = _safe_execute_command(
            representative["cleanup"], f"cleanup:{run_key}"
        )
        private_group_runs.append(
            {
                "run_once_key": run_key,
                "machine_run_id": context["machine_run_id"],
                "producer_nonce": context["producer_nonces"].get(run_key),
                "rerun_scope_on_failure": GROUP_RERUN_SCOPE,
                "producer": producer_private,
                "cleanup": cleanup_private,
            }
        )
        for scenario in members:
            frozen = frozen_by_case[scenario["id"]]
            cleanup_public_evidence, missing_cleanup_public, stale_cleanup_public = _capture_evidence(
                frozen["cleanup_public_paths"], evidence_before
            )
            cleanup_private_evidence, missing_cleanup_private, stale_cleanup_private = _capture_evidence(
                frozen["cleanup_private_paths"], evidence_before
            )
            changed_public = _changed_after_cleanup(
                frozen["business_public_paths"], frozen["business_public"]
            )
            changed_private = _changed_after_cleanup(
                frozen["business_private_paths"], frozen["business_private"]
            )
            public_evidence = [*frozen["business_public"], *cleanup_public_evidence]
            private_evidence = [*frozen["business_private"], *cleanup_private_evidence]
            missing_public = [*frozen["missing_business_public"], *missing_cleanup_public]
            missing_private = [*frozen["missing_business_private"], *missing_cleanup_private]
            stale_public = [*frozen["stale_business_public"], *stale_cleanup_public]
            stale_private = [*frozen["stale_business_private"], *stale_cleanup_private]
            pass_public = frozen["pass_public"]
            pass_private = frozen["pass_private"]
            fail_public = frozen["fail_public"]
            fail_private = frozen["fail_private"]
            cleanup_checks_public, cleanup_checks_private = _evaluate_assertions(
                scenario["cleanup"]["checks"], roots
            )
            cleanup_ok = cleanup_public["status"] == PASS and all(
                item["passed"] for item in cleanup_checks_public
            )
            if not dependencies_pass:
                scenario_status = NOT_RUN if cleanup_ok else FAIL
            else:
                scenario_status = (
                    PASS
                    if producer_public["status"] == PASS
                    and cleanup_ok
                    and public_evidence
                    and private_evidence
                    and not missing_public
                    and not missing_private
                    and not stale_public
                    and not stale_private
                    and not changed_public
                    and not changed_private
                    and all(item["passed"] for item in pass_public)
                    and not any(item["passed"] for item in fail_public)
                    else FAIL
                )
            public_item = {
                "id": scenario["id"],
                "name": scenario["name"],
                "run_once_key": run_key,
                "machine_run_id": context["machine_run_id"],
                "producer_nonce": context["producer_nonces"].get(run_key),
                "rerun_scope_on_failure": GROUP_RERUN_SCOPE,
                "dependencies": list(scenario["dependencies"]),
                "status": scenario_status,
                "producer": producer_public,
                "cleanup": cleanup_public,
                "pass_assertions": pass_public,
                "fail_assertions": fail_public,
                "cleanup_checks": cleanup_checks_public,
                "public_evidence": public_evidence,
                "missing_public_evidence": missing_public,
                "stale_public_evidence": stale_public,
                "changed_public_evidence_after_cleanup": changed_public,
                "private_evidence_count": len(private_evidence),
                "missing_private_evidence_count": len(missing_private),
                "stale_private_evidence_count": len(stale_private),
                "changed_private_evidence_after_cleanup_count": len(changed_private),
            }
            private_item = {
                **public_item,
                "producer": producer_private,
                "cleanup": cleanup_private,
                "pass_assertions": pass_private,
                "fail_assertions": fail_private,
                "cleanup_checks": cleanup_checks_private,
                "public_evidence": public_evidence,
                "private_evidence": private_evidence,
                "missing_private_evidence": missing_private,
                "stale_private_evidence": stale_private,
                "changed_private_evidence_after_cleanup": changed_private,
                "declared_public_evidence": [relative for _, relative in scenario["evidence"]["public"]],
                "declared_private_evidence": [relative for _, relative in scenario["evidence"]["private"]],
            }
            public_results.append(public_item)
            private_results.append(private_item)
            status_by_id[scenario["id"]] = scenario_status

    after_bindings: dict[str, dict[str, Any] | None] = {}
    for name, path in bound_paths.items():
        try:
            after_bindings[name] = _binding_snapshot(path)
        except (GateError, OSError):
            after_bindings[name] = None
    if context.get("repository_binding") is not None:
        repository_after = _assert_repository_clean(
            context["repo"], "formal acceptance run after producers"
        )
        if repository_after["head"] != context["repository_binding"]["head"]:
            raise GateError("formal acceptance producers changed repository HEAD")
    declared_input_bindings = {
        f"input:{scenario['id']}:{index}": {
            "bytes": item["bytes"],
            "sha256": item["sha256"],
        }
        for scenario in context["scenarios"]
        for index, item in enumerate(scenario["inputs"])
    }
    integrity_checks = {
        "config_unchanged": (
            before_bindings["config"]["bytes"] == context["bytes"]
            and before_bindings["config"]["sha256"] == context["sha256"]
            and before_bindings["config"] == after_bindings["config"]
        ),
        "candidate_unchanged": (
            before_bindings["candidate"]["bytes"] == context["candidate"]["bytes"]
            and before_bindings["candidate"]["sha256"] == context["candidate"]["sha256"]
            and before_bindings["candidate"] == after_bindings["candidate"]
        ),
        "build_manifest_unchanged": (
            before_bindings["build_manifest"]["bytes"] == context["build_manifest"]["bytes"]
            and before_bindings["build_manifest"]["sha256"]
            == context["build_manifest"]["sha256"]
            and before_bindings["build_manifest"] == after_bindings["build_manifest"]
        ),
        "inputs_unchanged": all(
            before_bindings[name]["bytes"] == declared["bytes"]
            and before_bindings[name]["sha256"] == declared["sha256"]
            and before_bindings[name] == after_bindings[name]
            for name, declared in declared_input_bindings.items()
        ),
        "source_commit_unchanged": _git_head(context["repo"]) == context["source_commit"],
    }
    if not all(integrity_checks.values()):
        for public_item, private_item in zip(public_results, private_results, strict=True):
            if public_item["status"] == PASS:
                public_item["status"] = FAIL
                private_item["status"] = FAIL
                public_item["integrity_blocker"] = True
                private_item["integrity_blocker"] = True

    public_results.sort(key=lambda item: item["id"])
    private_results.sort(key=lambda item: item["id"])
    stats = _statistics(public_results, len(context["run_keys"]))
    overall_status = PASS if stats["pass"] == len(required_ids) else FAIL
    input_public_records = sorted(
        {
            (snapshot["bytes"], snapshot["sha256"])
            for name, snapshot in before_bindings.items()
            if name.startswith("input:")
        }
    )
    private_report = {
        "schema_version": SCHEMA_VERSION,
        "stage": PRIVATE_STAGE,
        "generated_at": utc_now(),
        "run_id": context["run_id"],
        "machine_run_id": context["machine_run_id"],
        "execution_contract": context["execution_contract"],
        "run_registration_sha256": (
            context["run_registration"]["sha256"]
            if context["run_registration"] is not None
            else None
        ),
        "producer_nonces": context["producer_nonces"],
        "source_commit": context["source_commit"],
        "status": overall_status,
        "config_path": str(context["path"]),
        "config_bytes": context["bytes"],
        "config_sha256": context["sha256"],
        "candidate": {
            "path": str(context["candidate"]["path"]),
            "bytes": context["candidate"]["bytes"],
            "sha256": context["candidate"]["sha256"],
        },
        "build_manifest": {
            "path": str(context["build_manifest"]["path"]),
            "bytes": context["build_manifest"]["bytes"],
            "sha256": context["build_manifest"]["sha256"],
        },
        "evidence_roots": {
            "public": str(context["public_root"]),
            "private": str(context["private_root"]),
        },
        "bound_file_snapshots_before": before_bindings,
        "bound_file_snapshots_after": after_bindings,
        "integrity_checks": integrity_checks,
        "statistics": stats,
        "producer_run_counts": producer_runs,
        "group_runs": private_group_runs,
        "scenarios": private_results,
    }
    atomic_write_json(private_output, private_report)
    private_record = file_record(private_output, relative_path=private_output.name)
    public_report = {
        "schema_version": SCHEMA_VERSION,
        "stage": PUBLIC_STAGE,
        "generated_at": utc_now(),
        "run_id": context["run_id"],
        "machine_run_id": context["machine_run_id"],
        "execution_contract": context["execution_contract"],
        "run_registration_sha256": (
            context["run_registration"]["sha256"]
            if context["run_registration"] is not None
            else None
        ),
        "producer_nonces": context["producer_nonces"],
        "source_commit": context["source_commit"],
        "status": overall_status,
        "config_bytes": context["bytes"],
        "config_sha256": context["sha256"],
        "candidate_bytes": context["candidate"]["bytes"],
        "candidate_sha256": context["candidate"]["sha256"],
        "build_manifest_bytes": context["build_manifest"]["bytes"],
        "build_manifest_sha256": context["build_manifest"]["sha256"],
        "input_files": [
            {"bytes": byte_count, "sha256": digest}
            for byte_count, digest in input_public_records
        ],
        "private_report_bytes": private_record["bytes"],
        "private_report_sha256": private_record["sha256"],
        "integrity_checks": integrity_checks,
        "statistics": stats,
        "producer_run_counts": producer_runs,
        "required_cases": list(required_ids),
        "scenarios": public_results,
        "privacy_rule": (
            "Exact argv, cwd, private paths, and command output are stored only in the private report."
        ),
    }
    atomic_write_json(public_output, public_report)
    return status_exit_code(overall_status), public_report, private_report


def _privacy_audit(public_report: Mapping[str, Any], context: Mapping[str, Any]) -> None:
    forbidden_keys = {
        "argv",
        "cwd",
        "stdout_base64",
        "stderr_base64",
        "start_error_text",
        "private_evidence",
        "missing_private_evidence",
        "stale_private_evidence",
        "config_path",
        "evidence_roots",
    }

    def walk(value: Any) -> None:
        if isinstance(value, dict):
            overlap = forbidden_keys.intersection(value)
            if overlap:
                raise GateError(f"public report leaks forbidden fields: {sorted(overlap)}")
            for item in value.values():
                walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)
        elif isinstance(value, str):
            private_root = str(context["private_root"])
            if private_root and private_root.casefold() in value.casefold():
                raise GateError("public report leaks the private evidence root")

    walk(public_report)


def _verify_evidence_records(
    records: Sequence[Mapping[str, Any]],
    declared: Sequence[tuple[Path, str]],
    label: str,
) -> None:
    by_path = {str(item.get("path")): item for item in records}
    if set(by_path) != {relative for _, relative in declared}:
        raise GateError(f"{label} evidence record set does not match the config")
    for path, relative in declared:
        record = require_mapping(by_path[relative], f"{label} evidence record")
        actual = file_record(path, relative_path=relative)
        if (
            record.get("bytes") != actual["bytes"]
            or require_sha256(record.get("sha256"), f"{label} sha256") != actual["sha256"]
            or record.get("fresh") is not True
        ):
            raise GateError(f"{label} evidence record does not match the real file")


def _public_command_view(private_command: Mapping[str, Any]) -> dict[str, Any]:
    private_only = {
        "argv",
        "cwd",
        "stdout_base64",
        "stderr_base64",
        "start_error_text",
        "error_text",
    }
    return {key: value for key, value in private_command.items() if key not in private_only}


def _verify_private_command(
    record: Mapping[str, Any], expected: Mapping[str, Any], label: str
) -> None:
    expected_argv = [str(expected["executable"]), *list(expected["argv"])[1:]]
    if record.get("status") != PASS or record.get("argv") != expected_argv:
        raise GateError(f"{label} command or status does not match the config")
    if record.get("cwd") != str(expected["cwd"]):
        raise GateError(f"{label} cwd does not match the config")
    if (
        record.get("timed_out") is not False
        or record.get("start_error") is not False
        or record.get("exit_code") not in expected["expected_exit_codes"]
        or record.get("expected_exit_codes") != list(expected["expected_exit_codes"])
    ):
        raise GateError(f"{label} process outcome is not a complete PASS")
    try:
        stdout_encoded = record.get("stdout_base64")
        stderr_encoded = record.get("stderr_base64")
        if not isinstance(stdout_encoded, str) or not isinstance(stderr_encoded, str):
            raise ValueError("command output is not a base64 string")
        stdout = base64.b64decode(stdout_encoded, validate=True)
        stderr = base64.b64decode(stderr_encoded, validate=True)
    except (ValueError, TypeError) as exc:
        raise GateError(f"{label} contains invalid private command output") from exc
    if (
        record.get("stdout_bytes") != len(stdout)
        or record.get("stdout_sha256") != sha256_bytes(stdout)
        or record.get("stderr_bytes") != len(stderr)
        or record.get("stderr_sha256") != sha256_bytes(stderr)
        or record.get("argv_sha256") != sha256_bytes(canonical_json_bytes(expected_argv))
        or record.get("cwd_sha256") != sha256_bytes(str(expected["cwd"]).encode("utf-8"))
    ):
        raise GateError(f"{label} private command hashes are invalid")
    executable_bytes, executable_sha = hash_file(Path(expected["executable"]))
    if (
        record.get("executable") != Path(expected["executable"]).name
        or record.get("executable_bytes") != executable_bytes
        or record.get("executable_sha256") != executable_sha
    ):
        raise GateError(f"{label} executable binding is invalid")


def _validate_window_identity(value: Any, label: str) -> dict[str, Any]:
    raw = require_mapping(value, label)
    _require_exact_keys(
        raw,
        {
            "application",
            "identity_role",
            "title",
            "window_id",
            "pid",
            "process_started_at",
            "process_instance_id",
            "exe_path",
            "exe_path_sha256",
        },
        label,
    )
    role = require_string(raw.get("identity_role"), f"{label} identity_role")
    if role not in {
        "candidate_main",
        "candidate_installer",
        "candidate_uninstaller",
        "file_manager",
        "report_viewer",
        "windows_sandbox",
    }:
        raise GateError(f"{label} identity_role is not approved")
    pid = raw.get("pid")
    if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
        raise GateError(f"{label} pid must be a positive integer")
    process_started_at = require_string(
        raw.get("process_started_at"), f"{label} process_started_at"
    )
    _parse_timestamp(process_started_at, f"{label} process_started_at")
    executable_path = _absolute_path(raw.get("exe_path"), f"{label} exe_path")
    _reject_link_chain(executable_path)
    if not executable_path.is_file():
        raise GateError(f"{label} executable is missing: {executable_path}")
    _, executable_sha256 = hash_file(executable_path)
    if require_sha256(
        raw.get("exe_path_sha256"), f"{label} exe_path_sha256"
    ) != executable_sha256:
        raise GateError(f"{label} executable SHA-256 does not match the file at exe_path")
    expected_process_instance_id = sha256_bytes(
        canonical_json_bytes(
            {
                "exe_path": str(executable_path),
                "exe_path_sha256": executable_sha256,
                "pid": pid,
                "process_started_at": process_started_at,
            }
        )
    )
    if require_sha256(
        raw.get("process_instance_id"), f"{label} process_instance_id"
    ) != expected_process_instance_id:
        raise GateError(
            f"{label} process_instance_id is not derived from PID, start time, and executable"
        )
    return {
        "application": require_string(raw.get("application"), f"{label} application"),
        "identity_role": role,
        "title": require_string(raw.get("title"), f"{label} title"),
        "window_id": _safe_id(raw.get("window_id"), f"{label} window_id"),
        "pid": pid,
        "process_started_at": process_started_at,
        "process_instance_id": expected_process_instance_id,
        "exe_path": str(executable_path),
        "exe_path_sha256": executable_sha256,
    }


def _candidate_identity_from_context(context: Mapping[str, Any]) -> dict[str, str]:
    details = require_mapping(
        context.get("build_manifest_details"), "candidate build-manifest details"
    )
    document = require_mapping(details.get("document"), "candidate build-manifest document")
    installed = require_mapping(
        details.get("installed_files"), "candidate installed-file records"
    )
    main = require_mapping(installed.get("main_executable"), "candidate main executable")
    return {
        "candidate_sha256": require_sha256(
            require_mapping(context.get("candidate"), "acceptance candidate").get("sha256"),
            "acceptance candidate sha256",
        ),
        "main_exe_sha256": require_sha256(
            main.get("sha256"), "candidate main executable sha256"
        ),
        "version": require_string(document.get("version"), "candidate version"),
    }


def _normalize_machine_event(
    value: Any,
    *,
    label: str,
    evidence_path: str,
    context: Mapping[str, Any],
    registered_at: datetime,
) -> dict[str, Any]:
    raw = require_mapping(value, label)
    _require_exact_keys(
        raw,
        {
            "event_id",
            "machine_run_id",
            "source_commit",
            "candidate_sha256",
            "run_registration_sha256",
            "producer_nonce",
            "started_at",
            "ended_at",
            "target_window",
            "observed_version",
        },
        label,
    )
    if raw.get("machine_run_id") != context["machine_run_id"]:
        raise GateError(f"{label} machine_run_id does not match acceptance")
    if require_git_commit(raw.get("source_commit"), f"{label} source_commit") != context["source_commit"]:
        raise GateError(f"{label} source_commit does not match acceptance")
    candidate = _candidate_identity_from_context(context)
    if require_sha256(raw.get("candidate_sha256"), f"{label} candidate_sha256") != candidate[
        "candidate_sha256"
    ]:
        raise GateError(f"{label} candidate_sha256 does not match acceptance")
    registration = require_mapping(context.get("run_registration"), "run registration")
    registration_sha = require_sha256(
        registration.get("sha256"), "run registration sha256"
    )
    if require_sha256(
        raw.get("run_registration_sha256"), f"{label} run_registration_sha256"
    ) != registration_sha:
        raise GateError(f"{label} run registration binding is invalid")
    producer_key = CUA_PRODUCER_KEY_BY_MACHINE_PATH.get(evidence_path)
    raw_nonce = raw.get("producer_nonce")
    if producer_key is None:
        if raw_nonce is not None:
            raise GateError(f"{label} must not invent a producer nonce")
        producer_nonce = None
    else:
        producer_nonce = require_sha256(raw_nonce, f"{label} producer_nonce")
        expected_nonces = _validate_producer_nonces(context.get("producer_nonces"))
        if producer_nonce != expected_nonces[producer_key]:
            raise GateError(f"{label} producer_nonce does not match its formal group")
    started = _parse_timestamp(raw.get("started_at"), f"{label} started_at")
    ended = _parse_timestamp(raw.get("ended_at"), f"{label} ended_at")
    if started < registered_at or ended < started:
        raise GateError(f"{label} happened before registration or ended before it started")
    target_window = _validate_window_identity(
        raw.get("target_window"), f"{label} target_window"
    )
    process_started = _parse_timestamp(
        target_window["process_started_at"], f"{label} target process_started_at"
    )
    if process_started > started:
        raise GateError(f"{label} target process started after the machine event")
    return {
        "event_id": _safe_id(raw.get("event_id"), f"{label} event_id"),
        "machine_run_id": context["machine_run_id"],
        "source_commit": context["source_commit"],
        "candidate_sha256": candidate["candidate_sha256"],
        "run_registration_sha256": registration_sha,
        "producer_nonce": producer_nonce,
        "started_at": started,
        "ended_at": ended,
        "target_window": target_window,
        "observed_version": require_string(
            raw.get("observed_version"), f"{label} observed_version"
        ),
    }


def _verify_cua_records(context: Mapping[str, Any]) -> dict[str, Any]:
    private_root = Path(context["private_root"]).resolve()
    candidate_identity = _candidate_identity_from_context(context)
    registration = require_mapping(context.get("run_registration"), "run registration")
    registration_document = require_mapping(
        registration.get("document"), "run registration document"
    )
    registration_sha = require_sha256(
        registration.get("sha256"), "run registration sha256"
    )
    registered_at = _parse_timestamp(
        registration_document.get("registered_at"), "run registration registered_at"
    )
    expected_record_paths = {
        (private_root / relative).resolve() for relative in CUA_RECORD_PATH_BY_TASK.values()
    }
    actual_record_paths = {
        path.resolve() for path in (private_root / "CUA").rglob("cua.private.json")
    } if (private_root / "CUA").is_dir() else set()
    if actual_record_paths != expected_record_paths:
        raise GateError("private evidence must contain exactly one cua.private.json for each AT-00 through AT-21")

    observed_ui_run_ids: list[str] = []
    verified_records: list[dict[str, Any]] = []
    machine_documents: dict[str, tuple[dict[str, Any], int, str]] = {}
    shared_event_usage: dict[tuple[str, str], set[str]] = {}
    machine_event_binding_count = 0
    for task_id in CUA_TASK_IDS:
        record_relative = CUA_RECORD_PATH_BY_TASK[task_id]
        manifest_relative = CUA_HASH_MANIFEST_PATH_BY_TASK[task_id]
        record_path, _ = resolve_under(private_root, record_relative, f"{task_id} CUA record")
        manifest_path, _ = resolve_under(
            private_root, manifest_relative, f"{task_id} CUA hash manifest"
        )
        record, record_sha, record_bytes = _stable_json(record_path, f"{task_id} CUA record")
        manifest, manifest_sha, manifest_bytes = _stable_json(
            manifest_path, f"{task_id} CUA hash manifest"
        )
        _require_exact_keys(
            manifest,
            {"schema_version", "stage", "task_id", "machine_run_id", "files"},
            f"{task_id} CUA hash manifest",
        )
        if (
            manifest.get("schema_version") != SCHEMA_VERSION
            or manifest.get("stage") != CUA_HASH_MANIFEST_STAGE
            or manifest.get("task_id") != task_id
            or manifest.get("machine_run_id") != context["machine_run_id"]
        ):
            raise GateError(f"{task_id} CUA hash manifest identity is invalid")
        files = require_list(manifest.get("files"), f"{task_id} CUA hash manifest files")
        if len(files) != 1:
            raise GateError(f"{task_id} CUA hash manifest must contain exactly the CUA record")
        manifest_record = require_mapping(files[0], f"{task_id} CUA hash record")
        _require_exact_keys(
            manifest_record, {"path", "bytes", "sha256"}, f"{task_id} CUA hash record"
        )
        if (
            manifest_record.get("path") != record_relative
            or manifest_record.get("bytes") != record_bytes
            or require_sha256(
                manifest_record.get("sha256"), f"{task_id} CUA manifest sha256"
            )
            != record_sha
        ):
            raise GateError(f"{task_id} CUA record does not match its evidence-sha256 manifest")

        _require_exact_keys(
            record,
            {
                "schema_version",
                "stage",
                "task_id",
                "machine_run_id",
                "ui_run_id",
                "source_commit",
                "candidate_sha256",
                "run_registration_sha256",
                "candidate_identity",
                "observed_version",
                "target_window",
                "window_selection",
                "session_started_at",
                "session_ended_at",
                "producer_session_active",
                "actions",
            },
            f"{task_id} CUA record",
        )
        if (
            record.get("schema_version") != SCHEMA_VERSION
            or record.get("stage") != CUA_RECORD_STAGE
            or record.get("task_id") != task_id
            or record.get("machine_run_id") != context["machine_run_id"]
            or record.get("producer_session_active") is not False
        ):
            raise GateError(f"{task_id} CUA record identity or session state is invalid")
        ui_run_id = _safe_id(record.get("ui_run_id"), f"{task_id} ui_run_id")
        if ui_run_id == context["machine_run_id"]:
            raise GateError(f"{task_id} ui_run_id must differ from machine_run_id")
        observed_ui_run_ids.append(ui_run_id)
        if require_git_commit(record.get("source_commit"), f"{task_id} source_commit") != context[
            "source_commit"
        ]:
            raise GateError(f"{task_id} source_commit does not match acceptance")
        if require_sha256(record.get("candidate_sha256"), f"{task_id} candidate_sha256") != candidate_identity[
            "candidate_sha256"
        ]:
            raise GateError(f"{task_id} candidate_sha256 does not match acceptance")
        if require_sha256(
            record.get("run_registration_sha256"), f"{task_id} run_registration_sha256"
        ) != registration_sha:
            raise GateError(f"{task_id} run registration binding is invalid")
        raw_candidate_identity = require_mapping(
            record.get("candidate_identity"), f"{task_id} candidate_identity"
        )
        _require_exact_keys(
            raw_candidate_identity,
            {"candidate_sha256", "main_exe_sha256", "version"},
            f"{task_id} candidate_identity",
        )
        normalized_candidate_identity = {
            "candidate_sha256": require_sha256(
                raw_candidate_identity.get("candidate_sha256"),
                f"{task_id} candidate identity installer sha256",
            ),
            "main_exe_sha256": require_sha256(
                raw_candidate_identity.get("main_exe_sha256"),
                f"{task_id} candidate identity main exe sha256",
            ),
            "version": require_string(
                raw_candidate_identity.get("version"), f"{task_id} candidate identity version"
            ),
        }
        if normalized_candidate_identity != candidate_identity:
            raise GateError(f"{task_id} candidate identity does not match the build manifest")
        observed_version = require_string(
            record.get("observed_version"), f"{task_id} observed_version"
        )
        if observed_version != candidate_identity["version"]:
            raise GateError(f"{task_id} observed_version does not match the candidate version")
        target_window = _validate_window_identity(
            record.get("target_window"), f"{task_id} target_window"
        )
        if target_window["identity_role"] == "candidate_main" and target_window[
            "exe_path_sha256"
        ] != candidate_identity["main_exe_sha256"]:
            raise GateError(f"{task_id} selected executable is not the candidate main executable")
        if target_window["identity_role"] == "candidate_installer" and target_window[
            "exe_path_sha256"
        ] != candidate_identity["candidate_sha256"]:
            raise GateError(f"{task_id} selected installer is not the acceptance candidate")
        if target_window["identity_role"] == "candidate_uninstaller":
            installed = require_mapping(
                require_mapping(context["build_manifest_details"], "build details").get(
                    "installed_files"
                ),
                "installed files",
            )
            uninstaller = require_mapping(installed.get("uninstaller"), "candidate uninstaller")
            if target_window["exe_path_sha256"] != require_sha256(
                uninstaller.get("sha256"), "candidate uninstaller sha256"
            ):
                raise GateError(f"{task_id} selected uninstaller is not the candidate uninstaller")

        selection = require_mapping(record.get("window_selection"), f"{task_id} window_selection")
        _require_exact_keys(
            selection,
            {
                "enumerated_at",
                "matching_window_count",
                "selected_window_id",
                "selection_query",
                "window_list_sha256",
            },
            f"{task_id} window_selection",
        )
        enumerated_at = _parse_timestamp(
            selection.get("enumerated_at"), f"{task_id} window enumerated_at"
        )
        if (
            selection.get("matching_window_count") != 1
            or selection.get("selected_window_id") != target_window["window_id"]
            or enumerated_at < registered_at
        ):
            raise GateError(f"{task_id} did not prove a unique target window")
        require_string(selection.get("selection_query"), f"{task_id} selection_query")
        require_sha256(selection.get("window_list_sha256"), f"{task_id} window_list_sha256")

        session_started = _parse_timestamp(
            record.get("session_started_at"), f"{task_id} session_started_at"
        )
        session_ended = _parse_timestamp(
            record.get("session_ended_at"), f"{task_id} session_ended_at"
        )
        target_process_started = _parse_timestamp(
            target_window["process_started_at"], f"{task_id} target process_started_at"
        )
        if (
            session_started < registered_at
            or session_ended < session_started
            or enumerated_at > session_started
            or target_process_started > session_started
        ):
            raise GateError(f"{task_id} CUA timing is outside the registered UI session")
        actions = require_list(record.get("actions"), f"{task_id} actions")
        if not actions:
            raise GateError(f"{task_id} must contain at least one real UI action")
        previous_end = session_started
        for index, raw_action in enumerate(actions, start=1):
            action = require_mapping(raw_action, f"{task_id} action[{index}]")
            _require_exact_keys(
                action,
                {
                    "sequence",
                    "actor",
                    "started_at",
                    "ended_at",
                    "before_state",
                    "action",
                    "after_state",
                    "observed_text",
                    "bound_machine_event_id",
                    "machine_evidence_path",
                    "machine_evidence_bytes",
                    "machine_evidence_sha256",
                    "codex_tool_record",
                },
                f"{task_id} action[{index}]",
            )
            if action.get("sequence") != index or action.get("actor") != "cua":
                raise GateError(f"{task_id} actions must be ordered CUA actions")
            action_started = _parse_timestamp(
                action.get("started_at"), f"{task_id} action[{index}] started_at"
            )
            action_ended = _parse_timestamp(
                action.get("ended_at"), f"{task_id} action[{index}] ended_at"
            )
            if (
                action_started < previous_end
                or action_ended < action_started
                or action_ended > session_ended
            ):
                raise GateError(f"{task_id} action[{index}] timing is invalid or overlaps")
            previous_end = action_ended
            for field in ("before_state", "action", "after_state", "observed_text"):
                require_string(action.get(field), f"{task_id} action[{index}] {field}")
            event_id = _safe_id(
                action.get("bound_machine_event_id"),
                f"{task_id} action[{index}] bound_machine_event_id",
            )
            machine_relative = normalize_relative_path(
                action.get("machine_evidence_path"),
                f"{task_id} action[{index}] machine_evidence_path",
            )
            if machine_relative not in CUA_ALLOWED_MACHINE_PATHS_BY_TASK[task_id]:
                raise GateError(f"{task_id} uses machine evidence outside the AT-to-FT allowlist")
            machine_path, _ = resolve_under(
                private_root, machine_relative, f"{task_id} machine evidence"
            )
            actual_bytes, actual_sha = hash_file(machine_path)
            if (
                action.get("machine_evidence_bytes") != actual_bytes
                or require_sha256(
                    action.get("machine_evidence_sha256"),
                    f"{task_id} action[{index}] machine evidence sha256",
                )
                != actual_sha
            ):
                raise GateError(f"{task_id} machine evidence hash binding is invalid")
            if machine_relative not in machine_documents:
                machine_document = require_mapping(
                    read_json(machine_path, f"{machine_relative} machine evidence"),
                    f"{machine_relative} machine evidence",
                )
                machine_documents[machine_relative] = (
                    machine_document,
                    actual_bytes,
                    actual_sha,
                )
            machine_document = machine_documents[machine_relative][0]
            events = require_list(
                machine_document.get("machine_events"),
                f"{machine_relative} machine_events",
            )
            matching_events = [
                item
                for item in events
                if isinstance(item, dict) and item.get("event_id") == event_id
            ]
            if len(matching_events) != 1:
                raise GateError(f"{task_id} bound machine event must exist exactly once")
            event = _normalize_machine_event(
                matching_events[0],
                label=f"{task_id} machine event {event_id}",
                evidence_path=machine_relative,
                context=context,
                registered_at=registered_at,
            )
            if (
                event["target_window"] != target_window
                or event["observed_version"] != observed_version
                or event["ended_at"] > session_started
            ):
                raise GateError(
                    f"{task_id} target window, version, PID instance, or non-overlap binding is invalid"
                )
            tool_record = require_mapping(
                action.get("codex_tool_record"), f"{task_id} action[{index}] Codex tool record"
            )
            _require_exact_keys(
                tool_record,
                {"thread_id", "turn_id", "item_id"},
                f"{task_id} action[{index}] Codex tool record",
            )
            for field in ("thread_id", "turn_id", "item_id"):
                _safe_id(
                    tool_record.get(field),
                    f"{task_id} action[{index}] Codex tool record {field}",
                )
            shared_event_usage.setdefault((machine_relative, event_id), set()).add(task_id)
            machine_event_binding_count += 1
        verified_records.append(
            {
                "task_id": task_id,
                "ui_run_id": ui_run_id,
                "record_bytes": record_bytes,
                "record_sha256": record_sha,
                "hash_manifest_bytes": manifest_bytes,
                "hash_manifest_sha256": manifest_sha,
            }
        )

    if len(set(observed_ui_run_ids)) != len(CUA_TASK_IDS):
        raise GateError("all 22 CUA ui_run_id values must be unique")
    for (machine_relative, _), task_ids in shared_event_usage.items():
        if len(task_ids) <= 1:
            continue
        declared_sets = [
            set(scope["task_ids"])
            for scope in CUA_MACHINE_EVIDENCE_ALLOWLIST
            if machine_relative in scope["paths"]
        ]
        if not any(task_ids.issubset(declared) for declared in declared_sets):
            raise GateError("machine evidence was reused by AT tasks outside the shared allowlist")
    for item in verified_records:
        task_id = item["task_id"]
        for relative, byte_key, hash_key in (
            (CUA_RECORD_PATH_BY_TASK[task_id], "record_bytes", "record_sha256"),
            (
                CUA_HASH_MANIFEST_PATH_BY_TASK[task_id],
                "hash_manifest_bytes",
                "hash_manifest_sha256",
            ),
        ):
            final_bytes, final_sha = hash_file(private_root / relative)
            if final_bytes != item[byte_key] or final_sha != item[hash_key]:
                raise GateError(f"{task_id} CUA evidence changed during verification")
    for relative, (_, expected_bytes, expected_sha) in machine_documents.items():
        final_bytes, final_sha = hash_file(private_root / relative)
        if final_bytes != expected_bytes or final_sha != expected_sha:
            raise GateError(f"{relative} machine evidence changed during CUA verification")
    return {
        "cua_records_verified": len(verified_records),
        "cua_ui_run_id_unique_count": len(set(observed_ui_run_ids)),
        "undeclared_shared_evidence": 0,
        "candidate_identity_exact": True,
        "machine_event_bindings_verified": machine_event_binding_count,
        "records": verified_records,
    }


def verify_acceptance(
    config_path: Path,
    repo: Path,
    public_report_path: Path,
    private_report_path: Path,
    *,
    required_ids: Sequence[str] = FORMAL_CASE_IDS,
    required_names: Mapping[str, str] | None = None,
    expected_run_keys: Mapping[str, str] | None = None,
    require_external_config: bool = True,
) -> dict[str, Any]:
    context = validate_config(
        config_path,
        repo,
        required_ids=required_ids,
        required_names=required_names,
        expected_run_keys=expected_run_keys,
        require_external_config=require_external_config,
    )
    public_report, public_hash, public_bytes = _stable_json(
        public_report_path.resolve(), "public acceptance report"
    )
    private_report, private_hash, private_bytes = _stable_json(
        private_report_path.resolve(), "private acceptance report"
    )
    if public_report.get("schema_version") != SCHEMA_VERSION or public_report.get("stage") != PUBLIC_STAGE:
        raise GateError("public acceptance report schema/stage is invalid")
    if private_report.get("schema_version") != SCHEMA_VERSION or private_report.get("stage") != PRIVATE_STAGE:
        raise GateError("private acceptance report schema/stage is invalid")
    _privacy_audit(public_report, context)
    expected_registration_sha = (
        context["run_registration"]["sha256"]
        if context["run_registration"] is not None
        else None
    )
    for label, report in (("public", public_report), ("private", private_report)):
        if report.get("run_id") != context["run_id"]:
            raise GateError(f"{label} report run_id does not match the config")
        if report.get("machine_run_id") != context["machine_run_id"]:
            raise GateError(f"{label} report machine_run_id does not match the config")
        if report.get("execution_contract") != context["execution_contract"]:
            raise GateError(f"{label} report execution_contract does not match the config")
        if report.get("run_registration_sha256") != expected_registration_sha:
            raise GateError(f"{label} report run registration does not match the config")
        if report.get("producer_nonces") != context["producer_nonces"]:
            raise GateError(f"{label} report producer nonces do not match the config")
        if report.get("source_commit") != context["source_commit"]:
            raise GateError(f"{label} report source_commit does not match the current HEAD")
        if report.get("config_sha256") != context["sha256"] or report.get("config_bytes") != context["bytes"]:
            raise GateError(f"{label} report config binding is invalid")
        if report.get("status") != PASS:
            raise GateError(f"{label} acceptance report is not PASS")
    if (
        public_report.get("candidate_sha256") != context["candidate"]["sha256"]
        or public_report.get("candidate_bytes") != context["candidate"]["bytes"]
        or public_report.get("build_manifest_sha256") != context["build_manifest"]["sha256"]
        or public_report.get("build_manifest_bytes") != context["build_manifest"]["bytes"]
    ):
        raise GateError("public report candidate/build binding is invalid")
    private_candidate = require_mapping(private_report.get("candidate"), "private candidate")
    private_manifest = require_mapping(private_report.get("build_manifest"), "private build manifest")
    if (
        private_candidate.get("sha256") != context["candidate"]["sha256"]
        or private_candidate.get("bytes") != context["candidate"]["bytes"]
        or private_candidate.get("path") != str(context["candidate"]["path"])
        or private_manifest.get("sha256") != context["build_manifest"]["sha256"]
        or private_manifest.get("bytes") != context["build_manifest"]["bytes"]
        or private_manifest.get("path") != str(context["build_manifest"]["path"])
    ):
        raise GateError("private report candidate/build binding is invalid")
    if private_report.get("config_path") != str(context["path"]) or private_report.get(
        "evidence_roots"
    ) != {"public": str(context["public_root"]), "private": str(context["private_root"])}:
        raise GateError("private report path binding is invalid")
    if public_report.get("private_report_sha256") != private_hash or public_report.get(
        "private_report_bytes"
    ) != private_bytes:
        raise GateError("public report is not bound to the supplied private report")
    public_integrity = require_mapping(public_report.get("integrity_checks"), "integrity checks")
    if any(value is not True for value in public_integrity.values()):
        raise GateError("acceptance run input integrity checks did not all pass")
    if private_report.get("integrity_checks") != public_integrity:
        raise GateError("public and private integrity checks do not match")
    expected_inputs = sorted(
        {
            (item["bytes"], item["sha256"])
            for scenario in context["scenarios"]
            for item in scenario["inputs"]
        }
    )
    if public_report.get("input_files") != [
        {"bytes": byte_count, "sha256": digest} for byte_count, digest in expected_inputs
    ]:
        raise GateError("public report input-file binding is invalid")
    before_snapshots = require_mapping(
        private_report.get("bound_file_snapshots_before"), "private before snapshots"
    )
    after_snapshots = require_mapping(
        private_report.get("bound_file_snapshots_after"), "private after snapshots"
    )
    if before_snapshots != after_snapshots:
        raise GateError("private report shows a bound file changed during the run")
    expected_snapshot_bindings: dict[str, tuple[int, str]] = {
        "config": (context["bytes"], context["sha256"]),
        "candidate": (context["candidate"]["bytes"], context["candidate"]["sha256"]),
        "build_manifest": (
            context["build_manifest"]["bytes"],
            context["build_manifest"]["sha256"],
        ),
    }
    for scenario in context["scenarios"]:
        for index, item in enumerate(scenario["inputs"]):
            expected_snapshot_bindings[f"input:{scenario['id']}:{index}"] = (
                item["bytes"],
                item["sha256"],
            )
    if set(before_snapshots) != set(expected_snapshot_bindings):
        raise GateError("private report bound-file snapshot set is incomplete")
    for name, (byte_count, digest) in expected_snapshot_bindings.items():
        snapshot = require_mapping(before_snapshots[name], f"snapshot {name}")
        if snapshot.get("bytes") != byte_count or snapshot.get("sha256") != digest:
            raise GateError("private report bound-file snapshot is invalid")

    public_scenarios = [
        require_mapping(item, "public scenario")
        for item in require_list(public_report.get("scenarios"), "public scenarios")
    ]
    private_scenarios = [
        require_mapping(item, "private scenario")
        for item in require_list(private_report.get("scenarios"), "private scenarios")
    ]
    public_by_id = {str(item.get("id")): item for item in public_scenarios}
    private_by_id = {str(item.get("id")): item for item in private_scenarios}
    if public_report.get("required_cases") != list(required_ids):
        raise GateError("public report required-case contract is incorrect")
    if (
        set(public_by_id) != set(required_ids)
        or len(public_scenarios) != len(required_ids)
        or set(private_by_id) != set(required_ids)
        or len(private_scenarios) != len(required_ids)
    ):
        raise GateError("acceptance reports do not contain all required cases exactly once")
    expected_stats = {
        "total": len(required_ids),
        "pass": len(required_ids),
        "fail": 0,
        "not_run": 0,
        "run_once_total": len(context["run_keys"]),
    }
    if public_report.get("statistics") != expected_stats or private_report.get("statistics") != expected_stats:
        raise GateError("acceptance report statistics are incorrect")
    expected_run_counts = {key: 1 for key in context["run_keys"]}
    if public_report.get("producer_run_counts") != expected_run_counts or private_report.get(
        "producer_run_counts"
    ) != expected_run_counts:
        raise GateError("run_once producer counts are incorrect")
    group_runs = require_list(private_report.get("group_runs"), "private group runs")
    if [item.get("run_once_key") for item in group_runs if isinstance(item, dict)] != list(
        context["run_keys"]
    ):
        raise GateError("private report does not contain one execution per run_once group")
    group_run_by_key = {
        str(item["run_once_key"]): require_mapping(item, "private group run")
        for item in group_runs
        if isinstance(item, dict) and "run_once_key" in item
    }
    for run_key in context["run_keys"]:
        representative = next(
            item for item in context["scenarios"] if item["run_once_key"] == run_key
        )
        group_run = group_run_by_key[run_key]
        if group_run.get("machine_run_id") != context["machine_run_id"]:
            raise GateError(f"{run_key} group machine_run_id does not match the config")
        if group_run.get("producer_nonce") != context["producer_nonces"].get(run_key):
            raise GateError(f"{run_key} group producer nonce does not match the config")
        if group_run.get("rerun_scope_on_failure") != GROUP_RERUN_SCOPE:
            raise GateError(f"{run_key} group rerun scope is not the whole run_once group")
        _verify_private_command(
            require_mapping(group_run.get("producer"), f"{run_key} producer"),
            representative["command"],
            f"{run_key} producer",
        )
        _verify_private_command(
            require_mapping(group_run.get("cleanup"), f"{run_key} cleanup"),
            representative["cleanup"],
            f"{run_key} cleanup",
        )

    roots = {"public": context["public_root"], "private": context["private_root"]}
    for scenario in context["scenarios"]:
        case_id = scenario["id"]
        public_item = public_by_id[case_id]
        private_item = private_by_id[case_id]
        for label, item in (("public", public_item), ("private", private_item)):
            if item.get("machine_run_id") != context["machine_run_id"]:
                raise GateError(f"{case_id} {label} machine_run_id does not match the config")
            if item.get("producer_nonce") != context["producer_nonces"].get(
                scenario["run_once_key"]
            ):
                raise GateError(f"{case_id} {label} producer nonce does not match the config")
            if item.get("rerun_scope_on_failure") != GROUP_RERUN_SCOPE:
                raise GateError(f"{case_id} {label} rerun scope is not the whole run_once group")
        if public_item.get("status") != PASS or private_item.get("status") != PASS:
            raise GateError(f"forged or incomplete PASS for {case_id}")
        for key in ("producer", "cleanup"):
            if not isinstance(public_item.get(key), dict) or public_item[key].get("status") != PASS:
                raise GateError(f"{case_id} {key} is not PASS")
            if not isinstance(private_item.get(key), dict) or private_item[key].get("status") != PASS:
                raise GateError(f"{case_id} private {key} is not PASS")
        group_run = group_run_by_key[scenario["run_once_key"]]
        if private_item.get("producer") != group_run.get("producer") or private_item.get(
            "cleanup"
        ) != group_run.get("cleanup"):
            raise GateError(f"{case_id} private command record is not the run_once execution")
        if public_item.get("producer") != _public_command_view(group_run["producer"]) or public_item.get(
            "cleanup"
        ) != _public_command_view(group_run["cleanup"]):
            raise GateError(f"{case_id} public command summary is inconsistent")
        _verify_evidence_records(
            require_list(public_item.get("public_evidence"), f"{case_id} public evidence"),
            scenario["evidence"]["public"],
            f"{case_id} public",
        )
        _verify_evidence_records(
            require_list(private_item.get("private_evidence"), f"{case_id} private evidence"),
            scenario["evidence"]["private"],
            f"{case_id} private",
        )
        if (
            public_item.get("missing_public_evidence")
            or public_item.get("stale_public_evidence")
            or public_item.get("changed_public_evidence_after_cleanup")
            or public_item.get("missing_private_evidence_count") != 0
            or public_item.get("stale_private_evidence_count") != 0
            or public_item.get("changed_private_evidence_after_cleanup_count") != 0
            or private_item.get("changed_private_evidence_after_cleanup")
        ):
            raise GateError(f"{case_id} PASS has missing, stale, or cleanup-modified evidence")
        pass_checks, _ = _evaluate_assertions(scenario["pass_assertions"], roots)
        fail_checks, _ = _evaluate_assertions(scenario["fail_assertions"], roots)
        cleanup_checks, _ = _evaluate_assertions(scenario["cleanup"]["checks"], roots)
        if not all(item["passed"] for item in pass_checks) or any(
            item["passed"] for item in fail_checks
        ) or not all(item["passed"] for item in cleanup_checks):
            raise GateError(f"{case_id} evidence no longer satisfies its assertions")
        if context["formal"]:
            result_document = require_mapping(
                read_json(
                    context["public_root"] / f"{case_id}/result.public.json",
                    f"{case_id} public result",
                ),
                f"{case_id} public result",
            )
            produced_at = _parse_timestamp(
                result_document.get("produced_at"), f"{case_id} produced_at"
            )
            registered_at = _parse_timestamp(
                context["run_registration"]["document"].get("registered_at"),
                "run registration registered_at",
            )
            if produced_at < registered_at:
                raise GateError(f"{case_id} evidence predates the formal run registration")
        if public_item.get("pass_assertions") != pass_checks or public_item.get(
            "fail_assertions"
        ) != fail_checks or public_item.get("cleanup_checks") != cleanup_checks:
            raise GateError(f"{case_id} recorded assertion results are forged")

    cua_report = _verify_cua_records(context) if context["formal"] else None
    producer_nonce_unique_count = len(set(context["producer_nonces"].values()))

    return {
        "schema_version": SCHEMA_VERSION,
        "stage": VERIFY_STAGE,
        "generated_at": utc_now(),
        "status": PASS,
        "run_id": context["run_id"],
        "machine_run_id": context["machine_run_id"],
        "execution_contract": context["execution_contract"],
        "source_commit": context["source_commit"],
        "config_bytes": context["bytes"],
        "config_sha256": context["sha256"],
        "candidate_bytes": context["candidate"]["bytes"],
        "candidate_sha256": context["candidate"]["sha256"],
        "public_report_bytes": public_bytes,
        "public_report_sha256": public_hash,
        "private_report_bytes": private_bytes,
        "private_report_sha256": private_hash,
        "statistics": expected_stats,
        "cua_records_verified": (
            cua_report["cua_records_verified"] if cua_report is not None else 0
        ),
        "producer_nonce_unique_count": producer_nonce_unique_count,
        "cua_ui_run_id_unique_count": (
            cua_report["cua_ui_run_id_unique_count"] if cua_report is not None else 0
        ),
        "cua": cua_report,
        "checks": {
            "all_cases_pass": True,
            "all_public_and_private_evidence_rehashed": True,
            "all_assertions_recomputed": True,
            "config_candidate_inputs_and_source_still_bound": True,
            "public_privacy_fields_absent": True,
            "run_once_counts_exact": True,
            "run_id_registry_unique": context["run_registration"] is not None
            if context["formal"]
            else True,
            "producer_nonce_unique_count_is_seven": producer_nonce_unique_count == 7
            if context["formal"]
            else True,
            "cua_records_verified_is_22": cua_report is not None
            and cua_report["cua_records_verified"] == 22
            if context["formal"]
            else True,
            "cua_ui_run_id_unique_count_is_22": cua_report is not None
            and cua_report["cua_ui_run_id_unique_count"] == 22
            if context["formal"]
            else True,
            "undeclared_shared_evidence_zero": cua_report is not None
            and cua_report["undeclared_shared_evidence"] == 0
            if context["formal"]
            else True,
        },
    }


def _validation_report(context: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "schema_version": SCHEMA_VERSION,
        "stage": "MOSS_FUNCTIONAL_FIX_ACCEPTANCE_CONFIG_VALIDATION",
        "generated_at": utc_now(),
        "status": PASS,
        "run_id": context["run_id"],
        "machine_run_id": context["machine_run_id"],
        "execution_contract": context["execution_contract"],
        "run_registration_sha256": (
            context["run_registration"]["sha256"]
            if context["run_registration"] is not None
            else None
        ),
        "producer_nonce_unique_count": len(set(context["producer_nonces"].values())),
        "source_commit": context["source_commit"],
        "config_bytes": context["bytes"],
        "config_sha256": context["sha256"],
        "candidate_bytes": context["candidate"]["bytes"],
        "candidate_sha256": context["candidate"]["sha256"],
        "build_manifest_bytes": context["build_manifest"]["bytes"],
        "build_manifest_sha256": context["build_manifest"]["sha256"],
        "scenario_count": len(context["scenarios"]),
        "run_once_count": len(context["run_keys"]),
        "checks": {
            "exact_ft_numbering": True,
            "no_obsolete_or_placeholder_content": True,
            "real_file_hashes_match": True,
            "dependencies_acyclic": True,
            "shared_run_once_definitions_match": True,
            "whole_group_debug_rerun_required": True,
            "formal_failure_requires_new_full_run": True,
            "ft26_dependency_chain_exact": True,
            "formal_schema_bindings_match": context["schema_bindings"] is not None
            if context["formal"]
            else True,
            "run_id_registry_unique": context["run_registration"] is not None
            if context["formal"]
            else True,
            "seven_unique_producer_nonces": len(set(context["producer_nonces"].values())) == 7
            if context["formal"]
            else True,
            "cua_22_record_contract_exact": context["cua_contract"] == FORMAL_CUA_CONTRACT
            if context["formal"]
            else True,
            "evidence_paths_contained": True,
            "cleanup_and_assertions_present": True,
        },
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    default_repo = Path(__file__).resolve().parents[2]

    register = subparsers.add_parser(
        "register-run",
        help="atomically reserve one formal machine_run_id before AT-00",
    )
    register.add_argument("--repo", type=Path, default=default_repo)
    register.add_argument("--candidate", type=Path, required=True)
    register.add_argument("--build-manifest", type=Path, required=True)
    register.add_argument("--run-id", required=True)
    register.add_argument("--run-registry", type=Path, required=True)
    register.add_argument("--public-evidence-root", type=Path, required=True)
    register.add_argument("--private-evidence-root", type=Path, required=True)

    materialize = subparsers.add_parser("materialize", help="bind the template to the final build")
    materialize.add_argument("--repo", type=Path, default=default_repo)
    materialize.add_argument("--template", type=Path, required=True)
    materialize.add_argument("--output", type=Path, required=True)
    materialize.add_argument("--candidate", type=Path, required=True)
    materialize.add_argument("--build-manifest", type=Path, required=True)
    materialize.add_argument("--run-id", required=True)
    materialize.add_argument("--public-evidence-root", type=Path, required=True)
    materialize.add_argument("--private-evidence-root", type=Path, required=True)
    materialize.add_argument("--run-registry", type=Path, required=True)
    materialize.add_argument("--token", action="append", default=[], metavar="NAME=VALUE")
    materialize.add_argument("--token-json", type=Path)

    validate = subparsers.add_parser("validate", help="reject an unsafe or incomplete config")
    validate.add_argument("--repo", type=Path, default=default_repo)
    validate.add_argument("--config", type=Path, required=True)
    validate.add_argument("--output", type=Path, required=True)

    run = subparsers.add_parser(
        "run",
        help="run the seven producers and evaluate all 28 cases",
        description=(
            "Run all seven producer groups and evaluate all 28 cases. "
            "There is no single-case formal release mode."
        ),
    )
    run.add_argument("--repo", type=Path, default=default_repo)
    run.add_argument("--config", type=Path, required=True)
    run.add_argument("--public-output", type=Path, required=True)
    run.add_argument("--private-output", type=Path, required=True)

    verify = subparsers.add_parser("verify", help="rehash and recompute a claimed PASS")
    verify.add_argument("--repo", type=Path, default=default_repo)
    verify.add_argument("--config", type=Path, required=True)
    verify.add_argument("--public-report", type=Path, required=True)
    verify.add_argument("--private-report", type=Path, required=True)
    verify.add_argument("--output", type=Path, required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.command == "register-run":
            registration = register_run_id(
                repo=args.repo,
                registry_root=args.run_registry,
                run_id=args.run_id,
                candidate_path=args.candidate,
                build_manifest_path=args.build_manifest,
                public_root=args.public_evidence_root,
                private_root=args.private_evidence_root,
            )
            print(
                json.dumps(
                    {
                        "status": PASS,
                        "machine_run_id": registration["document"]["machine_run_id"],
                        "run_registration_bytes": registration["bytes"],
                        "run_registration_sha256": registration["sha256"],
                    },
                    ensure_ascii=False,
                )
            )
            return 0
        if args.command == "materialize":
            context = materialize_config(
                repo=args.repo,
                template_path=args.template,
                output_path=args.output,
                candidate_path=args.candidate,
                build_manifest_path=args.build_manifest,
                run_id=args.run_id,
                public_root=args.public_evidence_root,
                private_root=args.private_evidence_root,
                tokens=_token_values(args.token, args.token_json),
                run_registry_root=args.run_registry,
            )
            print(
                json.dumps(
                    {
                        "status": PASS,
                        "machine_run_id": context["machine_run_id"],
                        "source_commit": context["source_commit"],
                        "config_sha256": context["sha256"],
                        "scenario_count": len(context["scenarios"]),
                    },
                    ensure_ascii=False,
                )
            )
            return 0
        if args.command == "validate":
            context = validate_config(args.config, args.repo)
            output = _output_path(context["public_root"], args.output, "validation report")
            report = _validation_report(context)
            atomic_write_json(output, report)
            print(json.dumps(report, ensure_ascii=False))
            return 0
        if args.command == "run":
            exit_code, public_report, _ = run_acceptance(
                args.config, args.repo, args.public_output, args.private_output
            )
            print(
                json.dumps(
                    {
                        "status": public_report["status"],
                        "machine_run_id": public_report["machine_run_id"],
                        "statistics": public_report["statistics"],
                        "config_sha256": public_report["config_sha256"],
                    },
                    ensure_ascii=False,
                )
            )
            return exit_code
        if args.command == "verify":
            context = validate_config(args.config, args.repo)
            output = _output_path(context["public_root"], args.output, "verification report")
            report = verify_acceptance(
                args.config, args.repo, args.public_report, args.private_report
            )
            atomic_write_json(output, report)
            print(json.dumps(report, ensure_ascii=False))
            return 0
        raise GateError("unsupported command")
    except GateError as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
