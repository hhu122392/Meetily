#!/usr/bin/env python3
"""Prepare, score, and verify the MOSS functional-fix short quality gate.

This program is deliberately a producer/scorer, not a replacement for a real
MOSS run or a human reference.  ``prepare`` deterministically extracts the
frozen 70.370-296.810 second PCM window.  ``score`` recomputes every metric from
bound, current-run artifacts.  ``verify`` repeats the computation and rejects a
report whose status, metrics, provenance, or input hashes were edited later.

Only the Python standard library and the repository's shared P6 safety helpers
are used.
"""

from __future__ import annotations

import argparse
import csv
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import secrets
import sqlite3
import struct
import subprocess
import sys
import time
import unicodedata
import wave
from typing import Any, Sequence

from moss_v3_p6_common import (
    FAIL,
    PASS,
    GateError,
    canonical_json_bytes,
    file_record,
    hash_file,
    read_json,
    reject_symlink,
    require_git_commit,
    require_list,
    require_mapping,
    require_sha256,
    require_string,
    sha256_bytes,
    utc_now,
)


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")


SCHEMA_VERSION = 1
PREPARE_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_PREPARED_WINDOW"
BINDINGS_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_RUN_BINDINGS"
REPORT_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_SHORT_QUALITY_GATE"
PRIVATE_REPORT_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_SHORT_QUALITY_GATE_PRIVATE"
CANDIDATE_BUILD_MANIFEST_STAGE = "MOSS_FUNCTIONAL_INSTALL_BUILD_MANIFEST"
APPROVED_PRODUCT_NAME = "meetily-p6-lifecycle"
APPROVED_BUNDLE_ID = "com.meetily.ai.p6lifecycle"

SOURCE_SHA256 = "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB"
SOURCE_BYTES = 23_607_374
SOURCE_FRAMES = 11_803_648
SOURCE_DURATION_SECONDS = 737.728
WINDOW_START_SECONDS = 70.370
WINDOW_END_SECONDS = 296.810
WINDOW_DURATION_SECONDS = 226.440
SAMPLE_RATE_HZ = 16_000
CHANNELS = 1
SAMPLE_WIDTH_BYTES = 2
WINDOW_START_FRAME = 1_125_920
WINDOW_FRAME_COUNT = 3_623_040
WINDOW_BYTES = 7_246_124
WINDOW_SHA256 = "5128D52F41A6FA58BBD9387C6C68D09E076E9E782C8C13B8BF0CF5BB8655AD22"

# These are not user-entered approvals.  They are the exact, already frozen
# files from the completed 226.440 second human review.  Formal scoring accepts
# no substitute with the same shape but different bytes.
FROZEN_TRUTH_MANIFEST_BYTES = 2_854
FROZEN_TRUTH_MANIFEST_SHA256 = "1C9639F4E0AF6FA6C11BA3F5DC4674F6B227FC47A1323417E5D6846F5DDDCEB3"
FROZEN_HUMAN_VERBATIM_BYTES = 8_136
FROZEN_HUMAN_VERBATIM_SHA256 = "7B81C3D80043BD2CFA85EE892058BC4E432F3BA67DC18FD92FF0A0B8D97DD4FC"
FROZEN_SPEAKER_TRUTH_BYTES = 1_840
FROZEN_SPEAKER_TRUTH_SHA256 = "24A557080539EE02922B83165BD034264ED55E07940C5065F875A91CC897452D"
FROZEN_HUMAN_REVIEW_BYTES = 1_776
FROZEN_HUMAN_REVIEW_SHA256 = "01A51256FD4A19690270D7D6A78E9608FF60DAB36BCC25676DB2EC02ABA3AEB4"
FROZEN_PREMEETING_CONTEXT_BYTES = 10_793
FROZEN_PREMEETING_CONTEXT_SHA256 = "DED0E66A52F09B031A4F21AF6A0119FF7D1EA0BCD238968A1161CC871FD0D7CD"
FROZEN_PACKAGE_MANIFEST_BYTES = 4_630
FROZEN_PACKAGE_MANIFEST_SHA256 = "D9F9476D231ED1D82487AFE03D02D98E2DB84FD2490FC47ABFC3B75B75E410A4"
FROZEN_REVIEW_PROVENANCE_BYTES = 3_252
FROZEN_REVIEW_PROVENANCE_SHA256 = "2AAD12F4B701769FE8890CEB926EF5EB9E926170506F84297786390D8FBEC5D6"
FROZEN_POSITIVE_TRUTH_BYTES = 6_363
FROZEN_POSITIVE_TRUTH_SHA256 = "0F9AC1B5FE8EBD75995EC70118D32BAD2889C28C62803C0D031B489D072549B5"
FROZEN_NEGATIVE_TRUTH_BYTES = 1_605
FROZEN_NEGATIVE_TRUTH_SHA256 = "41A3FA60A6A2F6775C14E58D9E37E58480D9BA64D5541F4CF5412FA3697D83F7"

MODEL_CONTRACTS = {
    "moss": {
        "filename": "MOSS-Transcribe-Diarize-Q8_0.gguf",
        "bytes": 986_899_616,
        "sha256": "64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039",
    },
    "whisper": {
        "filename": "ggml-large-v3-turbo-q5_0.bin",
        "bytes": 574_041_195,
        "sha256": "394221709CD5AD1F40C46E6031CA61BCE88931E6E088C188294C6D5A55FFA7E2",
    },
    "qwen_2b": {
        "filename": "Qwen3.5-2B-Q4_K_M.gguf",
        "model_name": "qwen3.5:2b",
        "bytes": 1_280_835_840,
        "sha256": "AAF42C8B7C3CAB2BF3D69C355048D4A0EE9973D48F16C731C0520EE914699223",
    },
}
FAIR_COMPARISON_LANGUAGE = "zh-CN"
MOSS_NATIVE_DECODE_PARAMETERS_CONTRACT = {
    "language": "zh",
    "timestamps": "segment",
    "diarize": "on",
}
TRANSCRIPTION_DECODE_PARAMETERS = {
    "whisper": {
        "product_action": "start_import_audio_command",
        "provider": "localWhisper",
        "model": "large-v3-turbo-q5_0",
        "audio_decoder": "Meetily decode_audio_file + to_whisper_format",
        "sample_rate_hz": SAMPLE_RATE_HZ,
        "channels": CHANNELS,
        "language": FAIR_COMPARISON_LANGUAGE,
        "timeout_ms": 3_600_000,
    },
}
TRANSCRIPTION_LANGUAGE_SOURCES = {
    "moss": "api_moss_get_workspace.runs",
    "whisper": "EXPLICIT_IMPORT_REQUEST",
}
PROGRAM_CONTRACTS = {
    "main_executable": "meetily.exe",
    "llama_helper": "llama-helper.exe",
    "moss_helper": "moss-helper.exe",
    "ffmpeg": "ffmpeg.exe",
    "directml": "DirectML.dll",
    "webview2": "runtime/webview2-fixed/msedgewebview2.exe",
    "uninstaller": "uninstall.exe",
}
FORMAL_SESSION_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_GATE_SESSION"
FORMAL_RECEIPT_STAGE = "MOSS_FUNCTIONAL_FIX_Q00_GATE_EXECUTION_RECEIPT"
NONCE_RE = re.compile(r"^[0-9a-f]{32}$")

POSITIVE_ATTESTATION = (
    "I listened to the frozen Q00 window and confirmed each listed spoken-term "
    "occurrence count against the human verbatim reference."
)
NEGATIVE_ATTESTATION = (
    "I listened to the frozen Q00 window and confirmed that every listed negative "
    "term was not spoken in this window."
)

HUMAN_VERBATIM_HEADER = (
    "segment_id",
    "start_ms",
    "end_ms",
    "reference_speaker_id",
    "verbatim_text",
    "overlap",
    "non_speech",
    "valid_for_scoring",
    "reviewer_note",
)
SPEAKER_TRUTH_HEADER = (
    "turn_id",
    "start_ms",
    "end_ms",
    "reference_speaker_id",
    "overlap",
    "valid_for_scoring",
    "note",
)

ARTIFACT_ORIGINS = {
    "window_audio": "PREPARED_WINDOW",
    "window_manifest": "PREPARED_WINDOW",
    "moss_raw": "CURRENT_RUN",
    "whisper_same_window": "CURRENT_RUN",
    "corrected": "CURRENT_RUN",
    "human_verbatim": "FROZEN_TRUTH",
    "speaker_truth": "FROZEN_TRUTH",
    "positive_truth": "FROZEN_TRUTH",
    "negative_truth": "FROZEN_TRUTH",
    "activation_evidence": "CURRENT_RUN",
    "summary_evidence": "CURRENT_RUN",
}
CURRENT_JSON_ROLES = {
    "moss_raw",
    "whisper_same_window",
    "corrected",
    "activation_evidence",
    "summary_evidence",
}
TRANSCRIPT_ROLES = {
    "moss_raw": "MOSS",
    "whisper_same_window": "WHISPER",
    "corrected": "MOSS_CORRECTED",
}
RUN_ID_RE = re.compile(r"^Q00-[0-9a-f]{12}-[A-Za-z0-9][A-Za-z0-9._-]{2,63}$")


class QualityGateError(GateError):
    """The Q00 evidence is missing, unsafe, malformed, stale, or inconsistent."""


def _finite_number(value: Any, label: str, *, minimum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise QualityGateError(f"{label} must be a finite number")
    number = float(value)
    if not math.isfinite(number):
        raise QualityGateError(f"{label} must be a finite number")
    if minimum is not None and number < minimum:
        raise QualityGateError(f"{label} must be at least {minimum}")
    return number


def _integer(value: Any, label: str, *, minimum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise QualityGateError(f"{label} must be an integer")
    if minimum is not None and value < minimum:
        raise QualityGateError(f"{label} must be at least {minimum}")
    return value


def _parse_iso8601(value: Any, label: str) -> datetime:
    text = require_string(value, label)
    if not text.endswith("Z"):
        raise QualityGateError(f"{label} must be an ISO-8601 UTC timestamp ending in Z")
    try:
        parsed = datetime.fromisoformat(text[:-1] + "+00:00")
    except ValueError as exc:
        raise QualityGateError(f"{label} is not a valid ISO-8601 timestamp") from exc
    if parsed.tzinfo is None or parsed.utcoffset() != timezone.utc.utcoffset(parsed):
        raise QualityGateError(f"{label} must use UTC")
    return parsed


def _git_head(repo: Path) -> str:
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repo,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise QualityGateError(f"Cannot read Git HEAD from repository: {repo}")
    return require_git_commit(completed.stdout.strip(), "Git HEAD")


def _require_current_commit(repo: Path, source_commit: str) -> str:
    expected = require_git_commit(source_commit, "source_commit")
    resolved_repo = repo.resolve(strict=True)
    actual = _git_head(resolved_repo)
    if actual != expected:
        raise QualityGateError(
            f"source_commit is not the current repository HEAD: expected {expected}, got {actual}"
        )
    clean = subprocess.run(
        ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all"],
        cwd=resolved_repo,
        capture_output=True,
        check=False,
    )
    if clean.returncode != 0:
        raise QualityGateError("Cannot verify the complete repository state for Q00")
    if clean.stdout:
        raise QualityGateError(
            "Repository is dirty (tracked, staged, or untracked files exist); freeze and commit or remove them before Q00"
        )
    for critical in (Path(__file__).resolve(), Path(__file__).with_name("moss_v3_p6_common.py").resolve()):
        try:
            relative = critical.relative_to(resolved_repo).as_posix()
        except ValueError as exc:
            raise QualityGateError(f"Q00 scoring code is outside the declared repository: {critical}") from exc
        tracked = subprocess.run(
            ["git", "ls-files", "--error-unmatch", "--", relative],
            cwd=resolved_repo,
            capture_output=True,
            check=False,
        )
        if tracked.returncode != 0:
            raise QualityGateError(
                f"Q00 scoring dependency is not recorded in source_commit: {relative}"
            )
    return expected


def _same_path(left: Path, right: Path) -> bool:
    return os.path.normcase(os.path.abspath(left)) == os.path.normcase(os.path.abspath(right))


def _lexical_absolute(path: Path) -> Path:
    """Return an absolute path while preserving links long enough to reject them."""

    lexical = Path(os.path.abspath(path))
    boundary = Path(lexical.anchor) if lexical.anchor else None
    reject_symlink(lexical, boundary=boundary)
    return lexical


def _require_new_outputs(paths: Sequence[Path]) -> None:
    normalized = [os.path.normcase(os.path.abspath(path)) for path in paths]
    if len(set(normalized)) != len(normalized):
        raise QualityGateError("Output paths must be different files")
    for path in paths:
        if path.exists() or path.is_symlink():
            raise QualityGateError(f"Refusing to overwrite existing output: {path}")
        path.parent.mkdir(parents=True, exist_ok=True)
        _lexical_absolute(path.parent)


def _json_file_bytes(value: Any) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode("utf-8")
        + b"\n"
    )


def _write_json_exclusive(path: Path, value: Any) -> None:
    encoded = _json_file_bytes(value)
    created = False
    try:
        with path.open("xb") as stream:
            created = True
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
    except Exception:
        if created and path.exists():
            path.unlink()
        raise


def _with_integrity(document: dict[str, Any]) -> dict[str, Any]:
    if "integrity" in document:
        raise QualityGateError("Internal report already contains integrity data")
    result = dict(document)
    result["integrity"] = {
        "algorithm": "SHA-256",
        "canonical_payload_sha256": sha256_bytes(canonical_json_bytes(document)),
    }
    return result


def _safe_file_record(path: Path) -> dict[str, Any]:
    lexical = _lexical_absolute(path)
    if not lexical.is_file():
        raise QualityGateError(f"Required regular file is missing: {lexical}")
    record = file_record(lexical)
    return {"bytes": record["bytes"], "sha256": record["sha256"]}


def _write_pcm_wav_exclusive(path: Path, frames: bytes) -> None:
    data_size = len(frames)
    byte_rate = SAMPLE_RATE_HZ * CHANNELS * SAMPLE_WIDTH_BYTES
    block_align = CHANNELS * SAMPLE_WIDTH_BYTES
    header = struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF",
        36 + data_size,
        b"WAVE",
        b"fmt ",
        16,
        1,
        CHANNELS,
        SAMPLE_RATE_HZ,
        byte_rate,
        block_align,
        SAMPLE_WIDTH_BYTES * 8,
        b"data",
        data_size,
    )
    with path.open("xb") as stream:
        stream.write(header)
        stream.write(frames)
        stream.flush()
        os.fsync(stream.fileno())


