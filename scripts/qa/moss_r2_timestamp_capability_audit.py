#!/usr/bin/env python3
"""Mechanically audit the frozen transcribe.cpp MOSS timestamp capability."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


AUDITED_FILES = (
    Path("src/arch/moss/capabilities.cpp"),
    Path("src/arch/moss/diarize.cpp"),
    Path("include/transcribe.h"),
)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def git_text(root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return completed.stdout.strip()


def write_new_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        raise FileExistsError(f"refusing to overwrite evidence: {path}")
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def audit(source_root: Path, expected_commit: str) -> dict[str, object]:
    root = source_root.resolve(strict=True)
    commit = git_text(root, "rev-parse", "HEAD")
    tree_status = git_text(root, "status", "--porcelain")
    texts: dict[str, str] = {}
    files: list[dict[str, object]] = []
    for relative in AUDITED_FILES:
        path = root / relative
        text = path.read_text(encoding="utf-8")
        texts[relative.as_posix()] = text
        files.append(
            {
                "relative_path": relative.as_posix(),
                "bytes": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )

    capabilities = texts["src/arch/moss/capabilities.cpp"]
    diarize = texts["src/arch/moss/diarize.cpp"]
    header = texts["include/transcribe.h"]
    checks = {
        "commit_matches_frozen_value": commit.lower() == expected_commit.lower(),
        "source_tree_is_clean": tree_status == "",
        "moss_declares_segment_as_maximum_timestamp_kind": (
            "caps.max_timestamp_kind = TRANSCRIBE_TIMESTAMPS_SEGMENT;"
            in capabilities
        ),
        "moss_does_not_declare_word_as_maximum_timestamp_kind": (
            "caps.max_timestamp_kind = TRANSCRIBE_TIMESTAMPS_WORD;"
            not in capabilities
        ),
        "moss_result_policy_returns_segment_timestamps": (
            "returned_timestamp_kind" in diarize
            and "TRANSCRIBE_TIMESTAMPS_SEGMENT" in diarize
        ),
        "generic_header_contains_word_api": (
            "transcribe_n_words" in header and "transcribe_get_word" in header
        ),
    }
    status = "PASS" if all(checks.values()) else "FAIL"
    return {
        "schema_version": 1,
        "role": "MOSS_R2_TIMESTAMP_CAPABILITY_AUDIT",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "status": status,
        "frozen_source_commit": commit,
        "expected_source_commit": expected_commit.lower(),
        "source_tree_clean": tree_status == "",
        "files": files,
        "checks": checks,
        "conclusion": {
            "moss_timestamp_granularity": "SEGMENT",
            "moss_word_timestamp_supported": False,
            "generic_word_api_presence_does_not_override_family_capability": True,
            "window_boundary_gate_status": "BLOCKED_BY_FROZEN_MOSS_PORT_CAPABILITY",
            "synthetic_word_timestamps_allowed": False,
        },
        "absolute_paths_in_evidence": False,
        "transcript_in_evidence": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--expected-commit", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    payload = audit(args.source_root, args.expected_commit)
    write_new_json(args.output, payload)
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
