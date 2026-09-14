#!/usr/bin/env python3
"""Shared, standard-library-only helpers for the MOSS v3 P6 QA tools."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import tempfile
from datetime import datetime, timezone
from typing import Any, Iterable


PASS = "PASS"
FAIL = "FAIL"
BLOCKED = "BLOCKED"
NOT_RUN = "NOT RUN"
ALLOWED_STATUSES = {PASS, FAIL, BLOCKED, NOT_RUN}

REQUIRED_P6_SCENARIOS = (
    "live-bubbles",
    "stop-convergence",
    "manual-edit-persistence",
    "moss-progress",
    "moss-cancel",
    "candidate-review",
    "term-correction-positive",
    "term-correction-negative",
    "person-binding-and-segment-correction",
    "restart-persistence",
    "activate-moss",
    "qwen-2b-summary",
    "needs-review-conflict",
    "rollback-to-whisper",
    "reactivate-moss",
    "offline-release-chain",
    "forced-exit-recovery",
    "network-failure-preserves-data",
    "corrupt-model-preserves-data",
    "summary-failure-preserves-data",
    "process-release-before-summary",
    "windows-fresh-install",
    "windows-upgrade",
    "windows-uninstall",
    "windows-version-rollback",
    "release-data-drive-placement",
    "long-audio-3096-complete",
    "business-audio-737-chain",
)
REQUIRED_LIFECYCLE_CHECKPOINTS = (
    "before_install",
    "after_install",
    "before_upgrade",
    "after_upgrade",
    "after_uninstall",
    "before_rollback",
    "after_rollback",
)
REQUIRED_LIFECYCLE_TRANSITIONS = ("install", "upgrade", "uninstall", "rollback")

SHA256_RE = re.compile(r"^[0-9A-F]{64}$")
GIT_COMMIT_RE = re.compile(r"^[0-9a-f]{40}$")


class GateError(RuntimeError):
    """Raised when evidence is malformed or does not satisfy a hard gate."""


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest().upper()


def hash_file(path: Path) -> tuple[int, str]:
    before = path.stat()
    digest = hashlib.sha256()
    byte_count = 0
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
            byte_count += len(block)
    after = path.stat()
    before_identity = (before.st_size, before.st_mtime_ns, before.st_ino)
    after_identity = (after.st_size, after.st_mtime_ns, after.st_ino)
    if before_identity != after_identity or byte_count != after.st_size:
        raise GateError(f"Evidence file changed while it was being hashed: {path}")
    return byte_count, digest.hexdigest().upper()


def sha256_file(path: Path) -> str:
    return hash_file(path)[1]


def canonical_json_bytes(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")


def read_json(path: Path, label: str = "JSON document") -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise GateError(f"{label} contains duplicate key {key!r}: {path}")
            result[key] = value
        return result

    def reject_nonfinite(value: str) -> Any:
        raise GateError(f"{label} contains non-finite number {value}: {path}")

    try:
        reject_symlink(path)
        return json.loads(
            path.read_text(encoding="utf-8-sig"),
            object_pairs_hook=reject_duplicates,
            parse_constant=reject_nonfinite,
        )
    except FileNotFoundError as exc:
        raise GateError(f"{label} is missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise GateError(f"{label} is not valid JSON: {path}: {exc}") from exc


def atomic_write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        indent=2,
        sort_keys=True,
    ).encode("utf-8") + b"\n"
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", suffix=".tmp", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def require_mapping(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise GateError(f"{label} must be a JSON object")
    return value


def require_list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise GateError(f"{label} must be a JSON array")
    return value


def require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise GateError(f"{label} must be a non-empty string")
    return value


def require_sha256(value: Any, label: str) -> str:
    text = require_string(value, label).upper()
    if not SHA256_RE.fullmatch(text):
        raise GateError(f"{label} must be a 64-character SHA-256")
    return text


def require_git_commit(value: Any, label: str) -> str:
    text = require_string(value, label).lower()
    if not GIT_COMMIT_RE.fullmatch(text):
        raise GateError(f"{label} must be a full 40-character Git commit")
    return text


def require_status(value: Any, label: str) -> str:
    text = require_string(value, label)
    if text not in ALLOWED_STATUSES:
        raise GateError(f"{label} has unsupported status {text!r}")
    return text


def normalize_relative_path(value: Any, label: str) -> str:
    text = require_string(value, label).replace("\\", "/")
    posix = PurePosixPath(text)
    windows = PureWindowsPath(text)
    if (
        posix.is_absolute()
        or windows.is_absolute()
        or windows.drive
        or any(part in {"", ".", ".."} for part in posix.parts)
    ):
        raise GateError(f"{label} must be a safe relative path: {text!r}")
    return posix.as_posix()


def resolve_under(root: Path, relative: Any, label: str) -> tuple[Path, str]:
    normalized = normalize_relative_path(relative, label)
    # Keep the lexical paths long enough to reject links.  Resolving first would
    # erase the fact that an evidence path traversed a symlink or junction.
    lexical_root = Path(os.path.abspath(root))
    lexical_candidate = lexical_root / Path(*PurePosixPath(normalized).parts)
    reject_symlink(lexical_root)
    reject_symlink(lexical_candidate, boundary=lexical_root)
    resolved_root = lexical_root.resolve()
    candidate = lexical_candidate.resolve()
    try:
        candidate.relative_to(resolved_root)
    except ValueError as exc:
        raise GateError(f"{label} resolves outside its declared root") from exc
    return candidate, normalized


def reject_symlink(path: Path, *, boundary: Path | None = None) -> None:
    """Reject a symlink at ``path`` or in its chain below ``boundary``."""

    current = path
    resolved_boundary = boundary.resolve() if boundary else None
    while True:
        is_junction = bool(getattr(current, "is_junction", lambda: False)())
        if current.is_symlink() or is_junction:
            raise GateError(f"Links and junctions are not accepted as release evidence: {current}")
        if resolved_boundary is None or current == resolved_boundary:
            break
        parent = current.parent
        if parent == current:
            break
        try:
            parent.resolve().relative_to(resolved_boundary)
        except ValueError:
            break
        current = parent


def file_record(path: Path, *, relative_path: str | None = None) -> dict[str, Any]:
    if not path.is_file():
        raise GateError(f"Required file is missing: {path}")
    reject_symlink(path)
    byte_count, digest = hash_file(path)
    return {
        "path": relative_path if relative_path is not None else path.name,
        "bytes": byte_count,
        "sha256": digest,
    }


def tree_records(root: Path) -> list[dict[str, Any]]:
    if not root.exists():
        return []
    if not root.is_dir():
        raise GateError(f"Tree root is not a directory: {root}")
    reject_symlink(root)
    records: list[dict[str, Any]] = []
    for directory, directory_names, file_names in os.walk(root, followlinks=False):
        directory_path = Path(directory)
        for name in list(directory_names):
            child = directory_path / name
            is_junction = bool(getattr(child, "is_junction", lambda: False)())
            if child.is_symlink() or is_junction:
                raise GateError(f"Linked directory is not accepted in evidence: {child}")
        for name in sorted(file_names):
            child = directory_path / name
            reject_symlink(child, boundary=root)
            if not child.is_file():
                raise GateError(f"Non-regular file is not accepted in evidence: {child}")
            relative = child.relative_to(root).as_posix()
            records.append(file_record(child, relative_path=relative))
    records.sort(key=lambda item: str(item["path"]).casefold())
    return records


def records_sha256(records: Iterable[dict[str, Any]]) -> str:
    normalized = [
        {
            "path": item["path"],
            "bytes": item["bytes"],
            "sha256": item["sha256"],
        }
        for item in records
    ]
    return sha256_bytes(canonical_json_bytes(normalized))


def status_exit_code(status: str) -> int:
    if status == PASS:
        return 0
    if status == FAIL:
        return 1
    return 2


def checks_status(checks: dict[str, bool]) -> str:
    return PASS if checks and all(checks.values()) else FAIL