def _repository_status_record(repo: Path) -> dict[str, Any]:
    completed = subprocess.run(
        ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all"],
        cwd=repo,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise QualityGateError("Cannot capture complete repository status")
    entries = [item for item in completed.stdout.split(b"\0") if item]
    return {
        "format": "git-status-porcelain-v1-z-with-all-untracked",
        "entry_count": len(entries),
        "raw_sha256": sha256_bytes(completed.stdout),
    }


def begin_formal_session(
    *,
    repo: Path,
    source_commit: str,
    private_root: Path,
    public_root: Path,
    formal_runner_path: Path,
    node_path: Path,
    cdp_script_path: Path,
) -> dict[str, Any]:
    commit = _require_current_commit(repo, source_commit)
    repository = repo.resolve(strict=True)
    formal_runner = formal_runner_path.resolve(strict=True)
    node_runtime = node_path.resolve(strict=True)
    cdp_script = cdp_script_path.resolve(strict=True)
    if formal_runner != (repository / "scripts" / "qa" / "moss_functional_fix_q00_formal.py"):
        raise QualityGateError("Formal Q00 runner must use the fixed repository script")
    if cdp_script != (repository / "scripts" / "qa" / "moss-functional-ft-cdp.mjs"):
        raise QualityGateError("Formal Q00 CDP runner must use the fixed repository script")
    private = _lexical_absolute(private_root)
    public = _lexical_absolute(public_root)
    if not private.is_dir() or not public.is_dir():
        raise QualityGateError("Private and public roots must already exist before begin")
    if _paths_overlap(private, public):
        raise QualityGateError("Private and public roots must be disjoint")
    nonce = secrets.token_hex(16)
    run_id = f"Q00-{commit[:12]}-{nonce}"
    run_directory = private / run_id
    try:
        run_directory.mkdir(mode=0o700)
    except FileExistsError as exc:
        raise QualityGateError("Random Q00 run directory unexpectedly already exists") from exc
    started_at = utc_now()
    payload = {
        "schema_version": 1,
        "stage": FORMAL_SESSION_STAGE,
        "status": "STARTED",
        "created_by_gate": True,
        "source_commit": commit,
        "run_id": run_id,
        "nonce": nonce,
        "started_at": started_at,
        "started_monotonic_ns": time.monotonic_ns(),
        "maximum_moss_seconds": 1_800,
        "private_root": str(private.resolve(strict=True)),
        "public_root": str(public.resolve(strict=True)),
        "run_directory": str(run_directory.resolve(strict=True)),
        "repository_status_before": _repository_status_record(repository),
        "gate_program": _safe_file_record(Path(__file__).resolve(strict=True)),
        "formal_runner": {"path": str(formal_runner), **_safe_file_record(formal_runner)},
        "node_runtime": {"path": str(node_runtime), **_safe_file_record(node_runtime)},
        "cdp_runner": {"path": str(cdp_script), **_safe_file_record(cdp_script)},
    }
    if payload["repository_status_before"]["entry_count"] != 0:
        raise QualityGateError("Repository became dirty while starting Q00")
    document = _with_integrity(payload)
    session_path = run_directory / "q00-session.json"
    try:
        _write_json_exclusive(session_path, document)
    except Exception:
        try:
            run_directory.rmdir()
        except OSError:
            pass
        raise
    return {**document, "session_path": str(session_path)}


def mark_moss_start(*, session_path: Path, marker_path: Path | None = None) -> dict[str, Any]:
    session = require_mapping(read_json(session_path, "formal gate session"), "formal gate session")
    _require_document_integrity(session, "formal gate session")
    run_directory = Path(require_string(session.get("run_directory"), "run_directory"))
    _strict_child(session_path, run_directory, "formal gate session")
    marker = marker_path or (run_directory / "q00-moss-start.json")
    _strict_child(marker, run_directory, "MOSS start marker")
    if (run_directory / "q00-execution-receipt.json").exists():
        raise QualityGateError("This formal Q00 session has already been finished")
    document = _with_integrity(
        {
            "schema_version": 1,
            "stage": "MOSS_FUNCTIONAL_FIX_Q00_MOSS_START_MARKER",
            "run_id": session["run_id"],
            "nonce": session["nonce"],
            "started_at": utc_now(),
            "started_monotonic_ns": time.monotonic_ns(),
            "created_by_gate": True,
        }
    )
    _write_json_exclusive(marker, document)
    return {**document, "marker_path": str(marker)}


def _validated_app_process_record(app_process: dict[str, Any]) -> dict[str, Any]:
    app_record = require_mapping(app_process, "formal app process")
    if (
        not isinstance(app_record.get("pid"), int)
        or app_record.get("pid", 0) <= 0
        or isinstance(app_record.get("bytes"), bool)
        or not isinstance(app_record.get("bytes"), int)
        or app_record.get("bytes", 0) <= 0
        or not Path(require_string(app_record.get("executable_path"), "formal app executable")).is_absolute()
        or str(app_record.get("sha256", "")).upper() != require_sha256(
            app_record.get("sha256"), "formal app executable SHA-256"
        )
    ):
        raise QualityGateError("Formal app process record is invalid")
    return dict(app_record)


def finish_formal_session(
    *,
    repo: Path,
    source_commit: str,
    session_path: Path,
    marker_path: Path,
    moss_completed_monotonic_ns: int,
    runner_exit_code: int,
    timed_out: bool,
    runner_arguments: Sequence[str],
    runner_cwd: Path,
    app_process: dict[str, Any],
    cleanup: dict[str, Any],
) -> dict[str, Any]:
    commit = _require_current_commit(repo, source_commit)
    session = require_mapping(read_json(session_path, "formal gate session"), "formal gate session")
    _require_document_integrity(session, "formal gate session")
    marker = require_mapping(read_json(marker_path, "MOSS start marker"), "MOSS start marker")
    _require_document_integrity(marker, "MOSS start marker")
    if (
        session.get("source_commit") != commit
        or marker.get("run_id") != session.get("run_id")
        or marker.get("nonce") != session.get("nonce")
        or marker.get("created_by_gate") is not True
    ):
        raise QualityGateError("MOSS start marker is not from this gate session")
    run_directory = Path(require_string(session.get("run_directory"), "run_directory"))
    _strict_child(session_path, run_directory, "formal gate session")
    _strict_child(marker_path, run_directory, "MOSS start marker")
    receipt_path = run_directory / "q00-execution-receipt.json"
    _require_new_outputs((receipt_path,))
    started_ns = _integer(marker.get("started_monotonic_ns"), "MOSS start marker monotonic time", minimum=1)
    completed_ns = _integer(
        moss_completed_monotonic_ns, "MOSS completed monotonic time", minimum=started_ns + 1
    )
    elapsed_seconds = (completed_ns - started_ns) / 1_000_000_000.0
    maximum = _finite_number(session.get("maximum_moss_seconds"), "maximum_moss_seconds", minimum=1.0)
    if not isinstance(timed_out, bool):
        raise QualityGateError("Formal runner timed_out must be a boolean")
    if timed_out != (elapsed_seconds > maximum):
        raise QualityGateError("Formal runner timeout flag disagrees with the gate monotonic interval")
    if isinstance(runner_exit_code, bool) or not isinstance(runner_exit_code, int):
        raise QualityGateError("Formal runner exit code must be an integer")
    exact_arguments = [require_string(item, "formal runner argument") for item in runner_arguments]
    if len(exact_arguments) != 5 or exact_arguments[1] != "moss-complete":
        raise QualityGateError("Formal MOSS runner arguments are not the fixed five-argument CDP action")
    run_directory = run_directory.resolve(strict=True)
    for item in exact_arguments[2:]:
        _strict_child(Path(item), run_directory, "formal MOSS runner input/output")
    cwd = runner_cwd.resolve(strict=True)
    if cwd != repo.resolve(strict=True):
        raise QualityGateError("Formal MOSS runner working directory is not the repository")
    cleanup_record = require_mapping(cleanup, "formal execution cleanup")
    residual = require_list(cleanup_record.get("residual_processes"), "formal residual processes")
    cleanup_ok = (
        cleanup_record.get("completed") is True
        and cleanup_record.get("consecutive_zero_scans") == 2
        and not residual
        and cleanup_record.get("cdp_listener_closed") is True
    )
    app_record = _validated_app_process_record(app_process)
    status_after = _repository_status_record(repo.resolve(strict=True))
    succeeded = not timed_out and runner_exit_code == 0 and cleanup_ok and status_after["entry_count"] == 0
    receipt = _with_integrity(
        {
            "schema_version": 1,
            "stage": FORMAL_RECEIPT_STAGE,
            "status": "COMPLETED" if succeeded else "FAILED",
            "created_by_gate": True,
            "run_once": True,
            "source_commit": commit,
            "run_id": session["run_id"],
            "nonce": session["nonce"],
            "started_at": marker["started_at"],
            "completed_at": utc_now(),
            "runner_exit_code": runner_exit_code,
            "timed_out": timed_out,
            "runner": {
                "program": session["node_runtime"],
                "script": session["cdp_runner"],
                "arguments": exact_arguments,
                "working_directory": str(cwd),
            },
            "app_process": app_record,
            "timing": {
                "clock": "time.monotonic_ns",
                "moss_started_monotonic_ns": started_ns,
                "moss_completed_monotonic_ns": completed_ns,
                "moss_elapsed_seconds": elapsed_seconds,
            },
            "start_marker": {"path": str(marker_path.resolve(strict=True)), **_safe_file_record(marker_path)},
            "cleanup": dict(cleanup_record),
            "repository_status_after": status_after,
        }
    )
    _write_json_exclusive(receipt_path, receipt)
    return {**receipt, "receipt_path": str(receipt_path)}


def prepare_window(
    *,
    source_wav: Path,
    output_wav: Path,
    manifest_path: Path,
    source_commit: str,
    tool_path: Path | None = None,
    generated_at: str | None = None,
    expected_source_sha256: str = SOURCE_SHA256,
    expected_source_frames: int = SOURCE_FRAMES,
    start_seconds: float = WINDOW_START_SECONDS,
    duration_seconds: float = WINDOW_DURATION_SECONDS,
) -> dict[str, Any]:
    """Extract an exact PCM frame range without consulting any model output.

    The overridable constants exist for small unit fixtures.  The command line
    does not expose them and therefore always enforces the formal frozen values.
    """

    commit = require_git_commit(source_commit, "source_commit")
    source = _lexical_absolute(source_wav)
    if not source.is_file():
        raise QualityGateError(f"Frozen source is missing: {source}")
    output = Path(os.path.abspath(output_wav))
    manifest = Path(os.path.abspath(manifest_path))
    if _same_path(source, output) or _same_path(source, manifest):
        raise QualityGateError("The frozen source cannot also be an output")
    _require_new_outputs((output, manifest))
    source_record = _safe_file_record(source)
    expected_hash = require_sha256(expected_source_sha256, "expected source SHA-256")
    if source_record["sha256"] != expected_hash:
        raise QualityGateError(
            f"Frozen source SHA-256 mismatch: {source_record['sha256']}"
        )

    try:
        with wave.open(str(source), "rb") as reader:
            format_tuple = (
                reader.getnchannels(),
                reader.getsampwidth(),
                reader.getframerate(),
                reader.getcomptype(),
            )
            if format_tuple != (CHANNELS, SAMPLE_WIDTH_BYTES, SAMPLE_RATE_HZ, "NONE"):
                raise QualityGateError(
                    "Frozen source must be mono 16 kHz 16-bit uncompressed PCM WAV"
                )
            if reader.getnframes() != expected_source_frames:
                raise QualityGateError(
                    f"Frozen source frame count mismatch: {reader.getnframes()}"
                )
            start_frame = round(start_seconds * SAMPLE_RATE_HZ)
            frame_count = round(duration_seconds * SAMPLE_RATE_HZ)
            if start_frame < 0 or frame_count <= 0 or start_frame + frame_count > reader.getnframes():
                raise QualityGateError("Requested deterministic window is outside the source WAV")
            reader.setpos(start_frame)
            frames = reader.readframes(frame_count)
    except (EOFError, wave.Error) as exc:
        raise QualityGateError(f"Frozen source is not a readable PCM WAV: {source}") from exc
    expected_pcm_bytes = frame_count * CHANNELS * SAMPLE_WIDTH_BYTES
    if len(frames) != expected_pcm_bytes:
        raise QualityGateError("Frozen source ended before the deterministic window was read")

    try:
        _write_pcm_wav_exclusive(output, frames)
        output_record = _safe_file_record(output)
        tool = (tool_path or Path(__file__)).resolve(strict=True)
        tool_record = _safe_file_record(tool)
        source_duration = expected_source_frames / SAMPLE_RATE_HZ
        window_duration = frame_count / SAMPLE_RATE_HZ
        document = {
            "schema_version": SCHEMA_VERSION,
            "stage": PREPARE_STAGE,
            "status": "PREPARED",
            "generated_at": generated_at or utc_now(),
            "source_commit": commit,
            "selection_rule": "FIXED_RANGE_WITHOUT_MODEL_OUTPUT",
            "source_audio": {
                **source_record,
                "path": str(source.resolve(strict=True)),
                "channels": CHANNELS,
                "sample_rate_hz": SAMPLE_RATE_HZ,
                "sample_width_bytes": SAMPLE_WIDTH_BYTES,
                "frame_count": expected_source_frames,
                "duration_seconds": source_duration,
            },
            "crop": {
                "source_start_seconds": start_frame / SAMPLE_RATE_HZ,
                "source_end_seconds": (start_frame + frame_count) / SAMPLE_RATE_HZ,
                "duration_seconds": window_duration,
                "start_frame": start_frame,
                "frame_count": frame_count,
            },
            "output_audio": {
                **output_record,
                "channels": CHANNELS,
                "sample_rate_hz": SAMPLE_RATE_HZ,
                "sample_width_bytes": SAMPLE_WIDTH_BYTES,
                "frame_count": frame_count,
                "duration_seconds": window_duration,
            },
            "producer": {
                "kind": "DETERMINISTIC_PCM_WINDOW_PREPARER",
                "executable_path": str(tool),
                **tool_record,
            },
        }
        _write_json_exclusive(manifest, document)
        return document
    except Exception:
        if output.exists():
            output.unlink()
        raise


def _normalize_cer_text(text: str) -> str:
    """Deterministic Chinese CER normalization.

    NFKC is applied, Latin letters are case-folded, and only Unicode letters
    and numbers are retained.  Every retained Unicode code point is one CER
    symbol.  There is no dictionary, word segmentation, or model-specific rule.
    """

    normalized = unicodedata.normalize("NFKC", text).casefold()
    return "".join(character for character in normalized if unicodedata.category(character)[0] in {"L", "N"})


def _levenshtein_distance(reference: Sequence[str], hypothesis: Sequence[str]) -> int:
    if len(reference) < len(hypothesis):
        # The algorithm is symmetric; keeping the second row shorter reduces memory.
        return _levenshtein_distance(hypothesis, reference)
    previous = list(range(len(hypothesis) + 1))
    for row, reference_symbol in enumerate(reference, 1):
        current = [row]
        for column, hypothesis_symbol in enumerate(hypothesis, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[column] + 1,
                    previous[column - 1] + (reference_symbol != hypothesis_symbol),
                )
            )
        previous = current
    return previous[-1]


def _cer(reference: str, hypothesis: str) -> dict[str, Any]:
    normalized_reference = _normalize_cer_text(reference)
    normalized_hypothesis = _normalize_cer_text(hypothesis)
    if not normalized_reference:
        raise QualityGateError("Human verbatim truth is empty after CER normalization")
    distance = _levenshtein_distance(normalized_reference, normalized_hypothesis)
    return {
        "distance": distance,
        "reference_characters": len(normalized_reference),
        "hypothesis_characters": len(normalized_hypothesis),
        "cer": distance / len(normalized_reference),
        "normalized_reference": normalized_reference,
        "normalized_hypothesis": normalized_hypothesis,
    }


def _tsv_rows(path: Path, expected_header: Sequence[str], label: str) -> list[dict[str, str]]:
    reject_symlink(path)
    try:
        with path.open("r", encoding="utf-8-sig", newline="") as stream:
            reader = csv.DictReader(stream, delimiter="\t")
            if tuple(reader.fieldnames or ()) != tuple(expected_header):
                raise QualityGateError(
                    f"{label} header must exactly match the documented schema"
                )
            rows = []
            for row in reader:
                if None in row or any(value is None for value in row.values()):
                    raise QualityGateError(f"{label} contains an incomplete row")
                rows.append(dict(row))
            return rows
    except UnicodeDecodeError as exc:
        raise QualityGateError(f"{label} must be UTF-8 TSV") from exc


def _tsv_bool(value: str, label: str) -> bool:
    folded = value.strip().casefold()
    if folded == "true":
        return True
    if folded == "false":
        return False
    raise QualityGateError(f"{label} must be true or false")


def _tsv_milliseconds(value: str, label: str) -> int:
    if not re.fullmatch(r"0|[1-9][0-9]*", value.strip()):
        raise QualityGateError(f"{label} must be a non-negative integer number of milliseconds")
    return int(value)


def _load_human_verbatim(path: Path) -> dict[str, Any]:
    rows = _tsv_rows(path, HUMAN_VERBATIM_HEADER, "human verbatim truth")
    scoring: list[dict[str, Any]] = []
    previous_start = -1
    previous_end = -1
    seen_ids: set[str] = set()
    for index, row in enumerate(rows, 1):
        start_ms = _tsv_milliseconds(row["start_ms"], f"human row {index} start_ms")
        end_ms = _tsv_milliseconds(row["end_ms"], f"human row {index} end_ms")
        if start_ms >= end_ms or end_ms > round(WINDOW_DURATION_SECONDS * 1000):
            raise QualityGateError(f"human row {index} has an invalid time range")
        if start_ms < previous_start:
            raise QualityGateError("human verbatim rows are not ordered by start_ms")
        segment_id = require_string(row["segment_id"], f"human row {index} segment_id")
        if segment_id in seen_ids:
            raise QualityGateError("human verbatim segment_id values must be unique")
        seen_ids.add(segment_id)
        overlap = _tsv_bool(row["overlap"], f"human row {index} overlap")
        if start_ms < previous_end and not overlap:
            raise QualityGateError(
                "human verbatim rows overlap without overlap=true on the later row"
            )
        previous_start = start_ms
        previous_end = max(previous_end, end_ms)
        valid = _tsv_bool(row["valid_for_scoring"], f"human row {index} valid_for_scoring")
        non_speech = _tsv_bool(row["non_speech"], f"human row {index} non_speech")
        text = row["verbatim_text"]
        speaker = row["reference_speaker_id"].strip()
        if valid and not non_speech:
            if not text.strip() or not speaker:
                raise QualityGateError(
                    f"human row {index} is scoring speech but lacks text or reference speaker"
                )
            scoring.append(
                {
                    "segment_id": segment_id,
                    "start": start_ms / 1000.0,
                    "end": end_ms / 1000.0,
                    "speaker": speaker,
                    "text": text,
                    "overlap": overlap,
                }
            )
    if not scoring:
        raise QualityGateError("human verbatim truth has no valid scoring speech")
    return {"rows": scoring, "text": "".join(row["text"] for row in scoring)}


def _load_speaker_truth(path: Path) -> list[dict[str, Any]]:
    rows = _tsv_rows(path, SPEAKER_TRUTH_HEADER, "speaker truth")
    truth: list[dict[str, Any]] = []
    previous_start = -1
    previous_end = -1
    seen_ids: set[str] = set()
    for index, row in enumerate(rows, 1):
        start_ms = _tsv_milliseconds(row["start_ms"], f"speaker row {index} start_ms")
        end_ms = _tsv_milliseconds(row["end_ms"], f"speaker row {index} end_ms")
        if start_ms >= end_ms or end_ms > round(WINDOW_DURATION_SECONDS * 1000):
            raise QualityGateError(f"speaker row {index} has an invalid time range")
        if start_ms < previous_start:
            raise QualityGateError("speaker truth rows are not ordered by start_ms")
        turn_id = require_string(row["turn_id"], f"speaker row {index} turn_id")
        if turn_id in seen_ids:
            raise QualityGateError("speaker truth turn_id values must be unique")
        seen_ids.add(turn_id)
        overlap = _tsv_bool(row["overlap"], f"speaker row {index} overlap")
        if start_ms < previous_end and not overlap:
            raise QualityGateError(
                "speaker truth rows overlap without overlap=true on the later row"
            )
        previous_start = start_ms
        previous_end = max(previous_end, end_ms)
        valid = _tsv_bool(row["valid_for_scoring"], f"speaker row {index} valid_for_scoring")
        speaker = row["reference_speaker_id"].strip()
        if valid:
            if not speaker:
                raise QualityGateError(f"speaker row {index} lacks a reference speaker")
            truth.append(
                {
                    "turn_id": turn_id,
                    "start": start_ms / 1000.0,
                    "end": end_ms / 1000.0,
                    "speaker": speaker,
                    "overlap": overlap,
                }
            )
    if not truth:
        raise QualityGateError("speaker truth has no valid scoring turn")
    return truth


