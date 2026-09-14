#!/usr/bin/env python3
"""External supervisor for controlled MOSS v3 P1 runs.

This module does not perform MOSS inference and does not turn a chunk plan into
an inference PASS.  It provides the process boundary around the frozen P1
runner: deterministic memory admission, a non-executing chunk plan, process
tree monitoring, cancellation/timeout/low-memory termination, residual-process
checks, and privacy-safe public evidence.

The frozen long sample is intentionally rejected as one native invocation.
At 3,096 seconds the conservative KV estimate is about 12.69 GiB before model,
runtime, and safety reserves are included.
"""

from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import dataclasses
import hashlib
import json
import math
import ntpath
import os
import re
import signal
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

SAFE_SITE_PACKAGES = (
    Path(__file__).resolve().parents[2]
    / ".tools"
    / "moss-poc"
    / ".venv"
    / "Lib"
    / "site-packages"
)
PYTHON_CACHE_ROOT = Path(r"D:\MeetilyData\staging\moss-p1\empty-pycache").resolve()
TASKKILL_PATH = Path(r"C:\Windows\System32\taskkill.exe")
EXPECTED_TASKKILL_SHA256 = "1249717315FC8F4D2DF17D5DB9DA0444795FDB9FB83DFB1F763C3F39282244F7"
_psutil_entries = []
for _item in (SAFE_SITE_PACKAGES / "psutil").iterdir():
    if _item.is_file():
        _digest = hashlib.sha256(_item.read_bytes()).hexdigest().upper()
        _psutil_entries.append((_item.name, _item.stat().st_size, _digest))
_psutil_entries.sort()
_psutil_manifest = hashlib.sha256()
for _name, _size, _digest in _psutil_entries:
    _psutil_manifest.update(f"{_name}\0{_size}\0{_digest}\n".encode("utf-8"))
if (
    len(_psutil_entries) != 10
    or _psutil_manifest.hexdigest().upper()
    != "8E349CBB107CBA6C599D0A233957D21F91B9319FF601C00D6CD8FCBC8689C077"
):
    raise RuntimeError("frozen psutil package manifest mismatch before import")
if str(SAFE_SITE_PACKAGES) not in sys.path:
    sys.path.insert(0, str(SAFE_SITE_PACKAGES))

try:
    import psutil  # type: ignore
except ImportError:  # pragma: no cover - the frozen Windows P1 env includes psutil
    psutil = None


GIB = 1024**3

# Conservative frozen-lane estimate.  It intentionally rounds upward from the
# observed/context-growth envelope instead of claiming byte-exact native use.
KV_BYTES_PER_AUDIO_SECOND = 4_400_000
MODEL_RESIDENT_BYTES = 986_899_616
RUNTIME_WORKING_SET_RESERVE_BYTES = 1 * GIB
SAFETY_MARGIN_BYTES = 2 * GIB

# The single-process P1 lane has not been qualified above this memory budget.
# This makes the 3,096 s monolith a deterministic rejection even on a machine
# with unusually high free memory; chunk execution still needs separate proof.
VALIDATED_SINGLE_PROCESS_BUDGET_BYTES = 12 * GIB
DEFAULT_MIN_AVAILABLE_BYTES = 2 * GIB
DEFAULT_CHUNK_SECONDS = 600.0
DEFAULT_OVERLAP_SECONDS = 1.0
DEFAULT_POLL_SECONDS = 0.10

FULL_PATH_RE = re.compile(r"(?i)(?:[a-z]:[\\/]|\\\\[^\\]+\\|^/[^/])")
CONTENT_KEYS = {
    "body",
    "command",
    "message",
    "raw_text",
    "stderr",
    "stdout",
    "text",
    "traceback",
    "transcript",
}


class SupervisorError(RuntimeError):
    """Controlled supervisor failure."""


