#!/usr/bin/env python3
"""Freeze the Meetily MOSS v3 P0 source, inputs, environment, and user-data baseline.

The script is intentionally read-only for the repository and source user data. It
copies the current release files and a consistent SQLite backup into an external
recovery directory, then writes hash manifests into the requested evidence folder.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sqlite3
import subprocess
import sys
import time
import wave
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


EXPECTED_INPUTS = (
    (
        "short_12s",
        Path(r"D:\桌面\meetlily\.tools\moss-poc\fixtures\meetily-zh-12s.wav"),
        "E9376B7F00C3B553211E27E65BE397A09CEEB2ACDF28337AB951B7C7CDBBE9DD",
    ),
    (
        "real_107s",
        Path(r"D:\音乐\meetily-recordings\Meeting 2026-08-25_13-01-18_2026-08-25_05-01\audio.mp4"),
        "0361C5203587BFBF65ADC2F31DF157C8F14C44F2D42CEADF88C114A8EF1789F6",
    ),
    (
        "review_244s",
        Path(r"D:\桌面\meetlily\target\release\docs\方案\证据\MOSS-S8-M00-R3-20260828-110430\human-review-window-04m04s940ms.wav"),
        "DE0B7237880441606AD614CF8508EB34CA76ABE64A875DAFD0B7E030705FB881",
    ),
    (
        "business_737s",
        Path(r"D:\桌面\meetlily\target\release\docs\方案\证据\MOSS-S8-M00-R3-20260828-110430\source-context-frozen-real-business.wav"),
        "82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB",
    ),
    (
        "monthly_900s",
        Path(r"D:\MeetilyBuildScratch\uat-20260828\real-monthly-meeting-20260827-first-15min.m4a"),
        "C6815E3C81CA022B12EA48BB2102EFCFA625B3F760FFC796F5D0F02002A0DD71",
    ),
    (
        "long_3096s",
        Path(r"D:\音乐\meetily-recordings\会议 26_08_26_13_59_54_2026-08-26_05-59\audio.mp4"),
        "A1B8A502E5C9A3A88D7F1AC425229DEFDE085A389FE4CE78D3C665B57A373C04",
    ),
)


def sha256(path: Path, chunk_size: int = 8 * 1024 * 1024) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(chunk_size):
            digest.update(chunk)
    return digest.hexdigest().upper()


def write_json(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def media_duration(path: Path) -> tuple[float, str]:
    if path.suffix.lower() == ".wav":
        with wave.open(str(path), "rb") as wav:
            return wav.getnframes() / wav.getframerate(), "wave_pcm_frames"

    try:
        import av  # type: ignore

        with av.open(str(path)) as container:
            if container.duration is not None:
                return float(container.duration / av.time_base), "pyav_container_duration"
            stream = next(stream for stream in container.streams if stream.type == "audio")
            if stream.duration is None or stream.time_base is None:
                raise RuntimeError("audio stream has no duration")
            return float(stream.duration * stream.time_base), "pyav_audio_stream_duration"
    except Exception as exc:  # pragma: no cover - only used for actionable diagnostics
        raise RuntimeError(f"Unable to read media duration for {path}: {exc}") from exc


def copy_with_hash(source: Path, destination: Path) -> dict[str, Any]:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    source_hash = sha256(source)
    destination_hash = sha256(destination)
    if source_hash != destination_hash:
        raise RuntimeError(f"Copy hash mismatch: {source}")
    return {
        "source": str(source),
        "recovery_copy": str(destination),
        "bytes": source.stat().st_size,
        "sha256": source_hash,
    }


def sqlite_online_backup(source: Path, destination: Path) -> dict[str, Any]:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(f"file:{source}?mode=ro", uri=True) as src:
        with sqlite3.connect(destination) as dst:
            src.backup(dst)
            integrity = dst.execute("PRAGMA integrity_check").fetchone()[0]
            if integrity != "ok":
                raise RuntimeError(f"SQLite integrity check failed: {integrity}")
    return {
        "source": str(source),
        "recovery_copy": str(destination),
        "bytes": destination.stat().st_size,
        "sha256": sha256(destination),
        "integrity_check": "ok",
        "method": "sqlite_online_backup",
    }


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--recovery", type=Path, required=True)
    args = parser.parse_args()

    repo = args.repo.resolve()
    evidence = args.evidence.resolve()
    recovery = args.recovery.resolve()
    if repo != Path(r"D:\桌面\meetlily").resolve():
        raise RuntimeError(f"Unexpected repository path: {repo}")
    if not str(recovery).lower().startswith("d:\\meetilyrecovery\\"):
        raise RuntimeError(f"Recovery directory must be under D:\\MeetilyRecovery: {recovery}")

    evidence.mkdir(parents=True, exist_ok=True)
    recovery.mkdir(parents=True, exist_ok=True)
    started = time.time()
    now = datetime.now(timezone.utc).astimezone().isoformat()

    status = git(repo, "-c", "core.quotePath=false", "status", "--porcelain=v1", "-uall")
    status_lines = [line for line in status.splitlines() if line]
    status_digest = hashlib.sha256(status.encode("utf-8")).hexdigest().upper()
    source_state = {
        "captured_at": now,
        "repo": str(repo),
        "head": git(repo, "rev-parse", "HEAD"),
        "branch": git(repo, "branch", "--show-current"),
        "status_entry_count": len(status_lines),
        "status_sha256": status_digest,
        "tracked_dirty_count": sum(line[:2] != "??" for line in status_lines),
        "untracked_count": sum(line[:2] == "??" for line in status_lines),
        "baseline_commit": "cb98b28a0f466e4a508a2199c11fedeef552dfdc",
    }
    write_json(evidence / "00-source-state.json", source_state)

    inputs = []
    for role, path, expected_hash in EXPECTED_INPUTS:
        if not path.is_file():
            raise FileNotFoundError(path)
        actual_hash = sha256(path)
        if actual_hash != expected_hash:
            raise RuntimeError(f"Input hash mismatch for {role}: {actual_hash}")
        duration, duration_method = media_duration(path)
        inputs.append(
            {
                "role": role,
                "path": str(path),
                "bytes": path.stat().st_size,
                "sha256": actual_hash,
                "duration_seconds": duration,
                "duration_method": duration_method,
            }
        )
    write_json(
        evidence / "01-input-manifest.json",
        {"captured_at": now, "input_count": len(inputs), "inputs": inputs},
    )

    try:
        import psutil  # type: ignore

        memory = psutil.virtual_memory()
        disk = psutil.disk_usage("D:\\")
        environment = {
            "captured_at": now,
            "platform": platform.platform(),
            "python": sys.version,
            "cpu_logical": psutil.cpu_count(logical=True),
            "memory_total_bytes": memory.total,
            "memory_available_bytes": memory.available,
            "d_drive_total_bytes": disk.total,
            "d_drive_free_bytes": disk.free,
        }
    except Exception as exc:  # pragma: no cover
        environment = {"captured_at": now, "platform": platform.platform(), "error": str(exc)}
    write_json(evidence / "02-environment.json", environment)

    release_dir = repo / "target" / "release"
    release_files = [
        release_dir / "meetily.exe",
        release_dir / "llama-helper.exe",
        release_dir / "ffmpeg.exe",
        release_dir / "app_lib.dll",
        release_dir / "bundle" / "nsis" / "meetily_0.4.0_x64-setup.exe",
    ]
    release_records = []
    for source in release_files:
        if not source.is_file():
            raise FileNotFoundError(source)
        release_records.append(
            copy_with_hash(source, recovery / "release" / source.name)
        )

    app_data = Path(os.environ["APPDATA"]) / "com.meetily.ai"
    database = app_data / "meeting_minutes.sqlite"
    database_record = sqlite_online_backup(
        database, recovery / "user-data" / "meeting_minutes.sqlite"
    )

    settings_records = []
    for name in (
        "preferences.json",
        "recording_preferences.json",
        "summary-template-preferences.v1.json",
        "ui-locale.json",
    ):
        source = app_data / name
        if source.is_file():
            settings_records.append(
                copy_with_hash(source, recovery / "user-data" / name)
            )

    model_root = app_data / "models"
    model_records = []
    if model_root.is_dir():
        for source in sorted(path for path in model_root.rglob("*") if path.is_file()):
            model_records.append(
                {
                    "relative_path": source.relative_to(model_root).as_posix(),
                    "bytes": source.stat().st_size,
                    "sha256": sha256(source),
                }
            )

    data_integrity = {
        "captured_at": now,
        "release_files": release_records,
        "database": database_record,
        "settings": settings_records,
        "models": {
            "root": str(model_root),
            "file_count": len(model_records),
            "total_bytes": sum(record["bytes"] for record in model_records),
            "files": model_records,
        },
        "elapsed_seconds": time.time() - started,
    }
    write_json(evidence / "05-data-integrity.json", data_integrity)

    summary = {
        "result": "PASS",
        "evidence": str(evidence),
        "recovery": str(recovery),
        "inputs": len(inputs),
        "release_files": len(release_records),
        "settings": len(settings_records),
        "models": len(model_records),
        "elapsed_seconds": data_integrity["elapsed_seconds"],
    }
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