def _load_current_json(
    path: Path,
    *,
    role: str,
    run_id: str,
    source_commit: str,
    window_sha256: str,
    producer_sha256: str,
    run_started: datetime,
    run_completed: datetime,
) -> dict[str, Any]:
    document = require_mapping(read_json(path, role), role)
    if document.get("schema_version") != SCHEMA_VERSION:
        raise QualityGateError(f"{role} schema_version must be {SCHEMA_VERSION}")
    provenance = require_mapping(document.get("provenance"), f"{role} provenance")
    if provenance.get("artifact_role") != role:
        raise QualityGateError(f"{role} provenance artifact_role mismatch")
    if provenance.get("run_id") != run_id:
        raise QualityGateError(f"{role} was not produced by the current Q00 run")
    if provenance.get("source_commit") != source_commit:
        raise QualityGateError(f"{role} was not produced from the current source commit")
    if str(provenance.get("window_audio_sha256", "")).upper() != window_sha256:
        raise QualityGateError(f"{role} is bound to a different window audio")
    if str(provenance.get("producer_executable_sha256", "")).upper() != producer_sha256:
        raise QualityGateError(f"{role} producer hash does not match the live bound producer")
    produced_at = _parse_iso8601(provenance.get("produced_at"), f"{role} produced_at")
    if produced_at < run_started or produced_at > run_completed:
        raise QualityGateError(f"{role} produced_at is outside the current run interval")
    return document


def _expected_transcription_contract(engine_key: str) -> dict[str, Any]:
    if engine_key not in {"moss", "whisper"}:
        raise QualityGateError(f"Unknown Q00 transcription engine contract: {engine_key}")
    result = {
        "language_resolution_source": TRANSCRIPTION_LANGUAGE_SOURCES[engine_key],
        "model_sha256": str(MODEL_CONTRACTS[engine_key]["sha256"]),
    }
    if engine_key == "whisper":
        parameters = TRANSCRIPTION_DECODE_PARAMETERS[engine_key]
        result.update(
            {
                "decode_parameters": json.loads(json.dumps(parameters)),
                "decode_parameters_sha256": sha256_bytes(canonical_json_bytes(parameters)),
            }
        )
    return result


def _parse_decode_parameters_json(value: Any, label: str) -> tuple[str, dict[str, Any], str]:
    raw = require_string(value, label)

    def reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in pairs:
            if key in result:
                raise ValueError(f"duplicate key: {key}")
            result[key] = item
        return result

    def reject_nonfinite(value: str) -> None:
        raise ValueError(f"non-finite JSON number: {value}")

    try:
        parameters = json.loads(
            raw,
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_nonfinite,
        )
    except (json.JSONDecodeError, ValueError) as exc:
        raise QualityGateError(f"{label} must be a valid JSON object") from exc
    if not isinstance(parameters, dict):
        raise QualityGateError(f"{label} must be a valid JSON object")
    return raw, parameters, sha256_bytes(raw.encode("utf-8"))


def _parse_inference_contract(document: dict[str, Any], role: str) -> dict[str, Any]:
    engine_key = "moss" if role == "moss_raw" else "whisper"
    expected = _expected_transcription_contract(engine_key)
    contract = require_mapping(document.get("inference_contract"), f"{role} inference_contract")
    expected_fields = {
        "window_audio_sha256",
        "window_duration_seconds",
        "language_requested",
        "language_resolved",
        "language_resolution_source",
        "model_sha256",
        "decode_parameters_sha256",
    }
    expected_fields.add(
        "decode_parameters_json" if engine_key == "moss" else "decode_parameters"
    )
    if set(contract) != expected_fields:
        raise QualityGateError(f"{role} inference_contract fields are not exact")
    parameter_sha256 = require_sha256(
        contract.get("decode_parameters_sha256"), f"{role} decode_parameters_sha256"
    )
    decode_parameters_json = None
    if engine_key == "moss":
        (
            decode_parameters_json,
            parameters,
            calculated_parameters_sha256,
        ) = _parse_decode_parameters_json(
            contract.get("decode_parameters_json"), f"{role} decode_parameters_json"
        )
    else:
        parameters = require_mapping(
            contract.get("decode_parameters"), f"{role} decode_parameters"
        )
        calculated_parameters_sha256 = sha256_bytes(canonical_json_bytes(parameters))
    if parameter_sha256 != calculated_parameters_sha256:
        raise QualityGateError(
            f"{role} decode_parameters_sha256 does not match the actual JSON bytes"
            if engine_key == "moss"
            else f"{role} decode_parameters_sha256 does not match the recorded parameters"
        )
    language_requested = require_string(
        contract.get("language_requested"), f"{role} language_requested"
    )
    language_resolved = require_string(
        contract.get("language_resolved"), f"{role} language_resolved"
    )
    language_source = require_string(
        contract.get("language_resolution_source"),
        f"{role} language_resolution_source",
    )
    if engine_key == "moss":
        if parameters != MOSS_NATIVE_DECODE_PARAMETERS_CONTRACT:
            raise QualityGateError(
                f"{role} decode_parameters_json is not the exact frozen native contract"
            )
        if language_requested != FAIR_COMPARISON_LANGUAGE:
            raise QualityGateError(
                f"{role} language_requested must be explicit {FAIR_COMPARISON_LANGUAGE}"
            )
        if language_resolved.casefold() == "auto":
            raise QualityGateError(f"{role} language_resolved cannot be auto")
        if language_requested != language_resolved:
            raise QualityGateError(f"{role} requested/resolved language mismatch")
        if language_source != TRANSCRIPTION_LANGUAGE_SOURCES["moss"]:
            raise QualityGateError(
                f"{role} language/decode evidence must come from api_moss_get_workspace.runs"
            )
    return {
        "window_audio_sha256": require_sha256(
            contract.get("window_audio_sha256"), f"{role} comparison window SHA-256"
        ),
        "window_duration_seconds": _finite_number(
            contract.get("window_duration_seconds"),
            f"{role} comparison window duration",
            minimum=0.0,
        ),
        "language_requested": language_requested,
        "language_resolved": language_resolved,
        "language_resolution_source": language_source,
        "model_sha256": require_sha256(
            contract.get("model_sha256"), f"{role} model_sha256"
        ),
        "decode_parameters_json": decode_parameters_json,
        "decode_parameters": parameters,
        "decode_parameters_sha256": parameter_sha256,
        "calculated_decode_parameters_sha256": calculated_parameters_sha256,
        "expected_model_sha256": expected["model_sha256"],
        "expected_decode_parameters": expected.get("decode_parameters"),
        "expected_decode_parameters_sha256": expected.get("decode_parameters_sha256"),
        "expected_language_resolution_source": expected["language_resolution_source"],
    }


def _parse_manual_speaker_override(
    document: dict[str, Any], parsed_segments: list[dict[str, Any]]
) -> dict[str, Any]:
    rows = require_list(
        document.get("manual_speaker_overrides"),
        "corrected manual_speaker_overrides",
    )
    parsed_rows: list[dict[str, Any]] = []
    true_flip = len(rows) == 1
    for position, raw in enumerate(rows):
        row = require_mapping(raw, f"corrected manual speaker override {position}")
        if set(row) != {
            "wrong_override_id",
            "override_id",
            "segment_id",
            "segment_index",
            "source",
            "before_speaker_id",
            "after_speaker_id",
            "reference_speaker_id",
        }:
            raise QualityGateError("corrected manual speaker override fields are not exact")
        wrong_override_id = require_string(
            row.get("wrong_override_id"), "manual wrong_override_id"
        )
        override_id = require_string(row.get("override_id"), "manual override_id")
        segment_id = require_string(row.get("segment_id"), "manual override segment_id")
        segment_index = _integer(
            row.get("segment_index"), "manual override segment_index", minimum=0
        )
        before = require_string(
            row.get("before_speaker_id"), "manual override before_speaker_id"
        )
        after = require_string(
            row.get("after_speaker_id"), "manual override after_speaker_id"
        )
        reference = require_string(
            row.get("reference_speaker_id"), "manual override reference_speaker_id"
        )
        source = require_string(row.get("source"), "manual override source")
        final_speaker = (
            parsed_segments[segment_index]["speaker"]
            if segment_index < len(parsed_segments)
            else None
        )
        row_is_true_flip = (
            source == "HUMAN_SINGLE_SEGMENT_OVERRIDE"
            and wrong_override_id != override_id
            and before != after
            and after == reference
            and final_speaker == after
        )
        true_flip = true_flip and row_is_true_flip
        parsed_rows.append(
            {
                "wrong_override_id": wrong_override_id,
                "override_id": override_id,
                "segment_id": segment_id,
                "segment_index": segment_index,
                "source": source,
                "before_speaker_id": before,
                "after_speaker_id": after,
                "reference_speaker_id": reference,
                "final_speaker_id": final_speaker,
                "true_flip": row_is_true_flip,
            }
        )
    return {"count": len(rows), "true_flip": true_flip, "rows": parsed_rows}


def _parse_transcript(document: dict[str, Any], role: str) -> dict[str, Any]:
    if document.get("engine") != TRANSCRIPT_ROLES[role]:
        raise QualityGateError(f"{role} engine must be {TRANSCRIPT_ROLES[role]}")
    raw_segments = require_list(document.get("segments"), f"{role} segments")
    invalid_reasons: list[str] = []
    parsed: list[dict[str, Any]] = []
    text_parts: list[str] = []
    previous_start = -math.inf
    previous_end = -math.inf
    for index, item in enumerate(raw_segments):
        if not isinstance(item, dict):
            invalid_reasons.append(f"segment_{index}_not_object")
            continue
        text = item.get("text")
        if not isinstance(text, str) or not text.strip():
            invalid_reasons.append(f"segment_{index}_missing_text")
            text = "" if not isinstance(text, str) else text
        text_parts.append(text)
        start_value = item.get("start_seconds")
        end_value = item.get("end_seconds")
        valid_numbers = (
            not isinstance(start_value, bool)
            and isinstance(start_value, (int, float))
            and math.isfinite(float(start_value))
            and not isinstance(end_value, bool)
            and isinstance(end_value, (int, float))
            and math.isfinite(float(end_value))
        )
        if not valid_numbers:
            invalid_reasons.append(f"segment_{index}_invalid_number")
            continue
        start = float(start_value)
        end = float(end_value)
        if start < 0.0 or end <= start or end > WINDOW_DURATION_SECONDS + 1e-9:
            invalid_reasons.append(f"segment_{index}_out_of_bounds")
        if start < previous_start - 1e-9 or end < previous_end - 1e-9:
            invalid_reasons.append(f"segment_{index}_non_monotonic")
        previous_start = start
        previous_end = end
        speaker = item.get("speaker_id")
        parsed.append(
            {
                "index": index,
                "start": start,
                "end": end,
                "speaker": speaker.strip() if isinstance(speaker, str) else "",
                "text": text,
                "candidate_segment_id": (
                    item.get("candidate_segment_id")
                    if isinstance(item.get("candidate_segment_id"), str)
                    else None
                ),
            }
        )
    if not raw_segments:
        invalid_reasons.append("segments_empty")
    result = {
        "segments": parsed,
        "text": "".join(text_parts),
        "timestamp_valid": not invalid_reasons,
        "invalid_reasons": sorted(set(invalid_reasons)),
        "segment_count": len(raw_segments),
    }
    if role == "moss_raw":
        result["inference_seconds"] = _finite_number(
            document.get("inference_seconds"), "moss_raw inference_seconds", minimum=0.0
        )
        result["inference_contract"] = _parse_inference_contract(document, role)
    if role == "whisper_same_window":
        result["inference_contract"] = _parse_inference_contract(document, role)
    if role == "corrected":
        if "manual_speaker_overrides" not in document:
            raise QualityGateError("corrected must record manual_speaker_overrides")
        override = _parse_manual_speaker_override(document, parsed)
        result["manual_speaker_override_count"] = override["count"]
        result["speaker_override_true_flip"] = override["true_flip"]
        result["manual_speaker_override_evidence"] = override["rows"]
    return result


def _overlap_seconds(left: dict[str, Any], right: dict[str, Any]) -> float:
    return max(0.0, min(left["end"], right["end"]) - max(left["start"], right["start"]))


def _speaker_mapping(
    predicted: list[dict[str, Any]], truth: list[dict[str, Any]]
) -> tuple[dict[str, str], bool]:
    predicted_ids = sorted({segment["speaker"] for segment in predicted if segment["speaker"]})
    truth_ids = sorted({turn["speaker"] for turn in truth})
    if not predicted_ids or not truth_ids or len(truth_ids) > 12 or len(predicted_ids) > 12:
        return {}, False
    weights = {
        (predicted_id, truth_id): sum(
            _overlap_seconds(segment, turn)
            for segment in predicted
            if segment["speaker"] == predicted_id
            for turn in truth
            if turn["speaker"] == truth_id
        )
        for predicted_id in predicted_ids
        for truth_id in truth_ids
    }
    memo: dict[tuple[int, int], tuple[float, tuple[int | None, ...]]] = {}

    def solve(index: int, used: int) -> tuple[float, tuple[int | None, ...]]:
        key = (index, used)
        if key in memo:
            return memo[key]
        if index == len(predicted_ids):
            return 0.0, ()
        best_score, best_choices = solve(index + 1, used)
        best: tuple[float, tuple[int | None, ...]] = (best_score, (None,) + best_choices)
        for truth_index, truth_id in enumerate(truth_ids):
            bit = 1 << truth_index
            if used & bit:
                continue
            tail_score, tail_choices = solve(index + 1, used | bit)
            candidate = (
                weights[(predicted_ids[index], truth_id)] + tail_score,
                (truth_index,) + tail_choices,
            )
            if candidate[0] > best[0] + 1e-12:
                best = candidate
        memo[key] = best
        return best

    _, choices = solve(0, 0)
    mapping = {
        predicted_ids[index]: truth_ids[choice]
        for index, choice in enumerate(choices)
        if choice is not None
    }
    return mapping, len(mapping) == len(predicted_ids)


def _raw_speaker_metrics(
    predicted: list[dict[str, Any]], truth: list[dict[str, Any]]
) -> dict[str, Any]:
    mapping, mapping_complete = _speaker_mapping(predicted, truth)
    segment_errors = 0
    scored_segments = 0
    for segment in predicted:
        if not segment["speaker"]:
            segment_errors += 1
            scored_segments += 1
            continue
        overlaps = [(_overlap_seconds(segment, turn), turn) for turn in truth]
        best_overlap, best_turn = max(overlaps, key=lambda pair: pair[0])
        scored_segments += 1
        if best_overlap <= 0.0 or mapping.get(segment["speaker"]) != best_turn["speaker"]:
            segment_errors += 1

    boundaries = sorted(
        {point for item in predicted + truth for point in (item["start"], item["end"])}
    )
    scored_duration = 0.0
    duration_error = 0.0
    false_alarm = 0.0
    for start, end in zip(boundaries, boundaries[1:]):
        if end <= start:
            continue
        midpoint = (start + end) / 2.0
        active_truth = [turn for turn in truth if turn["start"] <= midpoint < turn["end"]]
        active_predicted = [
            segment for segment in predicted if segment["start"] <= midpoint < segment["end"]
        ]
        duration = end - start
        if len(active_truth) == 1:
            scored_duration += duration
            if (
                len(active_predicted) != 1
                or mapping.get(active_predicted[0]["speaker"]) != active_truth[0]["speaker"]
            ):
                duration_error += duration
        elif not active_truth and active_predicted:
            false_alarm += duration
    measurement_complete = (
        bool(predicted)
        and all(segment["speaker"] for segment in predicted)
        and mapping_complete
        and scored_segments > 0
        and scored_duration > 0.0
    )
    return {
        "measurement_complete": measurement_complete,
        "segment_error_count": segment_errors,
        "scored_segment_count": scored_segments,
        "segment_error_rate": segment_errors / scored_segments if scored_segments else 1.0,
        "duration_error_seconds": duration_error,
        "scored_duration_seconds": scored_duration,
        "duration_error_rate": duration_error / scored_duration if scored_duration else 1.0,
        "false_alarm_seconds": false_alarm,
        "anonymous_label_mapping": mapping,
    }


def _corrected_speaker_errors(
    corrected: list[dict[str, Any]], truth: list[dict[str, Any]]
) -> dict[str, Any]:
    error_count = 0
    correct_truth_turns: set[str] = set()
    for segment in corrected:
        overlaps = [(_overlap_seconds(segment, turn), turn) for turn in truth]
        best_overlap, best_turn = max(overlaps, key=lambda pair: pair[0])
        if (
            best_overlap <= 0.0
            or not segment["speaker"]
            or segment["speaker"] != best_turn["speaker"]
        ):
            error_count += 1
        else:
            correct_truth_turns.add(best_turn["turn_id"])
    uncovered_turns = [turn["turn_id"] for turn in truth if turn["turn_id"] not in correct_truth_turns]
    error_count += len(uncovered_turns)
    return {
        "error_count": error_count,
        "scored_segment_count": len(corrected),
        "uncovered_truth_turn_count": len(uncovered_turns),
        "uncovered_truth_turn_ids": uncovered_turns,
    }


def _speaker_override_truth_bound(
    override_rows: list[dict[str, Any]],
    corrected: list[dict[str, Any]],
    truth: list[dict[str, Any]],
) -> bool:
    if len(override_rows) != 1:
        return False
    row = override_rows[0]
    index = row["segment_index"]
    if index >= len(corrected) or not row["true_flip"]:
        return False
    segment = corrected[index]
    overlaps = [(_overlap_seconds(segment, turn), turn) for turn in truth]
    best_overlap, best_turn = max(overlaps, key=lambda pair: pair[0])
    candidate_segment_id = segment.get("candidate_segment_id")
    return (
        best_overlap > 0.0
        and row["reference_speaker_id"] == best_turn["speaker"]
        and row["after_speaker_id"] == best_turn["speaker"]
        and (
            candidate_segment_id is None
            or candidate_segment_id == row["segment_id"]
        )
    )