if os.name == "nt":
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000
    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS = 9

    class IO_COUNTERS(ctypes.Structure):
        _fields_ = [
            ("ReadOperationCount", ctypes.c_uint64),
            ("WriteOperationCount", ctypes.c_uint64),
            ("OtherOperationCount", ctypes.c_uint64),
            ("ReadTransferCount", ctypes.c_uint64),
            ("WriteTransferCount", ctypes.c_uint64),
            ("OtherTransferCount", ctypes.c_uint64),
        ]

    class JOBOBJECT_BASIC_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_int64),
            ("PerJobUserTimeLimit", ctypes.c_int64),
            ("LimitFlags", wintypes.DWORD),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", wintypes.DWORD),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", wintypes.DWORD),
            ("SchedulingClass", wintypes.DWORD),
        ]

    class JOBOBJECT_EXTENDED_LIMIT_INFORMATION(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", JOBOBJECT_BASIC_LIMIT_INFORMATION),
            ("IoInfo", IO_COUNTERS),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]


class KillOnCloseJob:
    """Own one Windows Job Object that kills its process tree on handle close.

    Reactive cleanup is not enough when the supervisor itself is terminated.
    Windows closes this process-owned handle during a hard exit; the kernel then
    terminates every process that inherited membership in the job.
    """

    def __init__(self) -> None:
        self._handle: int | None = None
        if os.name != "nt":
            return
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [
            wintypes.HANDLE,
            ctypes.c_int,
            ctypes.c_void_p,
            wintypes.DWORD,
        ]
        kernel32.SetInformationJobObject.restype = wintypes.BOOL
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.CloseHandle.restype = wintypes.BOOL
        self._kernel32 = kernel32

        handle = kernel32.CreateJobObjectW(None, None)
        if not handle:
            raise SupervisorError(
                f"CreateJobObjectW failed with Win32 error {ctypes.get_last_error()}"
            )
        self._handle = int(handle)
        limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION()
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not kernel32.SetInformationJobObject(
            wintypes.HANDLE(self._handle),
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
            ctypes.byref(limits),
            ctypes.sizeof(limits),
        ):
            error = ctypes.get_last_error()
            self.close()
            raise SupervisorError(
                f"SetInformationJobObject failed with Win32 error {error}"
            )

    def assign(self, process: subprocess.Popen[Any]) -> None:
        if os.name != "nt":
            return
        if self._handle is None:
            raise SupervisorError("kill-on-close job is already closed")
        process_handle = getattr(process, "_handle", None)
        if process_handle is None:
            raise SupervisorError("child process handle is unavailable")
        if not self._kernel32.AssignProcessToJobObject(
            wintypes.HANDLE(self._handle), wintypes.HANDLE(int(process_handle))
        ):
            raise SupervisorError(
                "AssignProcessToJobObject failed with Win32 error "
                f"{ctypes.get_last_error()}"
            )

    def close(self) -> None:
        if os.name == "nt" and self._handle is not None:
            self._kernel32.CloseHandle(wintypes.HANDLE(self._handle))
            self._handle = None

    def __enter__(self) -> "KillOnCloseJob":
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        self.close()


@dataclasses.dataclass(frozen=True)
class MemoryEstimate:
    duration_seconds: float
    kv_bytes: int
    model_resident_bytes: int
    runtime_reserve_bytes: int
    safety_margin_bytes: int
    required_bytes: int

    def public_record(self) -> dict[str, Any]:
        return {
            "duration_seconds": round(self.duration_seconds, 6),
            "kv_bytes": self.kv_bytes,
            "kv_gib": round(self.kv_bytes / GIB, 6),
            "model_resident_bytes": self.model_resident_bytes,
            "runtime_reserve_bytes": self.runtime_reserve_bytes,
            "safety_margin_bytes": self.safety_margin_bytes,
            "required_bytes": self.required_bytes,
            "required_gib": round(self.required_bytes / GIB, 6),
            "estimate_kind": "conservative_not_byte_exact",
        }


@dataclasses.dataclass(frozen=True)
class MemoryAdmission:
    admitted: bool
    reason_code: str
    reasons: tuple[str, ...]
    available_bytes: int
    effective_budget_bytes: int
    estimate: MemoryEstimate

    def public_record(self) -> dict[str, Any]:
        return {
            "admitted": self.admitted,
            "reason_code": self.reason_code,
            "reasons": list(self.reasons),
            "available_bytes": self.available_bytes,
            "available_gib": round(self.available_bytes / GIB, 6),
            "validated_single_process_budget_bytes": VALIDATED_SINGLE_PROCESS_BUDGET_BYTES,
            "effective_budget_bytes": self.effective_budget_bytes,
            "effective_budget_gib": round(self.effective_budget_bytes / GIB, 6),
            "estimate": self.estimate.public_record(),
        }


