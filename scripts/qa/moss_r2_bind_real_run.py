#!/usr/bin/env python3
"""Bind the R2 real-run evidence to exact source and binary inputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest().upper()


def artifact(path: Path) -> dict[str, object]:
    path = path.resolve(strict=True)
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return {"bytes": size, "sha256": digest.hexdigest().upper()}


def git(repo: Path, *arguments: str) -> bytes:
    return subprocess.run(
        ["git", *arguments],
        cwd=repo,
        check=True,
        capture_output=True,
    ).stdout


def write_new_json(path: Path, value: dict[str, object]) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    with path.open("xb") as stream:
        stream.write(payload)
        stream.flush()
        os.fsync(stream.fileno())
    digest = sha256_bytes(payload)
    sidecar = path.with_suffix(path.suffix + ".sha256")
    with sidecar.open("x", encoding="ascii", newline="\n") as stream:
        stream.write(f"{digest}  {path.name}\n")
        stream.flush()
        os.fsync(stream.fileno())
    return digest


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--test-executable", type=Path, required=True)
    parser.add_argument("--helper", type=Path, required=True)
    parser.add_argument("--runtime-manifest", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--binding-output", type=Path, required=True)
    parser.add_argument("--fields-output", type=Path, required=True)
    arguments = parser.parse_args()

    repo = arguments.repo.resolve(strict=True)
    status = git(repo, "status", "--porcelain=v1")
    tracked_diff = git(repo, "diff", "--binary", "HEAD", "--")
    records = {
        "cargo_lock": artifact(repo / "Cargo.lock"),
        "test_executable": artifact(arguments.test_executable),
        "moss_helper": artifact(arguments.helper),
        "runtime_manifest": artifact(arguments.runtime_manifest),
        "q8_model": artifact(arguments.model),
        "frozen_fixture": artifact(arguments.fixture),
    }
    binding: dict[str, object] = {
        "schema_version": 1,
        "stage": "MOSS_R2_REAL_RUN_BINDING",
        "source_commit": git(repo, "rev-parse", "HEAD").decode("ascii").strip(),
        "source_tree_clean": not status.strip(),
        "source_status_bytes": len(status),
        "source_status_sha256": sha256_bytes(status),
        "tracked_diff_bytes": len(tracked_diff),
        "tracked_diff_sha256": sha256_bytes(tracked_diff),
        "artifacts": records,
        "absolute_paths_in_evidence": False,
        "transcript_in_evidence": False,
    }
    binding_sha256 = write_new_json(arguments.binding_output, binding)
    fields = {
        "binding_sha256": binding_sha256,
        "source_commit": binding["source_commit"],
        "source_tree_clean": binding["source_tree_clean"],
        "cargo_lock_sha256": records["cargo_lock"]["sha256"],
        "test_executable_sha256": records["test_executable"]["sha256"],
        "helper_binary_sha256": records["moss_helper"]["sha256"],
        "runtime_manifest_sha256": records["runtime_manifest"]["sha256"],
        "model_sha256": records["q8_model"]["sha256"],
        "fixture_sha256": records["frozen_fixture"]["sha256"],
    }
    write_new_json(arguments.fields_output, fields)


if __name__ == "__main__":
    main()