def _count_occurrences(text: str, term: str) -> int:
    normalized_term = _normalize_cer_text(term)
    if not normalized_term:
        raise QualityGateError("A term is empty after deterministic normalization")
    return text.count(normalized_term)


def _load_term_truth(
    path: Path,
    *,
    kind: str,
    window_sha256: str,
    human_sha256: str,
    speaker_sha256: str,
) -> dict[str, Any]:
    document = require_mapping(read_json(path, f"{kind} term truth"), f"{kind} term truth")
    expected_stage = f"MOSS_FUNCTIONAL_FIX_Q00_{kind.upper()}_TRUTH"
    if document.get("stage") != expected_stage or document.get("status") != "APPROVED":
        raise QualityGateError(f"{kind} term truth is not an approved Q00 truth document")
    if document.get("schema_version") != SCHEMA_VERSION:
        raise QualityGateError(f"{kind} term truth schema_version must be {SCHEMA_VERSION}")
    if str(document.get("window_audio_sha256", "")).upper() != window_sha256:
        raise QualityGateError(f"{kind} term truth is bound to another audio window")
    if str(document.get("human_verbatim_sha256", "")).upper() != human_sha256:
        raise QualityGateError(f"{kind} term truth is bound to another human transcript")
    if str(document.get("speaker_truth_sha256", "")).upper() != speaker_sha256:
        raise QualityGateError(f"{kind} term truth is bound to another speaker truth")
    require_string(document.get("reviewer_id"), f"{kind} truth reviewer_id")
    _parse_iso8601(document.get("approved_at"), f"{kind} truth approved_at")
    expected_attestation = POSITIVE_ATTESTATION if kind == "positive" else NEGATIVE_ATTESTATION
    if document.get("attestation") != expected_attestation:
        raise QualityGateError(f"{kind} term truth lacks the required human attestation")
    rows = require_list(document.get("terms"), f"{kind} truth terms")
    if not rows:
        raise QualityGateError(f"{kind} term truth must contain at least one term")
    normalized_seen: set[str] = set()
    parsed: list[dict[str, Any]] = []
    for index, item in enumerate(rows, 1):
        row = require_mapping(item, f"{kind} term {index}")
        term = require_string(row.get("term"), f"{kind} term {index} term")
        normalized = _normalize_cer_text(term)
        if not normalized or normalized in normalized_seen:
            raise QualityGateError(f"{kind} term set contains an empty or duplicate term")
        normalized_seen.add(normalized)
        expected = _integer(
            row.get("expected_occurrences"),
            f"{kind} term {index} expected_occurrences",
            minimum=0,
        )
        source = require_sha256(
            row.get("source_evidence_sha256"),
            f"{kind} term {index} source_evidence_sha256",
        )
        if source != human_sha256:
            raise QualityGateError(
                f"{kind} term {index} source evidence must be the bound human verbatim truth"
            )
        if kind == "positive" and expected <= 0:
            raise QualityGateError("Positive term expected_occurrences must be positive")
        if kind == "negative" and expected != 0:
            raise QualityGateError("Negative term expected_occurrences must be zero")
        parsed.append(
            {
                "term": term,
                "normalized": normalized,
                "expected": expected,
                "source_evidence_sha256": source,
            }
        )
    return {"document": document, "terms": parsed}


def _validate_window_manifest(
    manifest_path: Path,
    window_path: Path,
    source_commit: str,
    *,
    formal: bool = True,
) -> tuple[dict[str, Any], dict[str, Any]]:
    manifest = require_mapping(read_json(manifest_path, "window manifest"), "window manifest")
    if (
        manifest.get("schema_version") != SCHEMA_VERSION
        or manifest.get("stage") != PREPARE_STAGE
        or manifest.get("status") != "PREPARED"
        or manifest.get("selection_rule") != "FIXED_RANGE_WITHOUT_MODEL_OUTPUT"
        or manifest.get("source_commit") != source_commit
    ):
        raise QualityGateError("Window manifest is not a current Q00 prepare result")
    source = require_mapping(manifest.get("source_audio"), "window source audio")
    crop = require_mapping(manifest.get("crop"), "window crop")
    output = require_mapping(manifest.get("output_audio"), "window output audio")
    exact_source = (
        str(source.get("sha256", "")).upper() == SOURCE_SHA256
        and source.get("bytes") == SOURCE_BYTES
        and source.get("frame_count") == SOURCE_FRAMES
        and source.get("channels") == CHANNELS
        and source.get("sample_rate_hz") == SAMPLE_RATE_HZ
        and source.get("sample_width_bytes") == SAMPLE_WIDTH_BYTES
        and abs(_finite_number(source.get("duration_seconds"), "source duration") - SOURCE_DURATION_SECONDS) < 1e-9
    )
    exact_crop = (
        crop.get("start_frame") == WINDOW_START_FRAME
        and crop.get("frame_count") == WINDOW_FRAME_COUNT
        and abs(_finite_number(crop.get("source_start_seconds"), "crop start") - WINDOW_START_SECONDS) < 1e-9
        and abs(_finite_number(crop.get("source_end_seconds"), "crop end") - WINDOW_END_SECONDS) < 1e-9
        and abs(_finite_number(crop.get("duration_seconds"), "crop duration") - WINDOW_DURATION_SECONDS) < 1e-9
    )
    if not exact_source or not exact_crop:
        raise QualityGateError("Window manifest does not describe the frozen Q00 source and range")
    window_record = _safe_file_record(window_path)
    if formal and window_record != {"bytes": WINDOW_BYTES, "sha256": WINDOW_SHA256}:
        raise QualityGateError("Prepared window is not the unique frozen Q00 window bytes")
    if (
        output.get("bytes") != window_record["bytes"]
        or str(output.get("sha256", "")).upper() != window_record["sha256"]
        or output.get("frame_count") != WINDOW_FRAME_COUNT
        or output.get("channels") != CHANNELS
        or output.get("sample_rate_hz") != SAMPLE_RATE_HZ
        or output.get("sample_width_bytes") != SAMPLE_WIDTH_BYTES
        or abs(_finite_number(output.get("duration_seconds"), "window duration") - WINDOW_DURATION_SECONDS) > 1e-9
    ):
        raise QualityGateError("Window manifest output hash or PCM facts do not match the audio")
    try:
        with wave.open(str(window_path), "rb") as reader:
            actual = (
                reader.getnchannels(),
                reader.getsampwidth(),
                reader.getframerate(),
                reader.getcomptype(),
                reader.getnframes(),
            )
    except (EOFError, wave.Error) as exc:
        raise QualityGateError("Prepared window is not a readable PCM WAV") from exc
    if actual != (CHANNELS, SAMPLE_WIDTH_BYTES, SAMPLE_RATE_HZ, "NONE", WINDOW_FRAME_COUNT):
        raise QualityGateError("Prepared window PCM format or frame count is wrong")
    if formal:
        source_path = Path(require_string(source.get("path"), "window source audio path"))
        if not source_path.is_absolute():
            raise QualityGateError("Window source audio path must be absolute")
        source_record = _safe_file_record(source_path)
        if source_record != {"bytes": SOURCE_BYTES, "sha256": SOURCE_SHA256}:
            raise QualityGateError("Live frozen source bytes do not match the formal Q00 source")
        try:
            with wave.open(str(source_path), "rb") as reader:
                if (
                    reader.getnchannels(),
                    reader.getsampwidth(),
                    reader.getframerate(),
                    reader.getcomptype(),
                    reader.getnframes(),
                ) != (CHANNELS, SAMPLE_WIDTH_BYTES, SAMPLE_RATE_HZ, "NONE", SOURCE_FRAMES):
                    raise QualityGateError("Live frozen source PCM facts are wrong")
                reader.setpos(WINDOW_START_FRAME)
                recropped_frames = reader.readframes(WINDOW_FRAME_COUNT)
        except (EOFError, wave.Error) as exc:
            raise QualityGateError("Live frozen source cannot be independently re-cropped") from exc
        expected_header = struct.pack(
            "<4sI4s4sIHHIIHH4sI",
            b"RIFF",
            36 + len(recropped_frames),
            b"WAVE",
            b"fmt ",
            16,
            1,
            CHANNELS,
            SAMPLE_RATE_HZ,
            SAMPLE_RATE_HZ * CHANNELS * SAMPLE_WIDTH_BYTES,
            CHANNELS * SAMPLE_WIDTH_BYTES,
            SAMPLE_WIDTH_BYTES * 8,
            b"data",
            len(recropped_frames),
        )
        recropped = expected_header + recropped_frames
        if len(recropped) != WINDOW_BYTES or sha256_bytes(recropped) != WINDOW_SHA256:
            raise QualityGateError("Independent source re-crop does not equal the frozen window")
        if window_path.read_bytes() != recropped:
            raise QualityGateError("Prepared window differs byte-for-byte from the independent re-crop")
    producer = require_mapping(manifest.get("producer"), "window manifest producer")
    if producer.get("kind") != "DETERMINISTIC_PCM_WINDOW_PREPARER":
        raise QualityGateError("Window manifest producer kind is wrong")
    producer_path = Path(
        require_string(producer.get("executable_path"), "window manifest producer path")
    )
    if not producer_path.is_absolute():
        raise QualityGateError("Window manifest producer path must be absolute")
    actual_producer = _safe_file_record(producer_path)
    if (
        producer.get("bytes") != actual_producer["bytes"]
        or str(producer.get("sha256", "")).upper() != actual_producer["sha256"]
    ):
        raise QualityGateError("Window manifest producer executable hash is stale")
    return manifest, window_record


def _validate_bindings(
    bindings_path: Path,
    artifact_paths: dict[str, Path],
    source_commit: str,
) -> dict[str, Any]:
    document = require_mapping(read_json(bindings_path, "Q00 run bindings"), "Q00 run bindings")
    if (
        document.get("schema_version") != SCHEMA_VERSION
        or document.get("stage") != BINDINGS_STAGE
        or document.get("formal_short_gate") is not True
        or document.get("current_run") is not True
    ):
        raise QualityGateError("Run bindings are not a formal current Q00 run")
    if document.get("source_commit") != source_commit:
        raise QualityGateError("Run bindings source_commit is stale")
    run_id = require_string(document.get("run_id"), "run_id")
    if not RUN_ID_RE.fullmatch(run_id) or not run_id.startswith(f"Q00-{source_commit[:12]}-"):
        raise QualityGateError("run_id is not bound to the current source commit")
    started = _parse_iso8601(document.get("started_at"), "run started_at")
    completed = _parse_iso8601(document.get("completed_at"), "run completed_at")
    if completed < started:
        raise QualityGateError("Run completed_at is earlier than started_at")
    artifacts = require_mapping(document.get("artifacts"), "run binding artifacts")
    if set(artifacts) != set(ARTIFACT_ORIGINS):
        missing = sorted(set(ARTIFACT_ORIGINS) - set(artifacts))
        extra = sorted(set(artifacts) - set(ARTIFACT_ORIGINS))
        raise QualityGateError(f"Run binding artifact roles mismatch; missing={missing}, extra={extra}")
    if set(artifact_paths) != set(ARTIFACT_ORIGINS):
        raise QualityGateError("Internal artifact path roles do not match the Q00 schema")
    resolved_inputs = [path.resolve(strict=True) for path in artifact_paths.values()]
    if len({os.path.normcase(str(path)) for path in resolved_inputs}) != len(resolved_inputs):
        raise QualityGateError("Every Q00 artifact role must use a distinct file")
    validated: dict[str, Any] = {}
    for role in sorted(ARTIFACT_ORIGINS):
        binding = require_mapping(artifacts[role], f"{role} binding")
        if binding.get("origin") != ARTIFACT_ORIGINS[role]:
            raise QualityGateError(f"{role} origin is wrong")
        if binding.get("source_commit") != source_commit or binding.get("run_id") != run_id:
            raise QualityGateError(f"{role} binding is not tied to the current run and commit")
        actual = _safe_file_record(artifact_paths[role])
        if (
            binding.get("bytes") != actual["bytes"]
            or str(binding.get("sha256", "")).upper() != actual["sha256"]
        ):
            raise QualityGateError(f"{role} bytes or SHA-256 do not match the real artifact")
        producer = require_mapping(binding.get("producer"), f"{role} producer")
        executable_path = Path(
            require_string(producer.get("executable_path"), f"{role} producer executable_path")
        )
        if not executable_path.is_absolute():
            raise QualityGateError(f"{role} producer executable_path must be absolute")
        executable = _safe_file_record(executable_path)
        if (
            producer.get("bytes") != executable["bytes"]
            or str(producer.get("sha256", "")).upper() != executable["sha256"]
        ):
            raise QualityGateError(f"{role} producer executable hash is stale or fabricated")
        exporter_record = None
        exporter_path_value = None
        if "exporter" in binding:
            exporter = require_mapping(binding.get("exporter"), f"{role} exporter")
            exporter_path = Path(
                require_string(exporter.get("executable_path"), f"{role} exporter executable_path")
            )
            if not exporter_path.is_absolute():
                raise QualityGateError(f"{role} exporter executable_path must be absolute")
            exporter_record = _safe_file_record(exporter_path)
            if (
                exporter.get("bytes") != exporter_record["bytes"]
                or str(exporter.get("sha256", "")).upper() != exporter_record["sha256"]
            ):
                raise QualityGateError(f"{role} exporter executable hash is stale or fabricated")
            exporter_path_value = str(exporter_path.resolve(strict=True))
        validated[role] = {
            "origin": ARTIFACT_ORIGINS[role],
            "source_commit": source_commit,
            "run_id": run_id,
            **actual,
            "producer": executable,
            "producer_path": str(executable_path.resolve(strict=True)),
            "exporter": exporter_record,
            "exporter_path": exporter_path_value,
        }
    return {
        "document": document,
        "path": str(bindings_path.resolve(strict=True)),
        "run_id": run_id,
        "started": started,
        "completed": completed,
        "artifacts": validated,
        "record": _safe_file_record(bindings_path),
    }


def _norm_path(path: Path) -> str:
    return os.path.normcase(str(path.resolve(strict=True)))


def _strict_child(path: Path, root: Path, label: str, *, allow_equal: bool = False) -> Path:
    lexical_path = _lexical_absolute(path)
    lexical_root = _lexical_absolute(root)
    if not lexical_root.is_dir():
        raise QualityGateError(f"{label} root is not a directory: {lexical_root}")
    try:
        relative = lexical_path.relative_to(lexical_root)
    except ValueError as exc:
        raise QualityGateError(f"{label} is outside its approved root") from exc
    if not allow_equal and not relative.parts:
        raise QualityGateError(f"{label} must be a strict child of its approved root")
    reject_symlink(lexical_path, boundary=lexical_root)
    return lexical_path


def _paths_overlap(left: Path, right: Path) -> bool:
    left_text = os.path.normcase(os.path.abspath(left)).rstrip("\\/")
    right_text = os.path.normcase(os.path.abspath(right)).rstrip("\\/")
    separator = os.sep
    return (
        left_text == right_text
        or left_text.startswith(right_text + separator)
        or right_text.startswith(left_text + separator)
    )


def _require_exact_file(path: Path, *, size: int, sha256: str, label: str) -> dict[str, Any]:
    actual = _safe_file_record(path)
    if actual != {"bytes": size, "sha256": sha256}:
        raise QualityGateError(f"{label} is not the exact frozen file")
    return actual


def _bound_path_record(value: Any, label: str) -> tuple[Path, dict[str, Any]]:
    item = require_mapping(value, label)
    path = Path(require_string(item.get("path"), f"{label} path"))
    if not path.is_absolute():
        raise QualityGateError(f"{label} path must be absolute")
    actual = _safe_file_record(path)
    if item.get("bytes") != actual["bytes"] or str(item.get("sha256", "")).upper() != actual["sha256"]:
        raise QualityGateError(f"{label} bytes or SHA-256 are stale")
    return path.resolve(strict=True), actual


def _require_document_integrity(document: dict[str, Any], label: str) -> None:
    integrity = require_mapping(document.get("integrity"), f"{label} integrity")
    expected = require_sha256(
        integrity.get("canonical_payload_sha256"),
        f"{label} integrity canonical_payload_sha256",
    )
    payload = dict(document)
    payload.pop("integrity", None)
    if sha256_bytes(canonical_json_bytes(payload)) != expected:
        raise QualityGateError(f"{label} integrity hash is wrong")