@dataclasses.dataclass(frozen=True)
class ProcessIdentity:
    pid: int
    create_time: float | None


@dataclasses.dataclass(frozen=True)
class SupervisionResult:
    termination_reason: str
    child_exit_code: int | None
    elapsed_seconds: float
    peak_tree_rss_bytes: int
    minimum_available_memory_bytes: int
    observed_process_count: int
    residual_processes_before_cleanup: tuple[int, ...]
    residual_processes_after_cleanup: tuple[int, ...]

    def public_record(self) -> dict[str, Any]:
        return {
            "termination_reason": self.termination_reason,
            "child_exit_code": self.child_exit_code,
            "elapsed_seconds": round(self.elapsed_seconds, 6),
            "peak_tree_rss_bytes": self.peak_tree_rss_bytes,
            "peak_tree_rss_gib": round(self.peak_tree_rss_bytes / GIB, 6),
            "minimum_available_memory_bytes": self.minimum_available_memory_bytes,
            "minimum_available_memory_gib": round(
                self.minimum_available_memory_bytes / GIB, 6
            ),
            "observed_process_count": self.observed_process_count,
            "residual_processes_before_cleanup": list(
                self.residual_processes_before_cleanup
            ),
            "residual_processes_after_cleanup": list(
                self.residual_processes_after_cleanup
            ),
            "residual_process_check": (
                "PASS"
                if not self.residual_processes_after_cleanup
                else "FAIL"
            ),
        }


def now_iso() -> str:
    return datetime.now(timezone.utc).astimezone().isoformat()


