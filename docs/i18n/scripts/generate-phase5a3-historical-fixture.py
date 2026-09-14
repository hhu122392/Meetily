#!/usr/bin/env python3
"""Create a deterministic, secret-free Meetily v0.3 historical data seed.

The database intentionally contains only the official v0.3 initial schema.  The
official v0.3 Windows application must open it and execute its embedded SQLx
migrations before the richer upgrade fixture is produced.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import struct
import sys
import wave
from pathlib import Path


EXPECTED_INITIAL_MIGRATION_SHA256 = (
    "E7A9A6C41DD6E91A50EFD6C9C609B3F90B5DEC99B0266040B4FFE89BCCDED8AD"
)
OFFICIAL_V030_COMMIT = "91b0c0985932d0797e249033601afa14f22ee3d3"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def write_deterministic_wav(path: Path) -> None:
    """Write one second of low-amplitude deterministic PCM, not captured audio."""
    path.parent.mkdir(parents=True, exist_ok=True)
    sample_rate = 8_000
    frames = bytearray()
    for index in range(sample_rate):
        sample = 1_200 if (index // 40) % 2 == 0 else -1_200
        frames.extend(struct.pack("<h", sample))
    with wave.open(str(path), "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(sample_rate)
        output.writeframes(bytes(frames))


def create_database(db_path: Path, initial_schema: str) -> None:
    if db_path.exists():
        db_path.unlink()
    db_path.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(db_path)
    try:
        connection.execute("PRAGMA foreign_keys = ON")
        connection.executescript(initial_schema)
        connection.executemany(
            "INSERT INTO meetings(id,title,created_at,updated_at) VALUES(?,?,?,?)",
            [
                (
                    "meeting-en-001",
                    "Weekly Product Review",
                    "2025-09-18T09:00:00Z",
                    "2025-09-18T09:42:00Z",
                ),
                (
                    "meeting-zh-001",
                    "项目复盘会议",
                    "2025-09-19T02:30:00Z",
                    "2025-09-19T03:15:00Z",
                ),
                (
                    "meeting-mixed-001",
                    "Q4 Roadmap / 第四季度路线图",
                    "2025-09-20T12:00:00Z",
                    "2025-09-20T12:55:00Z",
                ),
            ],
        )
        connection.executemany(
            """
            INSERT INTO transcripts(
              id,meeting_id,transcript,timestamp,summary,action_items,key_points
            ) VALUES(?,?,?,?,?,?,?)
            """,
            [
                (
                    "segment-en-001",
                    "meeting-en-001",
                    "Alex: We will freeze the English baseline before translation.",
                    "2025-09-18T09:00:05Z",
                    "Baseline scope confirmed.",
                    '["Export the English catalog","Run the source audit"]',
                    '["No source-string changes after freeze"]',
                ),
                (
                    "segment-en-002",
                    "meeting-en-001",
                    "Morgan: Every release gate needs evidence and an owner.",
                    "2025-09-18T09:04:12Z",
                    None,
                    None,
                    None,
                ),
                (
                    "segment-zh-001",
                    "meeting-zh-001",
                    "刘晨：阶段验收必须保留数据库、录音和设置，不能只看界面。",
                    "2025-09-19T02:30:08Z",
                    "确认数据保留验收范围。",
                    '["核对会议记录","校验升级后哈希"]',
                    '["不得读取真实用户数据"]',
                ),
                (
                    "segment-zh-002",
                    "meeting-zh-001",
                    "王宁：失败注入后要证明旧数据仍可恢复。",
                    "2025-09-19T02:35:18Z",
                    None,
                    None,
                    None,
                ),
                (
                    "segment-mixed-001",
                    "meeting-mixed-001",
                    "Owner: release team；状态：Ready for sandbox validation ✅",
                    "2025-09-20T12:00:03Z",
                    "中英文混排与标点必须保持。",
                    '["Validate upgrade","验证回滚"]',
                    '["UTF-8","stable IDs"]',
                ),
            ],
        )
        connection.executemany(
            """
            INSERT INTO summary_processes(
              meeting_id,status,created_at,updated_at,error,result,start_time,
              end_time,chunk_count,processing_time,metadata
            ) VALUES(?,?,?,?,?,?,?,?,?,?,?)
            """,
            [
                (
                    "meeting-en-001",
                    "completed",
                    "2025-09-18T09:42:00Z",
                    "2025-09-18T09:42:08Z",
                    None,
                    json.dumps(
                        {
                            "summary": "The team froze the English catalog.",
                            "actionItems": ["Audit every release gate."],
                        },
                        ensure_ascii=False,
                        separators=(",", ":"),
                    ),
                    "2025-09-18T09:42:00Z",
                    "2025-09-18T09:42:08Z",
                    2,
                    8.25,
                    '{"fixture":"phase5a3","language":"en"}',
                ),
                (
                    "meeting-zh-001",
                    "completed",
                    "2025-09-19T03:15:00Z",
                    "2025-09-19T03:15:06Z",
                    None,
                    json.dumps(
                        {
                            "summary": "团队确认升级与回滚必须保留语义数据。",
                            "actionItems": ["执行沙箱升级矩阵"],
                        },
                        ensure_ascii=False,
                        separators=(",", ":"),
                    ),
                    "2025-09-19T03:15:00Z",
                    "2025-09-19T03:15:06Z",
                    2,
                    6.5,
                    '{"fixture":"phase5a3","language":"zh-CN"}',
                ),
            ],
        )
        connection.executemany(
            """
            INSERT INTO transcript_chunks(
              meeting_id,meeting_name,transcript_text,model,model_name,
              chunk_size,overlap,created_at
            ) VALUES(?,?,?,?,?,?,?,?)
            """,
            [
                (
                    "meeting-en-001",
                    "Weekly Product Review",
                    "English baseline and evidence ownership.",
                    "fixture-local",
                    "deterministic-test-model",
                    900,
                    90,
                    "2025-09-18T09:42:00Z",
                ),
                (
                    "meeting-zh-001",
                    "项目复盘会议",
                    "数据保留、失败保护与恢复验证。",
                    "fixture-local",
                    "deterministic-test-model",
                    900,
                    90,
                    "2025-09-19T03:15:00Z",
                ),
            ],
        )
        connection.execute(
            """
            INSERT INTO settings(
              id,provider,model,whisperModel,groqApiKey,openaiApiKey,
              anthropicApiKey,ollamaApiKey
            ) VALUES(?,?,?,?,NULL,NULL,NULL,NULL)
            """,
            ("settings-main", "builtin-ai", "gemma3:1b", "large-v3"),
        )
        connection.execute(
            """
            INSERT INTO transcript_settings(
              id,provider,model,whisperApiKey,deepgramApiKey,
              elevenLabsApiKey,groqApiKey,openaiApiKey
            ) VALUES(?,?,?,NULL,NULL,NULL,NULL,NULL)
            """,
            ("transcript-settings-main", "parakeet", "parakeet-tdt-0.6b-v2"),
        )
        connection.commit()
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"fixture database integrity_check failed: {integrity}")
    finally:
        connection.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    repo_root = args.repo_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    migration_path = (
        repo_root
        / "frontend"
        / "src-tauri"
        / "migrations"
        / "20250916100000_initial_schema.sql"
    )
    if sha256(migration_path) != EXPECTED_INITIAL_MIGRATION_SHA256:
        raise RuntimeError("initial migration hash does not match the frozen v0.3 source")

    app_data = output / "app-data"
    database_path = app_data / "meeting_minutes.sqlite"
    create_database(database_path, migration_path.read_text(encoding="utf-8"))

    write_json(
        app_data / "recording_preferences.json",
        {
            "preferences": {
                "save_folder": "C:\\Users\\WDAGUtilityAccount\\Music\\meetily-recordings",
                "auto_save": True,
                "file_format": "wav",
                "preferred_mic_device": "Phase5A3 Fixture Microphone",
                "preferred_system_device": "Phase5A3 Fixture System Audio",
            }
        },
    )
    write_json(
        app_data / "onboarding-status.json",
        {
            "status": {
                "version": "1.0",
                "completed": True,
                "current_step": 3,
                "model_status": {
                    "parakeet": "not_downloaded",
                    "summary": "not_downloaded",
                },
                "last_updated": "2025-09-20T13:00:00Z",
            }
        },
    )
    write_json(app_data / "preferences.json", {"show_recording_notification": False})
    (app_data / "phase5a3-preservation-sentinel.txt").write_text(
        "Meetily Stage 5A-3 secret-free preservation sentinel\n",
        encoding="utf-8",
        newline="\n",
    )
    write_json(
        output / "config" / "meetily" / "notifications.json",
        {
            "recording_notifications": True,
            "time_based_reminders": False,
            "meeting_reminders": True,
            "respect_do_not_disturb": True,
            "notification_sound": False,
            "system_permission_granted": False,
            "consent_given": True,
            "manual_dnd_mode": False,
            "notification_preferences": {
                "show_recording_started": True,
                "show_recording_stopped": True,
                "show_recording_paused": True,
                "show_recording_resumed": True,
                "show_transcription_complete": True,
                "show_meeting_reminders": False,
                "show_system_errors": True,
                "meeting_reminder_minutes": [15, 5],
            },
        },
    )

    recordings = output / "recordings"
    recording_specs = [
        (
            "Weekly Product Review_2025-09-18_090000",
            "meeting-en-001",
            "Weekly Product Review",
            [
                {
                    "id": "file-segment-en-001",
                    "text": "This is deterministic fixture audio metadata.",
                    "audio_start_time": 0.0,
                    "audio_end_time": 1.0,
                    "duration": 1.0,
                    "display_time": "[00:00]",
                    "confidence": 0.99,
                    "sequence_id": 1,
                }
            ],
        ),
        (
            "项目复盘会议_2025-09-19_103000",
            "meeting-zh-001",
            "项目复盘会议",
            [
                {
                    "id": "file-segment-zh-001",
                    "text": "这是确定性的脱敏音频夹具。",
                    "audio_start_time": 0.0,
                    "audio_end_time": 1.0,
                    "duration": 1.0,
                    "display_time": "[00:00]",
                    "confidence": 0.98,
                    "sequence_id": 1,
                }
            ],
        ),
    ]
    for folder_name, meeting_id, meeting_name, segments in recording_specs:
        folder = recordings / folder_name
        write_deterministic_wav(folder / "audio.wav")
        write_json(
            folder / "metadata.json",
            {
                "version": "1.0",
                "meeting_id": meeting_id,
                "meeting_name": meeting_name,
                "created_at": "2025-09-18T09:00:00Z",
                "completed_at": "2025-09-18T09:00:01Z",
                "duration_seconds": 1.0,
                "devices": {
                    "microphone": "Phase5A3 Fixture Microphone",
                    "system_audio": "Phase5A3 Fixture System Audio",
                },
                "audio_file": "audio.wav",
                "transcript_file": "transcripts.json",
                "sample_rate": 8000,
                "status": "completed",
            },
        )
        write_json(folder / "transcripts.json", {"version": "1.0", "segments": segments})

    tracked_files = sorted(
        path
        for path in output.rglob("*")
        if path.is_file() and path.name != "fixture-manifest.json"
    )
    manifest = {
        "schemaVersion": 1,
        "phase": "15.11 / Stage 5A-3",
        "fixtureKind": "synthetic-redacted-v030-initial-schema",
        "officialTag": "v0.3.0",
        "officialTagCommit": OFFICIAL_V030_COMMIT,
        "containsRealUserData": False,
        "containsSecrets": False,
        "requiresOfficialV030Migration": True,
        "initialMigrationSha256": EXPECTED_INITIAL_MIGRATION_SHA256,
        "expectedSeedCounts": {
            "meetings": 3,
            "transcripts": 5,
            "summary_processes": 2,
            "transcript_chunks": 2,
            "settings": 1,
            "transcript_settings": 1,
            "recordingFolders": 2,
        },
        "files": [
            {
                "path": path.relative_to(output).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
            }
            for path in tracked_files
        ],
    }
    write_json(output / "fixture-manifest.json", manifest)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    print(json.dumps(manifest, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