def _validate_frozen_truth(
    formal: dict[str, Any], artifact_paths: dict[str, Path], normalized_human: str
) -> dict[str, Any]:
    truth = require_mapping(formal.get("truth"), "formal truth binding")
    truth_root = Path(require_string(truth.get("root"), "formal truth root"))
    if not truth_root.is_absolute():
        raise QualityGateError("Formal truth root must be absolute")
    truth_root = truth_root.resolve(strict=True)

    manifest_path, _ = _bound_path_record(truth.get("manifest"), "frozen truth manifest")
    _strict_child(manifest_path, truth_root, "frozen truth manifest")
    _require_exact_file(
        manifest_path,
        size=FROZEN_TRUTH_MANIFEST_BYTES,
        sha256=FROZEN_TRUTH_MANIFEST_SHA256,
        label="frozen truth manifest",
    )
    manifest = require_mapping(read_json(manifest_path, "frozen truth manifest"), "frozen truth manifest")
    if (
        manifest.get("schema_version") != 1
        or manifest.get("stage") != "MOSS_FUNCTIONAL_FIX_Q00_FROZEN_TRUTH_MANIFEST"
        or manifest.get("status") != "FROZEN_FROM_APPROVED_FULL_WINDOW_HUMAN_REVIEW"
    ):
        raise QualityGateError("Frozen truth manifest identity is wrong")

    fixed = {
        "human_review": (FROZEN_HUMAN_REVIEW_BYTES, FROZEN_HUMAN_REVIEW_SHA256),
        "pre_meeting_context": (FROZEN_PREMEETING_CONTEXT_BYTES, FROZEN_PREMEETING_CONTEXT_SHA256),
        "truth_package_manifest": (FROZEN_PACKAGE_MANIFEST_BYTES, FROZEN_PACKAGE_MANIFEST_SHA256),
        "derived_review_provenance": (FROZEN_REVIEW_PROVENANCE_BYTES, FROZEN_REVIEW_PROVENANCE_SHA256),
    }
    resolved: dict[str, Path] = {}
    for role, (size, digest) in fixed.items():
        path, _ = _bound_path_record(truth.get(role), f"frozen {role}")
        _strict_child(path, truth_root, f"frozen {role}")
        _require_exact_file(path, size=size, sha256=digest, label=f"frozen {role}")
        resolved[role] = path

    _require_exact_file(
        artifact_paths["human_verbatim"],
        size=FROZEN_HUMAN_VERBATIM_BYTES,
        sha256=FROZEN_HUMAN_VERBATIM_SHA256,
        label="human verbatim truth",
    )
    _require_exact_file(
        artifact_paths["speaker_truth"],
        size=FROZEN_SPEAKER_TRUTH_BYTES,
        sha256=FROZEN_SPEAKER_TRUTH_SHA256,
        label="speaker truth",
    )
    _require_exact_file(
        artifact_paths["positive_truth"],
        size=FROZEN_POSITIVE_TRUTH_BYTES,
        sha256=FROZEN_POSITIVE_TRUTH_SHA256,
        label="positive truth",
    )
    _require_exact_file(
        artifact_paths["negative_truth"],
        size=FROZEN_NEGATIVE_TRUTH_BYTES,
        sha256=FROZEN_NEGATIVE_TRUTH_SHA256,
        label="negative truth",
    )
    for role in ("human_verbatim", "speaker_truth", "positive_truth", "negative_truth"):
        _strict_child(artifact_paths[role], truth_root, role)

    scope = require_mapping(manifest.get("scope"), "frozen truth manifest scope")
    if scope != {
        "source_audio_sha256": SOURCE_SHA256,
        "window_audio_sha256": WINDOW_SHA256,
        "window_start_ms": 70_370,
        "window_end_ms": 296_810,
        "window_duration_ms": 226_440,
    }:
        raise QualityGateError("Frozen truth manifest scope is not the exact Q00 window")
    manifest_files = require_mapping(manifest.get("files"), "frozen truth manifest files")
    expected_manifest_files = {
        "truth_package_manifest": ("MANIFEST.json", FROZEN_PACKAGE_MANIFEST_BYTES, FROZEN_PACKAGE_MANIFEST_SHA256),
        "human_verbatim": ("04-human-verbatim.tsv", FROZEN_HUMAN_VERBATIM_BYTES, FROZEN_HUMAN_VERBATIM_SHA256),
        "speaker_truth": ("05-human-speaker-turns.tsv", FROZEN_SPEAKER_TRUTH_BYTES, FROZEN_SPEAKER_TRUTH_SHA256),
        "human_review": ("06-human-review.json", FROZEN_HUMAN_REVIEW_BYTES, FROZEN_HUMAN_REVIEW_SHA256),
        "derived_review_provenance": (
            "17-derived-human-review-provenance.json",
            FROZEN_REVIEW_PROVENANCE_BYTES,
            FROZEN_REVIEW_PROVENANCE_SHA256,
        ),
        "pre_meeting_context": ("16-pre-meeting-context.json", FROZEN_PREMEETING_CONTEXT_BYTES, FROZEN_PREMEETING_CONTEXT_SHA256),
        "positive_truth": ("01-local-positive-terms-frozen.json", FROZEN_POSITIVE_TRUTH_BYTES, FROZEN_POSITIVE_TRUTH_SHA256),
        "negative_truth": ("Q00-NEGATIVE-TRUTH.json", FROZEN_NEGATIVE_TRUTH_BYTES, FROZEN_NEGATIVE_TRUTH_SHA256),
    }
    if set(manifest_files) != set(expected_manifest_files):
        raise QualityGateError("Frozen truth manifest file roles are not exact")
    for role, (relative, size, digest) in expected_manifest_files.items():
        row = require_mapping(manifest_files[role], f"frozen truth manifest {role}")
        if row != {"relative_path": relative, "bytes": size, "sha256": digest}:
            raise QualityGateError(f"Frozen truth manifest record is wrong for {role}")
        _require_exact_file(truth_root / relative, size=size, sha256=digest, label=f"frozen truth package {role}")

    review = require_mapping(read_json(resolved["human_review"], "human review"), "human review")
    coverage = require_mapping(review.get("playback_coverage"), "human review playback_coverage")
    if not (
        review.get("review_status") == "HUMAN_VERIFIED"
        and review.get("approved_as_ground_truth") is True
        and review.get("listened_from_start_to_end") is True
        and coverage.get("complete") is True
        and _finite_number(coverage.get("coverage_percent"), "human coverage percent") == 100.0
        and str(review.get("audio_sha256", "")).upper() == WINDOW_SHA256
        and str(review.get("verbatim_tsv_sha256", "")).upper() == FROZEN_HUMAN_VERBATIM_SHA256
        and str(review.get("speaker_turns_tsv_sha256", "")).upper() == FROZEN_SPEAKER_TRUTH_SHA256
    ):
        raise QualityGateError("The bound human review is not the completed approved full-window review")

    positive_doc = require_mapping(read_json(artifact_paths["positive_truth"], "positive truth"), "positive truth")
    source_bindings = require_mapping(positive_doc.get("source_bindings"), "positive source bindings")
    if (
        positive_doc.get("status") != "HUMAN_TRUTH_DERIVED_POSITIVE_FROZEN"
        or str(source_bindings.get("human_verbatim_sha256", "")).upper() != FROZEN_HUMAN_VERBATIM_SHA256
        or str(source_bindings.get("human_review_sha256", "")).upper() != FROZEN_HUMAN_REVIEW_SHA256
        or str(source_bindings.get("pre_meeting_context_sha256", "")).upper() != FROZEN_PREMEETING_CONTEXT_SHA256
    ):
        raise QualityGateError("Positive truth is not bound to the approved review bundle")
    positive_rows: list[dict[str, Any]] = []
    for index, raw in enumerate(require_list(positive_doc.get("positive_spoken_terms"), "positive spoken terms"), 1):
        row = require_mapping(raw, f"positive term {index}")
        term = require_string(row.get("term"), f"positive term {index} term")
        expected = _integer(row.get("expected_occurrences"), f"positive term {index} count", minimum=1)
        if _count_occurrences(normalized_human, term) != expected:
            raise QualityGateError("Frozen positive occurrence count no longer matches human truth")
        positive_rows.append({
            "term": term,
            "normalized": _normalize_cer_text(term),
            "expected": expected,
            "source_evidence_sha256": FROZEN_HUMAN_VERBATIM_SHA256,
        })
    if [row["term"] for row in positive_rows] != ["YouTube", "PWA", "Google"]:
        raise QualityGateError("Frozen positive term set is not exact")

    context = require_mapping(read_json(resolved["pre_meeting_context"], "pre-meeting context"), "pre-meeting context")
    if context.get("status") != "FROZEN_BEFORE_TRANSCRIPTION" or context.get("available_before_transcription") is not True:
        raise QualityGateError("Pre-meeting context was not frozen before transcription")
    context_business: dict[str, str] = {}
    for raw in require_list(context.get("entries"), "pre-meeting context entries"):
        row = require_mapping(raw, "pre-meeting context entry")
        if row.get("type") == "business_term":
            context_business[require_string(row.get("entry_id"), "context entry id")] = require_string(
                row.get("term"), "context term"
            )
    derived = [
        (entry_id, term)
        for entry_id, term in context_business.items()
        if _count_occurrences(normalized_human, term) == 0
    ]
    negative_doc = require_mapping(read_json(artifact_paths["negative_truth"], "negative truth"), "negative truth")
    if (
        negative_doc.get("schema_version") != 2
        or negative_doc.get("status") != "MECHANICALLY_DERIVED_FROM_APPROVED_HUMAN_TRUTH"
        or str(negative_doc.get("human_review_sha256", "")).upper() != FROZEN_HUMAN_REVIEW_SHA256
        or str(negative_doc.get("pre_meeting_context_sha256", "")).upper() != FROZEN_PREMEETING_CONTEXT_SHA256
    ):
        raise QualityGateError("Negative truth is not the frozen mechanical derivation")
    negative_rows: list[dict[str, Any]] = []
    declared: list[tuple[str, str]] = []
    for index, raw in enumerate(require_list(negative_doc.get("terms"), "negative terms"), 1):
        row = require_mapping(raw, f"negative term {index}")
        entry_id = require_string(row.get("context_entry_id"), f"negative term {index} context_entry_id")
        term = require_string(row.get("term"), f"negative term {index} term")
        if row.get("expected_occurrences") != 0:
            raise QualityGateError("Negative truth expected occurrence must be zero")
        declared.append((entry_id, term))
        negative_rows.append({
            "term": term,
            "normalized": _normalize_cer_text(term),
            "expected": 0,
            "source_evidence_sha256": FROZEN_HUMAN_VERBATIM_SHA256,
        })
    if declared != derived or declared != [
        ("CTX-025", "M100"),
        ("CTX-028", "H5"),
        ("CTX-030", "VIP"),
        ("CTX-031", "A/B Test"),
        ("CTX-032", "TG"),
    ]:
        raise QualityGateError("Negative truth is not the complete deterministic absent-term set")
    return {"positive": positive_rows, "negative": negative_rows, "review": review}


def _validate_candidate(formal: dict[str, Any], source_commit: str) -> dict[str, Any]:
    candidate = require_mapping(formal.get("candidate"), "formal candidate binding")
    expected_candidate_keys = {
        "execution_install_root",
        "installed_snapshot_root",
        "build_manifest",
        "models",
        "moss_runtime",
    }
    if set(candidate) != expected_candidate_keys:
        raise QualityGateError("Formal candidate binding fields are not exact")
    execution_install_root = Path(
        require_string(candidate.get("execution_install_root"), "candidate execution_install_root")
    )
    snapshot_root = Path(
        require_string(candidate.get("installed_snapshot_root"), "candidate installed_snapshot_root")
    )
    if not execution_install_root.is_absolute() or not snapshot_root.is_absolute():
        raise QualityGateError("Candidate execution and snapshot roots must be absolute")
    approved_execution_root = Path(os.environ["LOCALAPPDATA"]) / APPROVED_PRODUCT_NAME
    if os.path.normcase(os.path.abspath(execution_install_root)) != os.path.normcase(
        os.path.abspath(approved_execution_root)
    ):
        raise QualityGateError("Candidate execution install root is not the isolated product root")
    snapshot_root = snapshot_root.resolve(strict=True)
    manifest_path, manifest_record = _bound_path_record(candidate.get("build_manifest"), "candidate build manifest")
    manifest = require_mapping(read_json(manifest_path, "candidate build manifest"), "candidate build manifest")
    if (
        manifest.get("schema_version") != 1
        or manifest.get("stage") != CANDIDATE_BUILD_MANIFEST_STAGE
        or manifest.get("status") != PASS
        or manifest.get("role") != "candidate"
        or manifest.get("source_commit") != source_commit
        or manifest.get("product_name") != APPROVED_PRODUCT_NAME
        or manifest.get("bundle_id") != APPROVED_BUNDLE_ID
    ):
        raise QualityGateError("Candidate build manifest is not for the current candidate commit")
    program_rows = require_list(manifest.get("installed_files"), "candidate installed_files")
    by_role: dict[str, dict[str, Any]] = {}
    for raw in program_rows:
        row = require_mapping(raw, "candidate installed file")
        role = require_string(row.get("role"), "candidate installed role")
        if role in by_role:
            raise QualityGateError("Candidate build manifest contains duplicate installed roles")
        by_role[role] = row
    if set(by_role) != set(PROGRAM_CONTRACTS):
        raise QualityGateError("Candidate build manifest installed roles are not the exact seven-role set")
    programs: dict[str, dict[str, Any]] = {}
    for role, expected_relative in PROGRAM_CONTRACTS.items():
        row = require_mapping(by_role.get(role), f"candidate {role}")
        relative = require_string(row.get("relative_path"), f"candidate {role} relative_path").replace("\\", "/")
        if relative.casefold() != expected_relative.casefold():
            raise QualityGateError(f"Candidate {role} path has the wrong product meaning")
        path = _strict_child(
            snapshot_root / Path(*relative.split("/")), snapshot_root, f"candidate {role} snapshot"
        )
        actual = _safe_file_record(path)
        if row.get("bytes") != actual["bytes"] or str(row.get("sha256", "")).upper() != actual["sha256"]:
            raise QualityGateError(f"Candidate {role} does not match the build manifest")
        programs[role] = {**actual, "path": path}

    models = require_mapping(candidate.get("models"), "candidate models")
    validated_models: dict[str, dict[str, Any]] = {}
    if set(models) != set(MODEL_CONTRACTS):
        raise QualityGateError("Candidate model roles must be exactly moss, whisper and qwen_2b")
    for role, contract in MODEL_CONTRACTS.items():
        path, _ = _bound_path_record(models[role], f"candidate {role} model")
        if path.name.casefold() != str(contract["filename"]).casefold():
            raise QualityGateError(f"Candidate {role} model filename is wrong")
        actual = _require_exact_file(
            path,
            size=int(contract["bytes"]),
            sha256=str(contract["sha256"]),
            label=f"candidate {role} model",
        )
        validated_models[role] = {**actual, "path": path}

    runtime = require_mapping(candidate.get("moss_runtime"), "candidate MOSS runtime")
    runtime_root = Path(require_string(runtime.get("root"), "candidate MOSS runtime root"))
    if not runtime_root.is_absolute():
        raise QualityGateError("Candidate MOSS runtime root must be absolute")
    runtime_root = runtime_root.resolve(strict=True)
    contract_path, contract_record = _bound_path_record(
        runtime.get("contract"), "candidate MOSS runtime contract"
    )
    _strict_child(contract_path, runtime_root, "candidate MOSS runtime contract")
    if contract_record != {
        "bytes": 125,
        "sha256": "C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73",
    }:
        raise QualityGateError("Candidate MOSS runtime contract is not the approved v0.2.2 package")
    runtime_files = require_list(runtime.get("files"), "candidate MOSS runtime files")
    runtime_seen: set[str] = set()
    for index, raw in enumerate(runtime_files):
        row = require_mapping(raw, f"candidate MOSS runtime file {index}")
        relative = require_string(row.get("relative_path"), "candidate MOSS runtime relative_path").replace("\\", "/")
        if relative in runtime_seen or relative.startswith("/") or any(
            part in {"", ".", ".."} for part in relative.split("/")
        ):
            raise QualityGateError("Candidate MOSS runtime file path is duplicate or unsafe")
        runtime_seen.add(relative)
        path = _strict_child(
            runtime_root / Path(*relative.split("/")), runtime_root, "candidate MOSS runtime file"
        )
        actual = _safe_file_record(path)
        if row.get("bytes") != actual["bytes"] or str(row.get("sha256", "")).upper() != actual["sha256"]:
            raise QualityGateError("Candidate MOSS runtime file record is stale")
    if "contract.json" not in runtime_seen or len(runtime_seen) < 4:
        raise QualityGateError("Candidate MOSS runtime package is incomplete")
    return {
        "manifest": manifest_record,
        "programs": programs,
        "models": validated_models,
        "runtime": {"contract": contract_record, "file_count": len(runtime_seen)},
        "execution_install_root": execution_install_root,
        "snapshot_root": snapshot_root,
    }