def sha256_file(path: Path, chunk_size: int = 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(chunk_size):
            digest.update(chunk)
    return digest.hexdigest().upper()


def _require_duration(value: float, label: str = "duration") -> float:
    if not math.isfinite(value) or value <= 0:
        raise SupervisorError(f"{label} must be a finite positive number")
    return float(value)


def estimate_memory(duration_seconds: float) -> MemoryEstimate:
    """Return the conservative single-invocation memory estimate."""

    duration = _require_duration(duration_seconds)
    kv_bytes = math.ceil(duration * KV_BYTES_PER_AUDIO_SECOND)
    required = (
        kv_bytes
        + MODEL_RESIDENT_BYTES
        + RUNTIME_WORKING_SET_RESERVE_BYTES
        + SAFETY_MARGIN_BYTES
    )
    return MemoryEstimate(
        duration_seconds=duration,
        kv_bytes=kv_bytes,
        model_resident_bytes=MODEL_RESIDENT_BYTES,
        runtime_reserve_bytes=RUNTIME_WORKING_SET_RESERVE_BYTES,
        safety_margin_bytes=SAFETY_MARGIN_BYTES,
        required_bytes=required,
    )


def assess_memory_admission(
    duration_seconds: float,
    available_memory_bytes: int,
) -> MemoryAdmission:
    """Gate one native invocation against live and qualified budgets."""

    if available_memory_bytes < 0:
        raise SupervisorError("available memory must not be negative")
    estimate = estimate_memory(duration_seconds)
    effective_budget = min(
        int(available_memory_bytes), VALIDATED_SINGLE_PROCESS_BUDGET_BYTES
    )
    reasons: list[str] = []
    if estimate.required_bytes > int(available_memory_bytes):
        reasons.append("required_with_safety_exceeds_available_memory")
    if estimate.required_bytes > VALIDATED_SINGLE_PROCESS_BUDGET_BYTES:
        reasons.append("required_with_safety_exceeds_validated_single_process_budget")
    admitted = not reasons
    return MemoryAdmission(
        admitted=admitted,
        reason_code=(
            "P1_MEMORY_ADMISSION_ACCEPTED"
            if admitted
            else "P1_MONOLITHIC_MEMORY_REJECTED"
        ),
        reasons=tuple(reasons),
        available_bytes=int(available_memory_bytes),
        effective_budget_bytes=effective_budget,
        estimate=estimate,
    )


def build_chunk_plan(
    duration_seconds: float,
    chunk_seconds: float = DEFAULT_CHUNK_SECONDS,
    overlap_seconds: float = DEFAULT_OVERLAP_SECONDS,
) -> dict[str, Any]:
    """Describe chunks without executing or claiming successful inference."""

    duration = _require_duration(duration_seconds)
    chunk = _require_duration(chunk_seconds, "chunk duration")
    if not math.isfinite(overlap_seconds) or overlap_seconds < 0:
        raise SupervisorError("overlap must be finite and non-negative")
    overlap = float(overlap_seconds)
    if overlap >= chunk:
        raise SupervisorError("overlap must be smaller than chunk duration")

    chunks: list[dict[str, Any]] = []
    start = 0.0
    index = 0
    epsilon = 1e-9
    while start < duration - epsilon:
        end = min(start + chunk, duration)
        actual_duration = end - start
        chunks.append(
            {
                "index": index,
                "start_seconds": round(start, 6),
                "end_seconds": round(end, 6),
                "duration_seconds": round(actual_duration, 6),
                "overlap_before_seconds": (
                    0.0 if index == 0 else round(overlap, 6)
                ),
                "memory_estimate": estimate_memory(actual_duration).public_record(),
                "execution_status": "NOT_RUN",
                "inference_verdict": "NOT_EVALUATED",
            }
        )
        if end >= duration - epsilon:
            break
        start = end - overlap
        index += 1

    return {
        "chunk_seconds": round(chunk, 6),
        "overlap_seconds": round(overlap, 6),
        "chunk_count": len(chunks),
        "source_duration_seconds": round(duration, 6),
        "execution_status": "NOT_RUN",
        "inference_verdict": "NOT_EVALUATED",
        "plan_is_not_inference_evidence": True,
        "chunks": chunks,
    }


class _MemoryStatusEx(ctypes.Structure):
    _fields_ = [
        ("dwLength", ctypes.c_ulong),
        ("dwMemoryLoad", ctypes.c_ulong),
        ("ullTotalPhys", ctypes.c_ulonglong),
        ("ullAvailPhys", ctypes.c_ulonglong),
        ("ullTotalPageFile", ctypes.c_ulonglong),
        ("ullAvailPageFile", ctypes.c_ulonglong),
        ("ullTotalVirtual", ctypes.c_ulonglong),
        ("ullAvailVirtual", ctypes.c_ulonglong),
        ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
    ]


def get_available_memory_bytes() -> int:
    """Read available physical memory without starting another process."""

    if psutil is not None:
        return int(psutil.virtual_memory().available)
    if os.name == "nt":  # pragma: no cover - frozen P1 has psutil
        status = _MemoryStatusEx()
        status.dwLength = ctypes.sizeof(_MemoryStatusEx)
        if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            raise SupervisorError("GlobalMemoryStatusEx failed")
        return int(status.ullAvailPhys)
    if Path("/proc/meminfo").is_file():  # pragma: no cover - portability fallback
        for line in Path("/proc/meminfo").read_text(encoding="ascii").splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    raise SupervisorError("available physical memory could not be measured")


def _process_identity(process: Any) -> ProcessIdentity:
    try:
        created = float(process.create_time())
    except Exception:
        created = None
    return ProcessIdentity(pid=int(process.pid), create_time=created)


def discover_process_tree(root_pid: int) -> tuple[ProcessIdentity, ...]:
    """Snapshot the root and all recursive descendants known to psutil."""

    if psutil is None:
        return (ProcessIdentity(root_pid, None),)
    try:
        root = psutil.Process(root_pid)
        processes = [root, *root.children(recursive=True)]
    except (psutil.NoSuchProcess, psutil.ZombieProcess):
        return ()
    identities = {_process_identity(process) for process in processes}
    return tuple(sorted(identities, key=lambda item: item.pid))


def _identity_is_alive(identity: ProcessIdentity) -> bool:
    if psutil is None:
        try:
            os.kill(identity.pid, 0)
            return True
        except OSError:
            return False
    try:
        process = psutil.Process(identity.pid)
        if identity.create_time is not None:
            if abs(float(process.create_time()) - identity.create_time) > 0.01:
                return False
        return process.is_running() and process.status() != psutil.STATUS_ZOMBIE
    except (psutil.NoSuchProcess, psutil.ZombieProcess, psutil.AccessDenied):
        return False


def check_residual_processes(
    identities: Iterable[ProcessIdentity],
) -> tuple[int, ...]:
    """Return only identities that still name the same live process."""

    return tuple(
        sorted({identity.pid for identity in identities if _identity_is_alive(identity)})
    )


def _terminate_known_identities(identities: Iterable[ProcessIdentity]) -> None:
    if psutil is None:
        return
    processes = []
    for identity in identities:
        if not _identity_is_alive(identity):
            continue
        try:
            processes.append(psutil.Process(identity.pid))
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    # Children normally have greater depth/PID; terminating in reverse order
    # keeps parents from immediately replacing a worker during cleanup.
    for process in sorted(processes, key=lambda item: item.pid, reverse=True):
        try:
            process.terminate()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    _, alive = psutil.wait_procs(processes, timeout=1.0)
    for process in alive:
        try:
            process.kill()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            pass
    if alive:
        psutil.wait_procs(alive, timeout=2.0)


def terminate_process_tree(
    root_pid: int,
    known_identities: Iterable[ProcessIdentity] = (),
) -> None:
    """Terminate a Windows process tree, then clean any observed survivors."""

    identities = set(known_identities)
    identities.update(discover_process_tree(root_pid))
    if os.name == "nt":
        try:
            subprocess.run(
                [str(TASKKILL_PATH), "/PID", str(root_pid), "/T", "/F"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=10,
                check=False,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
    else:  # pragma: no cover - useful for running the tests off Windows
        try:
            os.killpg(os.getpgid(root_pid), signal.SIGKILL)
        except (OSError, ProcessLookupError):
            pass
    _terminate_known_identities(identities)


def _tree_rss_bytes(identities: Iterable[ProcessIdentity]) -> int:
    if psutil is None:
        return 0
    total = 0
    for identity in identities:
        if not _identity_is_alive(identity):
            continue
        try:
            total += int(psutil.Process(identity.pid).memory_info().rss)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    return total


def _popen_creation_options() -> dict[str, Any]:
    if os.name == "nt":
        return {
            "creationflags": (
                getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
                | getattr(subprocess, "CREATE_NO_WINDOW", 0)
            )
        }
    return {"start_new_session": True}


def supervise_command(
    command: Sequence[str],
    *,
    timeout_seconds: float,
    minimum_available_memory_bytes: int = DEFAULT_MIN_AVAILABLE_BYTES,
    poll_seconds: float = DEFAULT_POLL_SECONDS,
    cancel_event: threading.Event | None = None,
    cancel_sentinel: Path | None = None,
    available_memory_provider: Callable[[], int] = get_available_memory_bytes,
    residual_grace_seconds: float = 0.25,
) -> SupervisionResult:
    """Run and supervise one child command without capturing its private body."""

    if not command or not str(command[0]).strip():
        raise SupervisorError("a child command is required")
    _require_duration(timeout_seconds, "timeout")
    _require_duration(poll_seconds, "poll interval")
    if minimum_available_memory_bytes < 0:
        raise SupervisorError("minimum available memory must not be negative")

    started = time.monotonic()
    kill_job = KillOnCloseJob()
    try:
        process = subprocess.Popen(
            [str(part) for part in command],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            **_popen_creation_options(),
        )
        try:
            kill_job.assign(process)
        except BaseException:
            terminate_process_tree(process.pid)
            process.wait(timeout=5.0)
            raise
    except BaseException:
        kill_job.close()
        raise
    known: set[ProcessIdentity] = set(discover_process_tree(process.pid))
    peak_rss = 0
    minimum_available = 2**63 - 1
    termination_reason: str | None = None

    try:
        while True:
            known.update(discover_process_tree(process.pid))
            peak_rss = max(peak_rss, _tree_rss_bytes(known))
            available = int(available_memory_provider())
            minimum_available = min(minimum_available, available)

            cancelled = cancel_event is not None and cancel_event.is_set()
            if cancel_sentinel is not None and cancel_sentinel.exists():
                cancelled = True
            elapsed = time.monotonic() - started

            if cancelled:
                termination_reason = "cancelled"
            elif elapsed >= timeout_seconds:
                termination_reason = "timeout"
            elif available < minimum_available_memory_bytes:
                termination_reason = "low_available_memory"

            if termination_reason is not None:
                terminate_process_tree(process.pid, known)
                break
            if process.poll() is not None:
                termination_reason = (
                    "completed" if process.returncode == 0 else "child_exit_nonzero"
                )
                break
            time.sleep(poll_seconds)
    except KeyboardInterrupt:
        termination_reason = "cancelled"
        terminate_process_tree(process.pid, known)
    except BaseException:
        terminate_process_tree(process.pid, known)
        kill_job.close()
        raise

    try:
        child_exit_code = process.wait(timeout=5.0)
    except subprocess.TimeoutExpired:
        terminate_process_tree(process.pid, known)
        try:
            child_exit_code = process.wait(timeout=3.0)
        except subprocess.TimeoutExpired:
            child_exit_code = None

    # A root can exit while an observed worker remains.  Check those identities,
    # clean them, then check again; create_time prevents killing a reused PID.
    if residual_grace_seconds > 0:
        time.sleep(residual_grace_seconds)
    residual_before = check_residual_processes(known)
    if residual_before:
        _terminate_known_identities(
            identity for identity in known if identity.pid in residual_before
        )
        time.sleep(min(max(poll_seconds, 0.01), 0.25))
    residual_after = check_residual_processes(known)
    if residual_after:
        termination_reason = "residual_processes"

    if minimum_available == 2**63 - 1:
        minimum_available = int(available_memory_provider())
    result = SupervisionResult(
        termination_reason=termination_reason or "supervisor_error",
        child_exit_code=child_exit_code,
        elapsed_seconds=time.monotonic() - started,
        peak_tree_rss_bytes=peak_rss,
        minimum_available_memory_bytes=minimum_available,
        observed_process_count=len(known),
        residual_processes_before_cleanup=residual_before,
        residual_processes_after_cleanup=residual_after,
    )
    kill_job.close()
    return result


def content_fingerprint(value: str) -> dict[str, Any]:
    encoded = value.encode("utf-8", errors="replace")
    return {
        "utf8_bytes": len(encoded),
        "characters": len(value),
        "sha256": hashlib.sha256(encoded).hexdigest().upper(),
    }


def _path_basename(value: str) -> str:
    return ntpath.basename(value.replace("/", "\\")) or "redacted-path"


def make_public_evidence(value: Any, key: str | None = None) -> Any:
    """Remove full paths, command/body text, and traceback from public data."""

    normalized_key = (key or "").lower()
    if normalized_key == "traceback":
        return None
    if normalized_key in CONTENT_KEYS:
        if isinstance(value, str):
            return content_fingerprint(value)
        return {"sha256": hashlib.sha256(repr(value).encode()).hexdigest().upper()}
    if isinstance(value, str):
        path_key = normalized_key.endswith(("path", "directory", "dir"))
        if path_key:
            return {"file_name": _path_basename(value)}
        if FULL_PATH_RE.search(value):
            return content_fingerprint(value)
        return value
    if dataclasses.is_dataclass(value):
        return make_public_evidence(dataclasses.asdict(value), key)
    if isinstance(value, dict):
        public: dict[str, Any] = {}
        for child_key, child_value in value.items():
            if str(child_key).lower() == "traceback":
                continue
            public[str(child_key)] = make_public_evidence(
                child_value, str(child_key)
            )
        return public
    if isinstance(value, (list, tuple, set)):
        return [make_public_evidence(item) for item in value]
    return value


def atomic_write_text_exclusive(path: Path, content: str) -> None:
    """Create one evidence file without replacing an existing file."""

    if path.exists():
        raise SupervisorError(f"refusing to overwrite evidence file: {path.name}")
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_name(
        f".{path.name}.{os.getpid()}.{time.time_ns()}.partial"
    )
    try:
        with partial.open("x", encoding="utf-8", newline="\n") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        # A same-directory hard link publishes the fully flushed inode in one
        # step and fails if the destination appeared concurrently.  This keeps
        # exclusivity on both Windows and POSIX instead of relying on POSIX
        # rename semantics, which would replace an existing destination.
        try:
            os.link(partial, path)
        except FileExistsError as exc:
            raise SupervisorError(
                f"refusing to overwrite evidence file: {path.name}"
            ) from exc
        partial.unlink()
    finally:
        if partial.exists():
            partial.unlink()


def write_public_evidence_atomic(path: Path, payload: dict[str, Any]) -> str:
    """Sanitize, atomically write JSON, then write its SHA-256 sidecar."""

    public = make_public_evidence(payload)
    serialized = json.dumps(
        public, ensure_ascii=False, indent=2, allow_nan=False, sort_keys=True
    ) + "\n"
    json.loads(serialized)
    atomic_write_text_exclusive(path, serialized)
    digest = sha256_file(path)
    atomic_write_text_exclusive(
        path.with_suffix(path.suffix + ".sha256"),
        f"{digest}  {path.name}\n",
    )
    return digest


def command_fingerprint(command: Sequence[str]) -> dict[str, Any]:
    encoded = "\0".join(str(part) for part in command).encode(
        "utf-8", errors="replace"
    )
    return {
        "executable_name": _path_basename(str(command[0])),
        "argument_count": max(0, len(command) - 1),
        "sha256": hashlib.sha256(encoded).hexdigest().upper(),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Supervise one P1 child run. Rejected monoliths receive a NOT_RUN "
            "600 s / 1 s-overlap chunk plan, never a fabricated inference PASS."
        )
    )
    parser.add_argument("--duration-seconds", type=float, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--evidence-name")
    parser.add_argument("--timeout-seconds", type=float, default=4 * 60 * 60)
    parser.add_argument(
        "--min-available-gib", type=float, default=DEFAULT_MIN_AVAILABLE_BYTES / GIB
    )
    parser.add_argument("--chunk-seconds", type=float, default=DEFAULT_CHUNK_SECONDS)
    parser.add_argument("--overlap-seconds", type=float, default=DEFAULT_OVERLAP_SECONDS)
    parser.add_argument("--cancel-sentinel", type=Path)
    parser.add_argument(
        "--plan-only",
        action="store_true",
        help="write admission/chunk evidence without starting a child",
    )
    parser.add_argument(
        "--bounded-chunk-child",
        action="store_true",
        help=(
            "Supervise a child that independently proves every 480-600 second "
            "bounded session; the supervisor does not claim the child evidence."
        ),
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def _safe_evidence_name(value: str | None) -> str:
    if value is None:
        return (
            "MOSS-V3-P1-SUPERVISOR-"
            + datetime.now().astimezone().strftime("%Y%m%d-%H%M%S-%f")
            + f"-{os.getpid()}.json"
        )
    name = value if value.lower().endswith(".json") else value + ".json"
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,119}", name):
        raise SupervisorError("evidence name contains unsupported characters")
    return name


def _payload_base(
    duration_seconds: float,
    admission: MemoryAdmission,
    chunk_plan: dict[str, Any],
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "gate": "MOSS_V3_P1_EXTERNAL_SUPERVISOR",
        "captured_at": now_iso(),
        "duration_seconds": round(duration_seconds, 6),
        "monolithic_memory_admission": admission.public_record(),
        "chunk_plan": chunk_plan,
        "chunk_execution_status": "NOT_RUN",
        "chunk_inference_verdict": "NOT_EVALUATED",
        "p1_inference_verdict": "NOT_EVALUATED_BY_SUPERVISOR",
    }


def main(argv: Sequence[str] | None = None) -> int:
    if not sys.flags.isolated or not sys.flags.no_site or not sys.flags.ignore_environment:
        print('{"status":"FAIL","error_code":"P1_ISOLATED_PYTHON_REQUIRED"}')
        return 2
    if (
        not sys.dont_write_bytecode
        or sys.pycache_prefix is None
        or Path(sys.pycache_prefix).resolve() != PYTHON_CACHE_ROOT
        or not PYTHON_CACHE_ROOT.is_dir()
        or any(PYTHON_CACHE_ROOT.rglob("*"))
    ):
        print('{"status":"FAIL","error_code":"P1_EMPTY_PYCACHE_PREFIX_REQUIRED"}')
        return 2
    if (
        not TASKKILL_PATH.is_file()
        or hashlib.sha256(TASKKILL_PATH.read_bytes()).hexdigest().upper()
        != EXPECTED_TASKKILL_SHA256
    ):
        print('{"status":"FAIL","error_code":"P1_TASKKILL_IDENTITY_MISMATCH"}')
        return 2
    parser = build_parser()
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not args.plan_only and not command:
        parser.error("a child command after -- is required unless --plan-only is used")
    if not math.isfinite(args.min_available_gib) or args.min_available_gib < 0:
        parser.error("--min-available-gib must be finite and non-negative")

    evidence_path = args.evidence_dir.resolve() / _safe_evidence_name(
        args.evidence_name
    )
    exit_code = 3
    try:
        duration = _require_duration(args.duration_seconds)
        available = get_available_memory_bytes()
        admission_duration = args.chunk_seconds if args.bounded_chunk_child else duration
        admission = assess_memory_admission(admission_duration, available)
        if args.bounded_chunk_child:
            chunk_plan = {
                "execution_status": "CHILD_EVIDENCE_REQUIRED",
                "inference_verdict": "NOT_EVALUATED",
                "maximum_chunk_seconds": args.chunk_seconds,
                "full_audio_duration_seconds": duration,
                "plan_is_not_inference_evidence": True,
            }
        else:
            chunk_plan = build_chunk_plan(
                duration, args.chunk_seconds, args.overlap_seconds
            )
        payload = _payload_base(duration, admission, chunk_plan)
        payload["memory_admission_duration_seconds"] = admission_duration
        payload["bounded_chunk_child"] = bool(args.bounded_chunk_child)

        if not admission.admitted:
            payload["status"] = "MONOLITHIC_REJECTED"
            payload["child_execution_status"] = "NOT_RUN"
            exit_code = 2
        elif args.plan_only:
            payload["status"] = "PLAN_ONLY"
            payload["child_execution_status"] = "NOT_RUN"
            exit_code = 0
        else:
            result = supervise_command(
                command,
                timeout_seconds=args.timeout_seconds,
                minimum_available_memory_bytes=math.ceil(
                    args.min_available_gib * GIB
                ),
                cancel_sentinel=(
                    args.cancel_sentinel.resolve()
                    if args.cancel_sentinel is not None
                    else None
                ),
            )
            payload["child"] = {
                "command_fingerprint": command_fingerprint(command),
                "supervision": result.public_record(),
            }
            if result.termination_reason == "completed":
                payload["status"] = "SUPERVISED_CHILD_COMPLETED"
                payload["child_execution_status"] = "COMPLETED"
                payload["p1_inference_verdict"] = "CHILD_EVIDENCE_REQUIRED"
                exit_code = 0
            else:
                payload["status"] = "SUPERVISED_CHILD_NOT_COMPLETED"
                payload["child_execution_status"] = "TERMINATED_OR_FAILED"
                exit_code = 3
    except BaseException as exc:
        payload = {
            "schema_version": 1,
            "gate": "MOSS_V3_P1_EXTERNAL_SUPERVISOR",
            "captured_at": now_iso(),
            "status": "SUPERVISOR_ERROR",
            "error": {
                "code": "P1_SUPERVISOR_ERROR",
                "type": type(exc).__name__,
            },
            "chunk_execution_status": "NOT_RUN",
            "chunk_inference_verdict": "NOT_EVALUATED",
            "p1_inference_verdict": "NOT_EVALUATED_BY_SUPERVISOR",
        }
        exit_code = 3

    digest = write_public_evidence_atomic(evidence_path, payload)
    print(
        json.dumps(
            {
                "status": payload["status"],
                "evidence_file": evidence_path.name,
                "evidence_sha256": digest,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
