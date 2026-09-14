#!/usr/bin/env python3
"""Launch the frozen R3 Whisper supervisor with the current P0-R scorer.

The upstream supervisor imports ``score_s8_m00_r3`` by module name.  The P0-R
balanced package contains the reviewed scorer that fixes partial-overlap
annotation handling, while the already-audited supervisor implementation is
kept byte-for-byte in the older R3 evidence package.  This launcher makes that
dependency selection explicit without editing either frozen file.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib
import json
import runpy
import sys
from pathlib import Path


if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def prepare_modules(scorer_root: Path, supervisor: Path) -> dict[str, str]:
    scorer_root = scorer_root.resolve(strict=True)
    supervisor = supervisor.resolve(strict=True)
    scorer_path = (scorer_root / "score_s8_m00_r3.py").resolve(strict=True)
    gate_path = (
        supervisor.parent / "supervise_windows_cuda_gate_r3.py"
    ).resolve(strict=True)
    if supervisor.name != "supervise_whisper_baseline_r3.py":
        raise SystemExit("unexpected Whisper supervisor filename")
    if "score_s8_m00_r3" in sys.modules:
        raise SystemExit("score_s8_m00_r3 was imported before dependency lock")

    sys.path.insert(0, str(scorer_root))
    sys.path.insert(1, str(supervisor.parent))
    scorer = importlib.import_module("score_s8_m00_r3")
    loaded_scorer = Path(scorer.__file__).resolve(strict=True)
    if loaded_scorer != scorer_path:
        raise SystemExit("formal supervisor did not load the balanced scorer")

    return {
        "launcher_path": str(Path(__file__).resolve(strict=True)),
        "launcher_sha256": sha256_file(Path(__file__).resolve(strict=True)),
        "supervisor_path": str(supervisor),
        "supervisor_sha256": sha256_file(supervisor),
        "shared_gate_supervisor_path": str(gate_path),
        "shared_gate_supervisor_sha256": sha256_file(gate_path),
        "scorer_path": str(loaded_scorer),
        "scorer_sha256": sha256_file(loaded_scorer),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scorer-root", type=Path, required=True)
    parser.add_argument("--supervisor", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("supervisor_args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    lock = prepare_modules(args.scorer_root, args.supervisor)
    if args.verify_only:
        print(json.dumps(lock, ensure_ascii=False, indent=2))
        return 0
    supervisor_args = list(args.supervisor_args)
    if supervisor_args and supervisor_args[0] == "--":
        supervisor_args.pop(0)
    if not supervisor_args:
        raise SystemExit("formal supervisor arguments are missing")
    sys.argv = [str(args.supervisor.resolve(strict=True)), *supervisor_args]
    runpy.run_path(str(args.supervisor.resolve(strict=True)), run_name="__main__")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