def _validate_session(
    formal: dict[str, Any], bindings: dict[str, Any], artifact_paths: dict[str, Path], source_commit: str
) -> dict[str, Any]:
    session_path, _ = _bound_path_record(formal.get("session"), "formal gate session")
    session = require_mapping(read_json(session_path, "formal gate session"), "formal gate session")
    _require_document_integrity(session, "formal gate session")
    if (
        session.get("schema_version") != 1
        or session.get("stage") != FORMAL_SESSION_STAGE
        or session.get("status") != "STARTED"
        or session.get("created_by_gate") is not True
        or session.get("source_commit") != source_commit
        or session.get("run_id") != bindings["run_id"]
        or session.get("maximum_moss_seconds") != 1_800
    ):
        raise QualityGateError("Formal gate session identity is wrong")
    repository_before = require_mapping(
        session.get("repository_status_before"), "formal repository status before"
    )
    if (
        repository_before.get("format") != "git-status-porcelain-v1-z-with-all-untracked"
        or repository_before.get("entry_count") != 0
    ):
        raise QualityGateError("Formal Q00 did not start from a completely clean repository")
    if require_mapping(session.get("gate_program"), "formal gate program") != _safe_file_record(
        Path(__file__).resolve(strict=True)
    ):
        raise QualityGateError("Formal gate program no longer matches the session")
    runner_path, runner_record = _bound_path_record(session.get("formal_runner"), "formal Q00 runner")
    node_path, node_record = _bound_path_record(session.get("node_runtime"), "formal Node runtime")
    cdp_path, cdp_record = _bound_path_record(session.get("cdp_runner"), "formal CDP runner")
    repository = Path(__file__).resolve(strict=True).parents[2]
    if runner_path != repository / "scripts" / "qa" / "moss_functional_fix_q00_formal.py":
        raise QualityGateError("Formal Q00 runner path is not fixed")
    if cdp_path != repository / "scripts" / "qa" / "moss-functional-ft-cdp.mjs":
        raise QualityGateError("Formal CDP runner path is not fixed")
    nonce = require_string(session.get("nonce"), "formal session nonce")
    if not NONCE_RE.fullmatch(nonce) or not bindings["run_id"].endswith(nonce):
        raise QualityGateError("Formal session nonce is invalid or not bound to run_id")
    private_root = Path(require_string(session.get("private_root"), "private_root"))
    public_root = Path(require_string(session.get("public_root"), "public_root"))
    run_directory = Path(require_string(session.get("run_directory"), "run_directory"))
    for label, path in (("private_root", private_root), ("public_root", public_root), ("run_directory", run_directory)):
        if not path.is_absolute():
            raise QualityGateError(f"{label} must be absolute")
    private_root = private_root.resolve(strict=True)
    public_root = public_root.resolve(strict=True)
    run_directory = _strict_child(run_directory, private_root, "run_directory")
    if _paths_overlap(private_root, public_root):
        raise QualityGateError("Public and private roots must be disjoint")
    _strict_child(session_path, run_directory, "formal gate session")

    receipt_path, _ = _bound_path_record(formal.get("execution_receipt"), "formal execution receipt")
    _strict_child(receipt_path, run_directory, "formal execution receipt")
    receipt = require_mapping(read_json(receipt_path, "formal execution receipt"), "formal execution receipt")
    _require_document_integrity(receipt, "formal execution receipt")
    if (
        receipt.get("schema_version") != 1
        or receipt.get("stage") != FORMAL_RECEIPT_STAGE
        or receipt.get("status") != "COMPLETED"
        or receipt.get("run_id") != bindings["run_id"]
        or receipt.get("nonce") != nonce
        or receipt.get("source_commit") != source_commit
        or receipt.get("created_by_gate") is not True
        or receipt.get("run_once") is not True
    ):
        raise QualityGateError("Execution receipt is not a gate-controlled one-time run")
    marker_record = require_mapping(receipt.get("start_marker"), "formal start marker record")
    marker_path, marker_actual = _bound_path_record(marker_record, "formal start marker")
    _strict_child(marker_path, run_directory, "formal start marker")
    marker = require_mapping(read_json(marker_path, "formal start marker"), "formal start marker")
    _require_document_integrity(marker, "formal start marker")
    if (
        marker.get("stage") != "MOSS_FUNCTIONAL_FIX_Q00_MOSS_START_MARKER"
        or marker.get("run_id") != bindings["run_id"]
        or marker.get("nonce") != nonce
        or marker.get("created_by_gate") is not True
        or marker_actual != {"bytes": marker_record.get("bytes"), "sha256": str(marker_record.get("sha256", "")).upper()}
    ):
        raise QualityGateError("Formal MOSS start marker identity is wrong")
    timing = require_mapping(receipt.get("timing"), "execution timing")
    start_ns = _integer(timing.get("moss_started_monotonic_ns"), "MOSS monotonic start", minimum=1)
    end_ns = _integer(timing.get("moss_completed_monotonic_ns"), "MOSS monotonic completion", minimum=1)
    if end_ns <= start_ns or timing.get("clock") != "time.monotonic_ns":
        raise QualityGateError("MOSS timing is not a positive gate monotonic interval")
    elapsed = (end_ns - start_ns) / 1_000_000_000.0
    if abs(
        _finite_number(timing.get("moss_elapsed_seconds"), "MOSS elapsed seconds", minimum=0.0)
        - elapsed
    ) > 1e-9:
        raise QualityGateError("MOSS elapsed seconds disagree with the monotonic timestamps")
    runner = require_mapping(receipt.get("runner"), "formal execution runner")
    if (
        require_mapping(runner.get("program"), "formal runner program") != session["node_runtime"]
        or require_mapping(runner.get("script"), "formal runner script") != session["cdp_runner"]
        or Path(require_string(runner.get("working_directory"), "formal runner cwd")).resolve(strict=True) != repository
    ):
        raise QualityGateError("Formal execution runner is not the session-bound Node/CDP program")
    arguments = require_list(runner.get("arguments"), "formal runner arguments")
    if len(arguments) != 5 or arguments[0] != str(cdp_path) or arguments[1] != "moss-complete":
        raise QualityGateError("Formal MOSS runner arguments are not the fixed action")
    for raw in arguments[2:]:
        _strict_child(Path(require_string(raw, "formal runner path argument")), run_directory, "formal runner path argument")
    cleanup = require_mapping(receipt.get("cleanup"), "execution cleanup")
    if (
        receipt.get("timed_out") is not False
        or receipt.get("runner_exit_code") != 0
        or cleanup.get("completed") is not True
        or cleanup.get("consecutive_zero_scans") != 2
        or cleanup.get("cdp_listener_closed") is not True
        or require_list(cleanup.get("residual_processes"), "residual processes")
    ):
        raise QualityGateError("Gate-controlled run timed out, failed, or left a process tree")
    repository_after = require_mapping(
        receipt.get("repository_status_after"), "formal repository status after"
    )
    if (
        repository_after.get("format") != "git-status-porcelain-v1-z-with-all-untracked"
        or repository_after.get("entry_count") != 0
    ):
        raise QualityGateError("Formal Q00 changed the repository")
    session_started = _parse_iso8601(session.get("started_at"), "formal session started_at")
    receipt_completed = _parse_iso8601(receipt.get("completed_at"), "formal receipt completed_at")
    if bindings["started"] < session_started or bindings["completed"] < receipt_completed:
        raise QualityGateError("Q00 binding interval does not enclose the formal session evidence")
    for role, path in artifact_paths.items():
        if ARTIFACT_ORIGINS[role] in {"CURRENT_RUN", "PREPARED_WINDOW"}:
            _strict_child(path, run_directory, f"current artifact {role}")
    _strict_child(Path(bindings["record_path"]), run_directory, "run bindings")
    return {
        "session": session,
        "receipt": receipt,
        "nonce": nonce,
        "private_root": private_root,
        "public_root": public_root,
        "run_directory": run_directory,
        "moss_elapsed_seconds": elapsed,
        "app_process": require_mapping(receipt.get("app_process"), "formal app process"),
        "runner_records": {"formal": runner_record, "node": node_record, "cdp": cdp_record},
    }


def _replace_char_range(value: str, start: int, end: int, original: str, replacement: str) -> str:
    chars = list(value)
    if start < 0 or end <= start or end > len(chars) or "".join(chars[start:end]) != original:
        raise QualityGateError("Stored term correction cannot be replayed from its prior text")
    result = "".join(chars[:start]) + replacement + "".join(chars[end:])
    if not result.strip():
        raise QualityGateError("Stored term correction replays to empty text")
    return result


def _meeting_context_snapshot_sha256(snapshot: dict[str, Any]) -> str:
    hashable = dict(snapshot)
    hashable["context_sha256"] = ""
    return sha256_bytes(canonical_json_bytes(hashable)).lower()


def _sqlite_rows(connection: sqlite3.Connection, sql: str, params: tuple[Any, ...], label: str) -> list[sqlite3.Row]:
    try:
        return list(connection.execute(sql, params).fetchall())
    except sqlite3.Error as exc:
        raise QualityGateError(f"Cannot query product database for {label}: {exc}") from exc


def _validate_product_state(
    formal: dict[str, Any],
    current_documents: dict[str, dict[str, Any]],
    candidate: dict[str, Any],
    session: dict[str, Any],
) -> dict[str, Any]:
    state = require_mapping(formal.get("product_state"), "formal product_state")
    if set(state) != {
        "database",
        "snapshot_kind",
        "source_database_sha256",
        "source_sidecars_absent",
        "meeting_id",
        "moss_run_id",
        "activation_id",
        "summary_generation_id",
        "product_context",
    }:
        raise QualityGateError("Formal product_state fields are not exact")
    database_path, database_record = _bound_path_record(state.get("database"), "product database")
    if (
        state.get("snapshot_kind") != "POST_CLEAN_EXIT_IMMUTABLE_MAIN_DATABASE"
        or state.get("source_sidecars_absent") is not True
        or str(state.get("source_database_sha256", "")).upper() != database_record["sha256"]
    ):
        raise QualityGateError("Product database is not the clean-exit immutable snapshot")
    for suffix in ("-wal", "-shm", "-journal"):
        if Path(str(database_path) + suffix).exists():
            raise QualityGateError("Immutable product database snapshot has a SQLite sidecar")
    database_before = _safe_file_record(database_path)
    meeting_id = require_string(state.get("meeting_id"), "product meeting_id")
    product_run_id = require_string(state.get("moss_run_id"), "product moss_run_id")
    activation_id = require_string(state.get("activation_id"), "product activation_id")
    summary_id = require_string(state.get("summary_generation_id"), "product summary_generation_id")
    context_path, _ = _bound_path_record(state.get("product_context"), "product meeting context")
    context_document = require_mapping(
        read_json(context_path, "product meeting context"), "product meeting context"
    )
    context_container = require_mapping(
        context_document.get("meeting_context"), "product meeting context container"
    )
    current_context_id = require_string(
        context_container.get("current_context_id"), "product current_context_id"
    )
    context_rows = [
        require_mapping(item, "product context row")
        for item in require_list(context_container.get("contexts"), "product context rows")
    ]
    current_contexts = [item for item in context_rows if item.get("context_id") == current_context_id]
    if len(current_contexts) != 1:
        raise QualityGateError("Product meeting context lacks one exact current snapshot")
    current_context = current_contexts[0]
    if (
        context_container.get("schema_version") != 1
        or context_container.get("recording_context_id") != current_context_id
        or len(context_rows) != 1
        or current_context.get("revision") != 1
        or current_context.get("reason") != "recording_start"
    ):
        raise QualityGateError("Product meeting context is not the one frozen pre-MOSS snapshot")
    context_people = [
        require_mapping(item, "product context person")
        for item in require_list(current_context.get("people"), "product context people")
    ]
    if [item.get("person_id") for item in context_people] != ["H01", "H04"] or any(
        item.get("attendance") != "attending" for item in context_people
    ):
        raise QualityGateError("Product Q00 speaker identities are not the exact frozen two-person set")
    context_terms = [
        require_mapping(item, "product context term")
        for item in require_list(current_context.get("terms"), "product context terms")
    ]
    if [item.get("canonical") for item in context_terms] != [
        "M100",
        "YouTube",
        "PWA",
        "H5",
        "Google",
        "VIP",
        "A/B Test",
        "TG",
    ]:
        raise QualityGateError("Product Q00 business-term context is not exact")
    source = require_mapping(current_context.get("source"), "product context source")
    if str(source.get("template_file_sha256", "")).upper() != FROZEN_TRUTH_MANIFEST_SHA256:
        raise QualityGateError("Product Q00 context is not bound to the frozen truth manifest")
    if _parse_iso8601(current_context.get("captured_at"), "product context captured_at") > _parse_iso8601(
        session["receipt"].get("started_at"), "formal MOSS started_at"
    ):
        raise QualityGateError("Product meeting context was not frozen before the MOSS action")
    product_context_sha256 = require_sha256(
        current_context.get("context_sha256"), "product current context SHA-256"
    ).lower()
    calculated_context_sha256 = _meeting_context_snapshot_sha256(current_context)
    if product_context_sha256 != calculated_context_sha256:
        raise QualityGateError("Product meeting context SHA-256 does not match its canonical snapshot")
    uri = database_path.as_uri() + "?mode=ro&immutable=1"
    try:
        connection = sqlite3.connect(uri, uri=True, timeout=5.0)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA query_only=ON")
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise QualityGateError("Product database integrity_check is not ok")
        connection.execute("BEGIN")
        run_rows = _sqlite_rows(
            connection,
            "SELECT * FROM moss_transcription_runs WHERE run_id=? AND meeting_id=?",
            (product_run_id, meeting_id),
            "MOSS run",
        )
        if len(run_rows) != 1:
            raise QualityGateError("Product database does not contain exactly one bound MOSS run")
        run = run_rows[0]
        if (
            run["status"] != "completed"
            or str(run["audio_sha256"]).upper() != WINDOW_SHA256
            or str(run["model_sha256"]).upper() != MODEL_CONTRACTS["moss"]["sha256"]
            or str(run["runtime_sha256"]).upper() != candidate["runtime"]["contract"]["sha256"]
            or str(run["context_sha256"]).lower() != product_context_sha256
            or run["wall_elapsed_ms"] is None
            or int(run["wall_elapsed_ms"]) <= 0
            or int(run["wall_elapsed_ms"]) / 1000.0 > session["moss_elapsed_seconds"] + 0.001
        ):
            raise QualityGateError("Persisted MOSS run is not the bound window/model/runtime completion")
        segments = _sqlite_rows(
            connection,
            "SELECT * FROM moss_candidate_segments WHERE run_id=? ORDER BY segment_index",
            (product_run_id,),
            "MOSS candidate segments",
        )
        if not segments or int(run["segment_count"]) != len(segments):
            raise QualityGateError("Persisted MOSS segment count is incomplete")

        raw_doc = current_documents["moss_raw"]
        raw_contract = _parse_inference_contract(raw_doc, "moss_raw")
        try:
            persisted_decode_contract = {
                "language_requested": str(run["language_requested"]),
                "language_resolved": str(run["language_resolved"]),
                "decode_parameters_json": str(run["decode_parameters_json"]),
                "decode_parameters_sha256": str(
                    run["decode_parameters_sha256"]
                ).upper(),
            }
        except (KeyError, IndexError, TypeError) as exc:
            raise QualityGateError(
                "Persisted MOSS run is missing native decode contract fields"
            ) from exc
        if persisted_decode_contract != {
            "language_requested": raw_contract["language_requested"],
            "language_resolved": raw_contract["language_resolved"],
            "decode_parameters_json": raw_contract["decode_parameters_json"],
            "decode_parameters_sha256": raw_contract["decode_parameters_sha256"],
        }:
            raise QualityGateError(
                "moss_raw native decode contract does not equal the persisted MOSS run"
            )
        raw_segments = require_list(raw_doc.get("segments"), "moss_raw segments")
        if len(raw_segments) != len(segments):
            raise QualityGateError("moss_raw does not equal persisted candidate segments")
        materialized: list[dict[str, Any]] = []
        correction_ids: list[str] = []
        override_ids: list[str] = []
        persisted_override_audits: list[dict[str, Any]] = []
        for index, db_segment in enumerate(segments):
            raw = require_mapping(raw_segments[index], f"moss_raw segment {index}")
            expected_raw = {
                "start_seconds": int(db_segment["start_ms"]) / 1000.0,
                "end_seconds": int(db_segment["end_ms"]) / 1000.0,
                "speaker_id": str(db_segment["speaker_label"]),
                "text": str(db_segment["raw_text"]),
            }
            if any(raw.get(key) != value for key, value in expected_raw.items()):
                raise QualityGateError("moss_raw contains text/timing/speaker not present in product state")
            text_value = str(db_segment["raw_text"])
            corrections = _sqlite_rows(
                connection,
                "SELECT * FROM moss_term_corrections WHERE segment_id=? AND reverted_at IS NULL ORDER BY revision",
                (db_segment["segment_id"],),
                "term corrections",
            )
            for correction in corrections:
                text_value = _replace_char_range(
                    text_value,
                    int(correction["start_char"]),
                    int(correction["end_char"]),
                    str(correction["original_text"]),
                    str(correction["replacement_text"]),
                )
                if text_value != str(correction["result_text"]):
                    raise QualityGateError("Stored correction result_text is not a deterministic replay")
                correction_ids.append(str(correction["correction_id"]))
            overrides = _sqlite_rows(
                connection,
                "SELECT * FROM moss_segment_overrides WHERE segment_id=? AND revoked_at IS NULL",
                (db_segment["segment_id"],),
                "segment overrides",
            )
            if len(overrides) > 1:
                raise QualityGateError("Product state contains more than one active segment override")
            override = overrides[0] if overrides else None
            if override is not None and override["replacement_text"] is not None:
                text_value = str(override["replacement_text"])
            if override is not None:
                override_ids.append(str(override["override_id"]))
                override_history = _sqlite_rows(
                    connection,
                    "SELECT * FROM moss_segment_overrides WHERE segment_id=? ORDER BY revision",
                    (db_segment["segment_id"],),
                    "segment override history",
                )
                if len(override_history) != 2:
                    raise QualityGateError(
                        "Product state does not contain one wrong and one corrected speaker override"
                    )
                wrong_override, corrected_override = override_history
                before_person = str(wrong_override["person_id"] or "")
                after_person = str(corrected_override["person_id"] or "")
                if (
                    wrong_override["revoked_at"] is None
                    or corrected_override["revoked_at"] is not None
                    or wrong_override["reason_code"] != "MANUAL_PERSON_OVERRIDE"
                    or corrected_override["reason_code"] != "MANUAL_PERSON_OVERRIDE"
                    or not before_person
                    or before_person == after_person
                    or str(corrected_override["override_id"]) != str(override["override_id"])
                ):
                    raise QualityGateError(
                        "Product state speaker override history is a no-op or is not wrong-to-correct"
                    )
                persisted_override_audits.append(
                    {
                        "wrong_override_id": str(wrong_override["override_id"]),
                        "override_id": str(corrected_override["override_id"]),
                        "segment_id": str(db_segment["segment_id"]),
                        "segment_index": index,
                        "source": "HUMAN_SINGLE_SEGMENT_OVERRIDE",
                        "before_speaker_id": before_person,
                        "after_speaker_id": after_person,
                        "reference_speaker_id": after_person,
                    }
                )
            binding_rows = _sqlite_rows(
                connection,
                "SELECT * FROM moss_speaker_bindings WHERE run_id=? AND speaker_label=? AND revoked_at IS NULL",
                (product_run_id, db_segment["speaker_label"]),
                "speaker bindings",
            )
            if len(binding_rows) > 1:
                raise QualityGateError("Product state contains duplicate active speaker bindings")
            person_id = None
            if override is not None and override["person_id"] is not None:
                person_id = str(override["person_id"])
            elif binding_rows:
                person_id = str(binding_rows[0]["person_id"])
            materialized.append({
                "start_seconds": int(db_segment["start_ms"]) / 1000.0,
                "end_seconds": int(db_segment["end_ms"]) / 1000.0,
                "speaker_id": person_id or str(db_segment["speaker_label"]),
                "text": text_value,
                "candidate_segment_id": str(db_segment["segment_id"]),
            })

        corrected_doc = current_documents["corrected"]
        corrected_rows = require_list(corrected_doc.get("segments"), "corrected segments")
        if len(corrected_rows) != len(materialized):
            raise QualityGateError("Corrected transcript segment count is not the persisted materialization")
        for index, expected in enumerate(materialized):
            row = require_mapping(corrected_rows[index], f"corrected segment {index}")
            if any(row.get(key) != expected[key] for key in ("start_seconds", "end_seconds", "speaker_id", "text")):
                raise QualityGateError("Corrected transcript cannot be replayed from raw product state")
        declared_overrides = require_list(corrected_doc.get("manual_speaker_overrides"), "manual speaker overrides")
        if len(persisted_override_audits) != 1 or declared_overrides != persisted_override_audits:
            raise QualityGateError(
                "Corrected transcript true-flip audit does not equal persisted override history"
            )

        activation_rows = _sqlite_rows(
            connection,
            "SELECT * FROM moss_activation_snapshots WHERE activation_id=? AND meeting_id=? AND run_id=?",
            (activation_id, meeting_id, product_run_id),
            "activation snapshot",
        )
        if len(activation_rows) != 1 or activation_rows[0]["status"] != "active":
            raise QualityGateError("The bound MOSS activation is not active in product state")
        activation = activation_rows[0]
        if sha256_bytes(str(activation["pre_activation_transcripts_json"]).encode("utf-8")).lower() != str(
            activation["pre_activation_transcript_sha256"]
        ).lower():
            raise QualityGateError("Pre-activation transcript JSON does not match its persisted hash")
        if sha256_bytes(str(activation["activated_transcripts_json"]).encode("utf-8")).lower() != str(
            activation["activated_transcript_sha256"]
        ).lower():
            raise QualityGateError("Activated transcript JSON does not match its persisted hash")
        try:
            pre_activation_transcripts = json.loads(str(activation["pre_activation_transcripts_json"]))
        except json.JSONDecodeError as exc:
            raise QualityGateError("Pre-activation Whisper snapshot is not valid JSON") from exc
        if not isinstance(pre_activation_transcripts, list) or not pre_activation_transcripts:
            raise QualityGateError("Pre-activation Whisper snapshot is empty")
        whisper_doc = current_documents["whisper_same_window"]
        whisper_segments = require_list(whisper_doc.get("segments"), "whisper_same_window segments")
        expected_whisper = []
        for row in pre_activation_transcripts:
            item = require_mapping(row, "pre-activation Whisper transcript")
            expected_whisper.append(
                {
                    "start_seconds": item.get("audio_start_time"),
                    "end_seconds": item.get("audio_end_time"),
                    "speaker_id": item.get("speaker") or "",
                    "text": item.get("transcript"),
                }
            )
        if whisper_segments != expected_whisper:
            raise QualityGateError("Whisper artifact is not the persisted pre-activation transcript snapshot")
        activation_segments = _sqlite_rows(
            connection,
            "SELECT * FROM moss_activation_segments WHERE activation_id=? ORDER BY segment_index",
            (activation_id,),
            "activation segments",
        )
        if len(activation_segments) != len(materialized):
            raise QualityGateError("Activation snapshot does not contain every materialized segment")
        for expected, actual in zip(materialized, activation_segments):
            if (
                int(actual["start_ms"]) / 1000.0 != expected["start_seconds"]
                or int(actual["end_ms"]) / 1000.0 != expected["end_seconds"]
                or str(actual["text"]) != expected["text"]
                or str(actual["candidate_segment_id"]) != expected["candidate_segment_id"]
                or str(actual["resolved_person_id"] or actual["speaker_label"]) != expected["speaker_id"]
            ):
                raise QualityGateError("Activation snapshot is not the replayed corrected transcript")
        if str(activation["candidate_sha256"]) != str(run["candidate_sha256"]):
            raise QualityGateError("Activation candidate hash is not the persisted MOSS candidate")

        summary_rows = _sqlite_rows(
            connection,
            "SELECT * FROM summary_generation_history WHERE generation_id=? AND meeting_id=?",
            (summary_id, meeting_id),
            "summary history",
        )
        if len(summary_rows) != 1:
            raise QualityGateError("Product database lacks the bound summary generation")
        summary = summary_rows[0]
        if (
            summary["status"] != "completed"
            or summary["transcript_source"] != "moss"
            or summary["moss_run_id"] != product_run_id
            or str(summary["transcript_sha256"]) != str(activation["activated_transcript_sha256"])
            or str(summary["model_name"]).casefold() != str(MODEL_CONTRACTS["qwen_2b"]["model_name"]).casefold()
        ):
            raise QualityGateError("Persisted summary is not completed from the active MOSS transcript with Qwen 2B")
        connection.commit()
    finally:
        try:
            connection.close()
        except UnboundLocalError:
            pass

    if _safe_file_record(database_path) != database_before:
        raise QualityGateError("Immutable product database changed while it was verified")

    activation_doc = current_documents["activation_evidence"]
    if (
        activation_doc.get("activation_id") != activation_id
        or activation_doc.get("meeting_id") != meeting_id
        or activation_doc.get("moss_run_id") != product_run_id
        or activation_doc.get("activated_transcript_sha256") != activation["activated_transcript_sha256"]
    ):
        raise QualityGateError("Activation evidence is not an exact snapshot of product state")
    summary_doc = current_documents["summary_evidence"]
    if (
        summary_doc.get("generation_id") != summary_id
        or summary_doc.get("meeting_id") != meeting_id
        or summary_doc.get("moss_run_id") != product_run_id
        or summary_doc.get("transcript_sha256") != summary["transcript_sha256"]
        or summary_doc.get("model_name") != summary["model_name"]
    ):
        raise QualityGateError("Summary evidence is not an exact snapshot of product state")
    return {
        "database": database_record,
        "materialized_segments": materialized,
        "correction_ids": correction_ids,
        "override_ids": override_ids,
        "activation_bound": True,
        "summary_bound": True,
        "whisper_snapshot_bound": True,
        "product_context_sha256": product_context_sha256,
    }


