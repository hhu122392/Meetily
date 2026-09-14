#!/usr/bin/env python3
"""Run the reproducible P0 source regression suite and write immutable-style evidence.

This runner deliberately excludes only blocknote-markdown.test.ts because that file
imports bun:test and Bun is not part of the frozen Windows toolchain.  The exclusion
is recorded in the machine-readable result instead of being hidden.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Sequence


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def run_command(
    *,
    name: str,
    argv: Sequence[str],
    cwd: Path,
    evidence_dir: Path,
    env: dict[str, str] | None = None,
) -> dict[str, object]:
    executable = shutil.which(argv[0], path=(env or os.environ).get("PATH"))
    if executable is None:
        raise FileNotFoundError(f"Executable not found: {argv[0]}")
    resolved_argv = [executable, *argv[1:]]
    started = time.perf_counter()
    completed = subprocess.run(
        resolved_argv,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    elapsed = time.perf_counter() - started
    log_path = evidence_dir / f"03-{name}.log"
    log_path.write_text(
        completed.stdout.decode("utf-8", errors="replace"),
        encoding="utf-8",
        newline="\n",
    )
    return {
        "name": name,
        "argv": resolved_argv,
        "cwd": str(cwd),
        "exit_code": completed.returncode,
        "elapsed_seconds": round(elapsed, 3),
        "log_path": str(log_path),
        "log_bytes": log_path.stat().st_size,
        "log_sha256": sha256(log_path),
        "status": "PASS" if completed.returncode == 0 else "FAIL",
    }


def rust_environment(repo: Path) -> dict[str, str]:
    env = os.environ.copy()
    msvc_root = Path(r"C:\BuildTools\VC\Tools\MSVC\14.44.35207")
    windows_sdk = Path(r"C:\Program Files (x86)\Windows Kits\10")
    compiler_bin = msvc_root / "bin" / "Hostx64" / "x64"
    sdk_bin = windows_sdk / "bin" / "10.0.26100.0" / "x64"
    required = [
        compiler_bin / "cl.exe",
        compiler_bin / "nmake.exe",
        repo / ".tools" / "libclang" / "clang" / "native" / "libclang.dll",
    ]
    missing = [str(path) for path in required if not path.exists()]
    if missing:
        raise FileNotFoundError(f"P0 Rust toolchain is incomplete: {missing}")

    env["PATH"] = os.pathsep.join([str(compiler_bin), str(sdk_bin), env.get("PATH", "")])
    env["INCLUDE"] = ";".join(
        [
            str(msvc_root / "include"),
            r"C:\BuildTools\VC\Auxiliary\VS\include",
            str(windows_sdk / "include" / "10.0.26100.0" / "ucrt"),
            str(windows_sdk / "include" / "10.0.26100.0" / "um"),
            str(windows_sdk / "include" / "10.0.26100.0" / "shared"),
            str(windows_sdk / "include" / "10.0.26100.0" / "winrt"),
            str(windows_sdk / "include" / "10.0.26100.0" / "cppwinrt"),
        ]
    )
    env["LIB"] = ";".join(
        [
            str(msvc_root / "lib" / "x64"),
            str(windows_sdk / "lib" / "10.0.26100.0" / "ucrt" / "x64"),
            str(windows_sdk / "lib" / "10.0.26100.0" / "um" / "x64"),
        ]
    )
    env["LIBCLANG_PATH"] = str(repo / ".tools" / "libclang" / "clang" / "native")
    env["CMAKE_GENERATOR"] = "NMake Makefiles"
    env["CMAKE_MAKE_PROGRAM"] = str(compiler_bin / "nmake.exe")
    env["CARGO_TARGET_DIR"] = r"D:\MeetilyBuildScratch\moss-v3-cargo-p0-nmake"
    env["PYTHONIOENCODING"] = "utf-8"
    return env


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()

    repo = args.repo.resolve()
    frontend = repo / "frontend"
    evidence_dir = args.evidence_dir.resolve()
    evidence_dir.mkdir(parents=True, exist_ok=True)

    node_tests = sorted(
        path.relative_to(frontend).as_posix()
        for path in (frontend / "tests").rglob("*.test.*")
        if path.suffix in {".ts", ".tsx"} and path.name != "blocknote-markdown.test.ts"
    )
    if not node_tests:
        raise RuntimeError("No Node-compatible frontend tests were found")

    commands: list[tuple[str, list[str], Path, dict[str, str] | None]] = [
        ("i18n", ["pnpm", "test:i18n"], frontend, None),
        ("frontend-build", ["pnpm", "build"], frontend, None),
        (
            "frontend-node-tests",
            ["pnpm", "exec", "tsx", "--test", *node_tests],
            frontend,
            None,
        ),
        (
            "cargo-fmt",
            [
                "cargo",
                "fmt",
                "--manifest-path",
                str(frontend / "src-tauri" / "Cargo.toml"),
                "--",
                "--check",
            ],
            repo,
            None,
        ),
        (
            "cargo-test",
            [
                "cargo",
                "test",
                "--manifest-path",
                str(frontend / "src-tauri" / "Cargo.toml"),
            ],
            repo,
            rust_environment(repo),
        ),
    ]

    results: list[dict[str, object]] = []
    for name, argv, cwd, env in commands:
        print(f"[P0] running {name}", flush=True)
        result = run_command(
            name=name,
            argv=argv,
            cwd=cwd,
            evidence_dir=evidence_dir,
            env=env,
        )
        results.append(result)
        print(f"[P0] {name}: {result['status']}", flush=True)

    payload = {
        "schema_version": 1,
        "suite": "MOSS-V3-P0-SOURCE-REGRESSION",
        "status": "PASS" if all(item["status"] == "PASS" for item in results) else "FAIL",
        "node_compatible_test_file_count": len(node_tests),
        "node_compatible_test_files": node_tests,
        "explicit_exclusions": [
            {
                "path": "tests/lib/blocknote-markdown.test.ts",
                "reason": "Imports bun:test; Bun is not installed in the frozen Windows toolchain.",
                "status": "NOT_RUN",
            }
        ],
        "commands": results,
    }
    output = evidence_dir / "03-source-tests.json"
    output.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": payload["status"], "output": str(output)}, ensure_ascii=False))
    return 0 if payload["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