def _validate_formal_evidence(
    bindings: dict[str, Any], artifact_paths: dict[str, Path], source_commit: str, normalized_human: str
) -> dict[str, Any]:
    if set(bindings["document"]) != {
        "schema_version",
        "stage",
        "formal_short_gate",
        "current_run",
        "source_commit",
        "run_id",
        "started_at",
        "completed_at",
        "window_audio_sha256",
        "artifacts",
        "formal_evidence",
        "bindings_path",
    }:
        raise QualityGateError("Formal run binding fields are not exact")
    formal = require_mapping(bindings["document"].get("formal_evidence"), "formal_evidence")
    if set(formal) != {"session", "execution_receipt", "candidate", "truth", "product_state"}:
        raise QualityGateError("Formal evidence fields are not exact")
    bindings["record_path"] = require_string(
        bindings["document"].get("bindings_path"), "bindings_path"
    )
    if _norm_path(Path(bindings["record_path"])) != _norm_path(Path(bindings["path"])):
        raise QualityGateError("bindings_path is not the live bindings file")
    session = _validate_session(formal, bindings, artifact_paths, source_commit)
    candidate = _validate_candidate(formal, source_commit)
    app_process = session["app_process"]
    expected_app_path = candidate["execution_install_root"] / PROGRAM_CONTRACTS["main_executable"]
    if (
        os.path.normcase(os.path.abspath(require_string(app_process.get("executable_path"), "app executable path")))
        != os.path.normcase(os.path.abspath(expected_app_path))
        or app_process.get("bytes") != candidate["programs"]["main_executable"]["bytes"]
        or str(app_process.get("sha256", "")).upper()
        != candidate["programs"]["main_executable"]["sha256"]
    ):
        raise QualityGateError("Formal app process is not the installed candidate main executable")
    truth = _validate_frozen_truth(formal, artifact_paths, normalized_human)
    expected_program_roles = {
        "moss_raw": "moss_helper",
        "whisper_same_window": "main_executable",
        "corrected": "main_executable",
        "activation_evidence": "main_executable",
        "summary_evidence": "main_executable",
    }
    for artifact_role, program_role in expected_program_roles.items():
        if bindings["artifacts"][artifact_role]["producer"] != {
            "bytes": candidate["programs"][program_role]["bytes"],
            "sha256": candidate["programs"][program_role]["sha256"],
        }:
            raise QualityGateError(f"{artifact_role} is not bound to the required candidate program role")
        if bindings["artifacts"][artifact_role]["exporter"] != session["runner_records"]["formal"]:
            raise QualityGateError(f"{artifact_role} is not exported by the fixed formal Q00 runner")
    return {"formal": formal, "session": session, "candidate": candidate, "truth": truth}


def _gate(actual: Any, threshold: Any, passed: bool) -> dict[str, Any]:
    return {"status": PASS if passed else FAIL, "actual": actual, "threshold": threshold}


def _public_artifact_records(bindings: dict[str, Any]) -> list[dict[str, Any]]:
    records = []
    for role in sorted(bindings["artifacts"]):
        item = bindings["artifacts"][role]
        record = {
                "role": role,
                "origin": item["origin"],
                "source_commit": item["source_commit"],
                "run_id": item["run_id"],
                "bytes": item["bytes"],
                "sha256": item["sha256"],
                "producer": item["producer"],
            }
        if item.get("exporter") is not None:
            record["exporter"] = item["exporter"]
        records.append(record)
    return records


def _public_privacy_scan(payload: dict[str, Any], forbidden_values: Sequence[str]) -> dict[str, Any]:
    encoded = json.dumps(payload, ensure_ascii=False, sort_keys=True)
    hits: list[str] = []
    checked = 0
    for value in forbidden_values:
        if not isinstance(value, str) or not value:
            continue
        checked += 1
        if value in encoded or value.replace("\\", "\\\\") in encoded:
            hits.append(sha256_bytes(value.encode("utf-8")))
    if re.search(r'(?i)(?:[a-z]:\\\\|\\\\\\\\[^"\\]+\\\\)', encoded):
        hits.append("WINDOWS_ABSOLUTE_PATH_PATTERN")
    if hits:
        raise QualityGateError(
            "Public report privacy scan found transcript, term, identity, or real path material"
        )
    return {
        "scanner": "Q00_PUBLIC_REPORT_SERIALIZED_VALUE_SCAN_V1",
        "status": PASS,
        "checked_forbidden_value_count": checked,
        "match_count": 0,
        "contains_transcript_or_term_text": False,
        "contains_real_filesystem_paths": False,
        "contains_reviewer_identity": False,
    }


def _validate_report_roots(
    bindings_path: Path, public_report_path: Path, private_report_path: Path
) -> None:
    document = require_mapping(read_json(bindings_path, "Q00 run bindings"), "Q00 run bindings")
    formal = require_mapping(document.get("formal_evidence"), "formal_evidence")
    session_path, _ = _bound_path_record(formal.get("session"), "formal gate session")
    session = require_mapping(read_json(session_path, "formal gate session"), "formal gate session")
    private_root = Path(require_string(session.get("private_root"), "private_root"))
    public_root = Path(require_string(session.get("public_root"), "public_root"))
    run_directory = Path(require_string(session.get("run_directory"), "run_directory"))
    if _paths_overlap(private_root, public_root):
        raise QualityGateError("Public and private report roots must be disjoint")
    _strict_child(private_report_path, run_directory, "private report")
    _strict_child(public_report_path, public_root, "public report")
    if _same_path(public_report_path, private_report_path):
        raise QualityGateError("Public and private reports must be different files")


def build_score_reports(
    *,
    artifact_paths: dict[str, Path],
    bindings_path: Path,
    source_commit: str,
    scorer_path: Path | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    """Load all evidence, recompute metrics, and return public/private reports."""

    commit = require_git_commit(source_commit, "source_commit")
    formal_mode = scorer_path is None
    bindings = _validate_bindings(bindings_path, artifact_paths, commit)
    window_manifest, window_record = _validate_window_manifest(
        artifact_paths["window_manifest"],
        artifact_paths["window_audio"],
        commit,
        formal=formal_mode,
    )
    if bindings["artifacts"]["window_audio"]["sha256"] != window_record["sha256"]:
        raise QualityGateError("Run bindings and window manifest disagree on the audio hash")
    manifest_producer = require_mapping(window_manifest["producer"], "window manifest producer")
    for role in ("window_audio", "window_manifest"):
        bound_producer = bindings["artifacts"][role]["producer"]
        if (
            bound_producer["bytes"] != manifest_producer.get("bytes")
            or bound_producer["sha256"]
            != str(manifest_producer.get("sha256", "")).upper()
        ):
            raise QualityGateError(
                f"{role} binding does not use the producer recorded by prepare"
            )
    declared_window = str(bindings["document"].get("window_audio_sha256", "")).upper()
    if declared_window != window_record["sha256"]:
        raise QualityGateError("Run bindings window_audio_sha256 is wrong")

    current_documents: dict[str, dict[str, Any]] = {}
    for role in sorted(CURRENT_JSON_ROLES):
        current_documents[role] = _load_current_json(
            artifact_paths[role],
            role=role,
            run_id=bindings["run_id"],
            source_commit=commit,
            window_sha256=window_record["sha256"],
            producer_sha256=bindings["artifacts"][role]["producer"]["sha256"],
            run_started=bindings["started"],
            run_completed=bindings["completed"],
        )

    transcripts = {
        role: _parse_transcript(current_documents[role], role)
        for role in sorted(TRANSCRIPT_ROLES)
    }
    human = _load_human_verbatim(artifact_paths["human_verbatim"])
    speaker_truth = _load_speaker_truth(artifact_paths["speaker_truth"])
    normalized_human_for_truth = _normalize_cer_text(human["text"])
    formal_evidence: dict[str, Any] | None = None
    product_state: dict[str, Any] | None = None
    if formal_mode:
        formal_evidence = _validate_formal_evidence(
            bindings, artifact_paths, commit, normalized_human_for_truth
        )
        for role in CURRENT_JSON_ROLES:
            provenance = require_mapping(current_documents[role].get("provenance"), f"{role} provenance")
            if provenance.get("session_nonce") != formal_evidence["session"]["nonce"]:
                raise QualityGateError(f"{role} is not bound to the gate-created session nonce")
        product_state = _validate_product_state(
            formal_evidence["formal"],
            current_documents,
            formal_evidence["candidate"],
            formal_evidence["session"],
        )
        positive = {"terms": formal_evidence["truth"]["positive"]}
        negative = {"terms": formal_evidence["truth"]["negative"]}
    else:
        positive = _load_term_truth(
            artifact_paths["positive_truth"],
            kind="positive",
            window_sha256=window_record["sha256"],
            human_sha256=bindings["artifacts"]["human_verbatim"]["sha256"],
            speaker_sha256=bindings["artifacts"]["speaker_truth"]["sha256"],
        )
        negative = _load_term_truth(
            artifact_paths["negative_truth"],
            kind="negative",
            window_sha256=window_record["sha256"],
            human_sha256=bindings["artifacts"]["human_verbatim"]["sha256"],
            speaker_sha256=bindings["artifacts"]["speaker_truth"]["sha256"],
        )

    cer_results = {
        role: _cer(human["text"], transcripts[role]["text"])
        for role in ("moss_raw", "whisper_same_window", "corrected")
    }
    corrected_normalized = cer_results["corrected"]["normalized_hypothesis"]
    positive_rows: list[dict[str, Any]] = []
    positive_expected = 0
    positive_credit = 0
    normalized_human = cer_results["moss_raw"]["normalized_reference"]
    for row in positive["terms"]:
        human_actual = _count_occurrences(normalized_human, row["term"])
        if human_actual != row["expected"]:
            raise QualityGateError(
                "Positive truth expected_occurrences does not match the bound human verbatim truth"
            )
        actual = _count_occurrences(corrected_normalized, row["term"])
        expected = row["expected"]
        credit = max(0, expected - abs(actual - expected))
        positive_expected += expected
        positive_credit += credit
        positive_rows.append(
            {**row, "human_actual": human_actual, "actual": actual, "credit": credit}
        )
    positive_accuracy = positive_credit / positive_expected
    negative_rows: list[dict[str, Any]] = []
    negative_insertions = 0
    for row in negative["terms"]:
        if _count_occurrences(normalized_human, row["term"]) != 0:
            raise QualityGateError(
                "Negative truth contains a term that is present in the bound human verbatim truth"
            )
        actual = _count_occurrences(corrected_normalized, row["term"])
        negative_insertions += actual
        negative_rows.append({**row, "actual": actual})

    raw_speaker = _raw_speaker_metrics(transcripts["moss_raw"]["segments"], speaker_truth)
    corrected_speaker = _corrected_speaker_errors(
        transcripts["corrected"]["segments"], speaker_truth
    )
    moss_contract = transcripts["moss_raw"]["inference_contract"]
    whisper_contract = transcripts["whisper_same_window"]["inference_contract"]
    comparison_window_bound = all(
        contract["window_audio_sha256"] == window_record["sha256"]
        and abs(contract["window_duration_seconds"] - WINDOW_DURATION_SECONDS) <= 1e-9
        for contract in (moss_contract, whisper_contract)
    )
    comparison_language_bound = all(
        contract["language_requested"] == FAIR_COMPARISON_LANGUAGE
        and contract["language_resolved"] == FAIR_COMPARISON_LANGUAGE
        and contract["language_resolution_source"]
        == contract["expected_language_resolution_source"]
        for contract in (moss_contract, whisper_contract)
    )
    model_and_decode_bound = (
        moss_contract["model_sha256"] == moss_contract["expected_model_sha256"]
        and moss_contract["decode_parameters_sha256"]
        == moss_contract["calculated_decode_parameters_sha256"]
        and whisper_contract["model_sha256"]
        == whisper_contract["expected_model_sha256"]
        and whisper_contract["decode_parameters"]
        == whisper_contract["expected_decode_parameters"]
        and whisper_contract["decode_parameters_sha256"]
        == whisper_contract["expected_decode_parameters_sha256"]
    )
    speaker_override_true_flip = (
        transcripts["corrected"]["speaker_override_true_flip"]
        and _speaker_override_truth_bound(
            transcripts["corrected"]["manual_speaker_override_evidence"],
            transcripts["corrected"]["segments"],
            speaker_truth,
        )
    )
    moss_elapsed = (
        formal_evidence["session"]["moss_elapsed_seconds"]
        if formal_evidence is not None
        else transcripts["moss_raw"]["inference_seconds"]
    )
    rtf = moss_elapsed / WINDOW_DURATION_SECONDS
    timestamps_valid = all(
        transcripts[role]["timestamp_valid"] for role in TRANSCRIPT_ROLES
    )

    if product_state is not None:
        activation_binding = bool(product_state["activation_bound"])
        summary_binding = bool(product_state["summary_bound"])
    else:
        activation = current_documents["activation_evidence"]
        activation_binding = (
            activation.get("status") == "ACTIVATED"
            and activation.get("candidate_source") == "MOSS"
            and activation.get("activation_kind") == "HUMAN_CONFIRMED"
            and str(activation.get("raw_candidate_sha256", "")).upper()
            == bindings["artifacts"]["moss_raw"]["sha256"]
            and str(activation.get("candidate_transcript_sha256", "")).upper()
            == bindings["artifacts"]["corrected"]["sha256"]
            and str(activation.get("active_transcript_sha256", "")).upper()
            == bindings["artifacts"]["corrected"]["sha256"]
        )
        summary = current_documents["summary_evidence"]
        summary_binding = (
            summary.get("status") == "COMPLETED"
            and summary.get("source_kind") == "ACTIVE_MOSS_TRANSCRIPT"
            and summary.get("model_family") == "QWEN_2B"
            and str(summary.get("source_transcript_sha256", "")).upper()
            == bindings["artifacts"]["corrected"]["sha256"]
            and str(summary.get("activation_evidence_sha256", "")).upper()
            == bindings["artifacts"]["activation_evidence"]["sha256"]
        )

    gates = {
        "moss_raw_cer_at_most_20_percent": _gate(
            cer_results["moss_raw"]["cer"], 0.20, cer_results["moss_raw"]["cer"] <= 0.20
        ),
        "moss_raw_not_worse_than_whisper": _gate(
            {
                "moss_raw_cer": cer_results["moss_raw"]["cer"],
                "whisper_same_window_cer": cer_results["whisper_same_window"]["cer"],
            },
            "moss_raw_cer <= whisper_same_window_cer",
            cer_results["moss_raw"]["cer"] <= cer_results["whisper_same_window"]["cer"],
        ),
        "corrected_cer_at_most_15_percent": _gate(
            cer_results["corrected"]["cer"],
            0.15,
            cer_results["corrected"]["cer"] <= 0.15,
        ),
        "positive_occurrence_accuracy_at_least_95_percent": _gate(
            positive_accuracy, 0.95, positive_accuracy >= 0.95
        ),
        "negative_term_insertions_zero": _gate(
            negative_insertions, 0, negative_insertions == 0
        ),
        "raw_speaker_measurement_complete": _gate(
            raw_speaker["measurement_complete"], True, raw_speaker["measurement_complete"]
        ),
        "raw_speaker_segment_error_rate_at_most_10_percent": _gate(
            raw_speaker["segment_error_rate"],
            0.10,
            raw_speaker["measurement_complete"]
            and raw_speaker["segment_error_rate"] <= 0.10,
        ),
        "raw_speaker_duration_error_rate_at_most_10_percent": _gate(
            raw_speaker["duration_error_rate"],
            0.10,
            raw_speaker["measurement_complete"]
            and raw_speaker["duration_error_rate"] <= 0.10,
        ),
        "speaker_override_true_flip": _gate(
            speaker_override_true_flip, True, speaker_override_true_flip
        ),
        "manual_single_segment_coverage_errors_zero": _gate(
            corrected_speaker["error_count"], 0, corrected_speaker["error_count"] == 0
        ),
        "whisper_moss_same_226440ms_window": _gate(
            comparison_window_bound,
            True,
            comparison_window_bound,
        ),
        "whisper_and_moss_language_resolved_zh_cn": _gate(
            {
                "whisper": whisper_contract["language_resolved"],
                "moss": moss_contract["language_resolved"],
            },
            {"whisper": FAIR_COMPARISON_LANGUAGE, "moss": FAIR_COMPARISON_LANGUAGE},
            comparison_language_bound,
        ),
        "transcription_models_and_decode_parameters_bound": _gate(
            model_and_decode_bound,
            True,
            model_and_decode_bound,
        ),
        "moss_rtf_at_most_one": _gate(rtf, 1.0, rtf <= 1.0),
        "all_transcript_timestamps_valid": _gate(
            timestamps_valid, True, timestamps_valid
        ),
        "candidate_activation_bound": _gate(
            activation_binding, True, activation_binding
        ),
        "summary_source_bound": _gate(summary_binding, True, summary_binding),
    }
    status = PASS if all(item["status"] == PASS for item in gates.values()) else FAIL
    failed_gates = sorted(name for name, item in gates.items() if item["status"] != PASS)
    scorer = (scorer_path or Path(__file__)).resolve(strict=True)
    scorer_record = _safe_file_record(scorer)

    public_metrics = {
        "moss_raw_cer": cer_results["moss_raw"]["cer"],
        "whisper_same_window_cer": cer_results["whisper_same_window"]["cer"],
        "corrected_cer": cer_results["corrected"]["cer"],
        "reference_character_count": cer_results["moss_raw"]["reference_characters"],
        "positive_expected_occurrences": positive_expected,
        "positive_credited_occurrences": positive_credit,
        "positive_occurrence_accuracy": positive_accuracy,
        "negative_term_insertions": negative_insertions,
        "raw_speaker_segment_error_count": raw_speaker["segment_error_count"],
        "raw_speaker_scored_segment_count": raw_speaker["scored_segment_count"],
        "raw_speaker_segment_error_rate": raw_speaker["segment_error_rate"],
        "raw_speaker_duration_error_seconds": raw_speaker["duration_error_seconds"],
        "raw_speaker_scored_duration_seconds": raw_speaker["scored_duration_seconds"],
        "raw_speaker_duration_error_rate": raw_speaker["duration_error_rate"],
        "raw_speaker_false_alarm_seconds": raw_speaker["false_alarm_seconds"],
        "manual_single_segment_coverage_error_count": corrected_speaker["error_count"],
        "manual_speaker_override_count": transcripts["corrected"]["manual_speaker_override_count"],
        "speaker_override_true_flip": speaker_override_true_flip,
        "comparison_window_audio_sha256": window_record["sha256"],
        "comparison_window_duration_seconds": WINDOW_DURATION_SECONDS,
        "whisper_language_requested": whisper_contract["language_requested"],
        "whisper_language_resolved": whisper_contract["language_resolved"],
        "moss_language_requested": moss_contract["language_requested"],
        "moss_language_resolved": moss_contract["language_resolved"],
        "moss_language_resolution_source": moss_contract["language_resolution_source"],
        "whisper_model_sha256": whisper_contract["model_sha256"],
        "moss_model_sha256": moss_contract["model_sha256"],
        "whisper_decode_parameters_sha256": whisper_contract[
            "decode_parameters_sha256"
        ],
        "moss_decode_parameters_json": moss_contract["decode_parameters_json"],
        "moss_decode_parameters_sha256": moss_contract["decode_parameters_sha256"],
        "moss_inference_seconds": moss_elapsed,
        "moss_rtf": rtf,
        "timestamp_invalid_item_count": sum(
            len(transcripts[role]["invalid_reasons"]) for role in TRANSCRIPT_ROLES
        ),
        "timestamps_valid": timestamps_valid,
        "candidate_activation_bound": activation_binding,
        "summary_source_bound": summary_binding,
    }
    private_metrics = {
        **public_metrics,
        "cer_diagnostics": cer_results,
        "positive_occurrences": positive_rows,
        "negative_occurrences": negative_rows,
        "raw_speaker": raw_speaker,
        "corrected_speaker": corrected_speaker,
        "manual_speaker_override_evidence": transcripts["corrected"][
            "manual_speaker_override_evidence"
        ],
        "transcription_inference_contracts": {
            "moss": moss_contract,
            "whisper": whisper_contract,
        },
        "timestamp_diagnostics": {
            role: transcripts[role]["invalid_reasons"] for role in TRANSCRIPT_ROLES
        },
    }
    public_artifacts = _public_artifact_records(bindings)
    input_manifest_sha256 = sha256_bytes(canonical_json_bytes(public_artifacts))
    generated_at = bindings["document"]["completed_at"]
    private = _with_integrity(
        {
            "schema_version": SCHEMA_VERSION,
            "stage": PRIVATE_REPORT_STAGE,
            "status": status,
            "generated_at": generated_at,
            "source_commit": commit,
            "run_id": bindings["run_id"],
            "scorer": scorer_record,
            "bindings": {
                **bindings["record"],
                "path": str(bindings_path.resolve(strict=True)),
            },
            "inputs": [
                {
                    **item,
                    "path": str(artifact_paths[item["role"]].resolve(strict=True)),
                    "producer_path": bindings["artifacts"][item["role"]]["producer_path"],
                }
                for item in public_artifacts
            ],
            "window_manifest": window_manifest,
            "metrics": private_metrics,
            "gates": gates,
            "failed_gates": failed_gates,
        }
    )
    private_bytes = _json_file_bytes(private)
    private_record = {
        "bytes": len(private_bytes),
        "sha256": sha256_bytes(private_bytes),
    }
    public_payload = {
            "schema_version": SCHEMA_VERSION,
            "stage": REPORT_STAGE,
            "status": status,
            "generated_at": generated_at,
            "source_commit": commit,
            "run_id": bindings["run_id"],
            "scope": {
                "source_audio_sha256": SOURCE_SHA256,
                "window_audio_sha256": window_record["sha256"],
                "source_start_seconds": WINDOW_START_SECONDS,
                "source_end_seconds": WINDOW_END_SECONDS,
                "duration_seconds": WINDOW_DURATION_SECONDS,
            },
            "cer_rule": {
                "unicode_normalization": "NFKC",
                "case": "UNICODE_CASEFOLD",
                "retained_categories": ["LETTER", "NUMBER"],
                "unit": "ONE_RETAINED_UNICODE_CODE_POINT",
                "formula": "LEVENSHTEIN_DISTANCE / REFERENCE_CODE_POINT_COUNT",
            },
            "positive_occurrence_rule": (
                "sum(max(0, expected-abs(actual-expected))) / sum(expected)"
            ),
            "scorer": scorer_record,
            "run_bindings": bindings["record"],
            "input_artifacts": public_artifacts,
            "input_manifest_sha256": input_manifest_sha256,
            "metrics": public_metrics,
            "gates": gates,
            "failed_gates": failed_gates,
            "private_report": private_record,
            "truth_rule": (
                "PASS requires complete hash-bound human verbatim, speaker, positive-term, "
                "and negative-term truth; missing truth cannot become N/A or PASS."
            ),
        }
    forbidden_values = [
        str(bindings_path.resolve(strict=True)),
        *(str(path.resolve(strict=True)) for path in artifact_paths.values()),
        *(item["producer_path"] for item in bindings["artifacts"].values()),
        *(row["text"] for row in human["rows"]),
        *(row["text"] for role in transcripts.values() for row in role["segments"]),
        *(row["term"] for row in positive["terms"]),
        *(row["term"] for row in negative["terms"]),
    ]
    if formal_evidence is not None:
        forbidden_values.append(str(formal_evidence["truth"]["review"].get("reviewer_id", "")))
        forbidden_values.extend(
            str(path)
            for group in (formal_evidence["candidate"]["programs"], formal_evidence["candidate"]["models"])
            for path in (group[role]["path"] for role in group)
        )
        forbidden_values.extend(
            str(formal_evidence["session"][name])
            for name in ("private_root", "public_root", "run_directory")
        )
    public_payload["privacy"] = _public_privacy_scan(public_payload, forbidden_values)
    public = _with_integrity(public_payload)
    return public, private


def score_files(
    *,
    artifact_paths: dict[str, Path],
    bindings_path: Path,
    public_report_path: Path,
    private_report_path: Path,
    source_commit: str,
    scorer_path: Path | None = None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if scorer_path is None:
        _validate_report_roots(bindings_path, public_report_path, private_report_path)
    _require_new_outputs((public_report_path, private_report_path))
    public, private = build_score_reports(
        artifact_paths=artifact_paths,
        bindings_path=bindings_path,
        source_commit=source_commit,
        scorer_path=scorer_path,
    )
    _write_json_exclusive(private_report_path, private)
    actual_private = _safe_file_record(private_report_path)
    if actual_private != public["private_report"]:
        private_report_path.unlink()
        raise QualityGateError("Private report serialization did not match its public hash binding")
    _write_json_exclusive(public_report_path, public)
    return public, private


def verify_files(
    *,
    artifact_paths: dict[str, Path],
    bindings_path: Path,
    public_report_path: Path,
    private_report_path: Path,
    source_commit: str,
    scorer_path: Path | None = None,
) -> str:
    if scorer_path is None:
        _validate_report_roots(bindings_path, public_report_path, private_report_path)
    actual_public = require_mapping(
        read_json(public_report_path, "public Q00 report"), "public Q00 report"
    )
    actual_private = require_mapping(
        read_json(private_report_path, "private Q00 report"), "private Q00 report"
    )
    expected_public, expected_private = build_score_reports(
        artifact_paths=artifact_paths,
        bindings_path=bindings_path,
        source_commit=source_commit,
        scorer_path=scorer_path,
    )
    if actual_private != expected_private:
        raise QualityGateError("Private report does not equal the recomputed report")
    private_record = _safe_file_record(private_report_path)
    if private_record != expected_public["private_report"]:
        raise QualityGateError("Private report bytes or SHA-256 no longer match the public report")
    if actual_public != expected_public:
        raise QualityGateError("Public report does not equal the recomputed report")
    if actual_public.get("status") == PASS and any(
        item.get("status") != PASS
        for item in require_mapping(actual_public.get("gates"), "public report gates").values()
        if isinstance(item, dict)
    ):
        raise QualityGateError("A public PASS contains a failing hard gate")
    return require_string(actual_public.get("status"), "public report status")


def _artifact_paths_from_args(args: argparse.Namespace) -> dict[str, Path]:
    return {
        "window_audio": args.window_audio,
        "window_manifest": args.window_manifest,
        "moss_raw": args.moss_raw,
        "whisper_same_window": args.whisper_same_window,
        "corrected": args.corrected,
        "human_verbatim": args.human_verbatim,
        "speaker_truth": args.speaker_truth,
        "positive_truth": args.positive_truth,
        "negative_truth": args.negative_truth,
        "activation_evidence": args.activation_evidence,
        "summary_evidence": args.summary_evidence,
    }


def _add_score_inputs(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--bindings", type=Path, required=True)
    parser.add_argument("--window-audio", type=Path, required=True)
    parser.add_argument("--window-manifest", type=Path, required=True)
    parser.add_argument("--moss-raw", type=Path, required=True)
    parser.add_argument("--whisper-same-window", type=Path, required=True)
    parser.add_argument("--corrected", type=Path, required=True)
    parser.add_argument("--human-verbatim", type=Path, required=True)
    parser.add_argument("--speaker-truth", type=Path, required=True)
    parser.add_argument("--positive-truth", type=Path, required=True)
    parser.add_argument("--negative-truth", type=Path, required=True)
    parser.add_argument("--activation-evidence", type=Path, required=True)
    parser.add_argument("--summary-evidence", type=Path, required=True)
    parser.add_argument("--public-report", type=Path, required=True)
    parser.add_argument("--private-report", type=Path, required=True)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    prepare = subparsers.add_parser("prepare", help="extract the fixed 226.440-second PCM window")
    prepare.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    prepare.add_argument("--source-commit", required=True)
    prepare.add_argument("--source-wav", type=Path, required=True)
    prepare.add_argument("--output-wav", type=Path, required=True)
    prepare.add_argument("--manifest", type=Path, required=True)

    score = subparsers.add_parser("score", help="compute the hard gate from real bound artifacts")
    _add_score_inputs(score)

    verify = subparsers.add_parser("verify", help="recompute and verify reports and artifact hashes")
    _add_score_inputs(verify)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        commit = _require_current_commit(args.repo, args.source_commit)
        if args.command == "prepare":
            document = prepare_window(
                source_wav=args.source_wav,
                output_wav=args.output_wav,
                manifest_path=args.manifest,
                source_commit=commit,
            )
            print(
                json.dumps(
                    {
                        "status": document["status"],
                        "window_sha256": document["output_audio"]["sha256"],
                        "manifest": str(args.manifest),
                    },
                    ensure_ascii=False,
                )
            )
            return 0
        artifacts = _artifact_paths_from_args(args)
        if args.command == "score":
            public, _ = score_files(
                artifact_paths=artifacts,
                bindings_path=args.bindings,
                public_report_path=args.public_report,
                private_report_path=args.private_report,
                source_commit=commit,
            )
            print(
                json.dumps(
                    {
                        "status": public["status"],
                        "failed_gates": public["failed_gates"],
                        "public_report": str(args.public_report),
                    },
                    ensure_ascii=False,
                )
            )
            return 0 if public["status"] == PASS else 1
        status = verify_files(
            artifact_paths=artifacts,
            bindings_path=args.bindings,
            public_report_path=args.public_report,
            private_report_path=args.private_report,
            source_commit=commit,
        )
        print(json.dumps({"verified": True, "status": status}, ensure_ascii=False))
        return 0 if status == PASS else 1
    except (GateError, OSError, ValueError) as exc:
        print(json.dumps({"status": FAIL, "error": str(exc)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
