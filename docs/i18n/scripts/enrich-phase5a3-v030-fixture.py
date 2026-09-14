#!/usr/bin/env python3
"""Enrich a database that the official v0.3 binary has genuinely migrated."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sqlite3
import sys
from pathlib import Path


EXPECTED_MIGRATIONS = 10
SANDBOX_RECORDINGS = Path(
    r"C:\Users\WDAGUtilityAccount\Music\meetily-recordings"
)


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


def table_columns(connection: sqlite3.Connection, table: str) -> list[str]:
    return [row[1] for row in connection.execute(f'PRAGMA table_info("{table}")')]


def apply_frozen_sqlx_migrations(
    connection: sqlite3.Connection, migration_source_dir: Path
) -> list[dict[str, object]]:
    """Apply the frozen SQL verbatim and reproduce SQLx 0.8.6 bookkeeping."""
    connection.execute(
        """
        CREATE TABLE IF NOT EXISTS _sqlx_migrations (
          version BIGINT PRIMARY KEY,
          description TEXT NOT NULL,
          installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
          success BOOLEAN NOT NULL,
          checksum BLOB NOT NULL,
          execution_time BIGINT NOT NULL
        )
        """
    )
    applied: list[dict[str, object]] = []
    for path in sorted(migration_source_dir.glob("*.sql")):
        version_text, description_file = path.name.split("_", 1)
        version = int(version_text)
        description = description_file.removesuffix(".sql").replace("_", " ")
        raw_sql = path.read_bytes()
        sql = raw_sql.decode("utf-8")
        checksum = hashlib.sha384(sql.encode("utf-8")).digest()
        connection.executescript(sql)
        connection.execute(
            """
            INSERT INTO _sqlx_migrations(
              version,description,success,checksum,execution_time
            ) VALUES(?,?,TRUE,?,0)
            """,
            (version, description, checksum),
        )
        connection.commit()
        applied.append(
            {
                "version": version,
                "description": description,
                "source": path.name,
                "sourceSha256": hashlib.sha256(raw_sql).hexdigest().upper(),
                "sqlxSha384": checksum.hex().upper(),
            }
        )
    return applied


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--migrated-app-data", required=True, type=Path)
    parser.add_argument("--seed-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--migration-source-dir", type=Path)
    args = parser.parse_args()

    source_app_data = args.migrated_app_data.resolve()
    seed_root = args.seed_root.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    output_app_data = output / "app-data"
    if output_app_data.exists():
        shutil.rmtree(output_app_data)
    shutil.copytree(source_app_data, output_app_data)
    for directory in ("recordings", "config"):
        destination = output / directory
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(seed_root / directory, destination)

    database_path = output_app_data / "meeting_minutes.sqlite"
    connection = sqlite3.connect(database_path)
    connection.row_factory = sqlite3.Row
    try:
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"official-migrated database integrity failed: {integrity}")
        has_sqlx_table = connection.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'"
        ).fetchone()[0] == 1
        source_applied_migrations: list[dict[str, object]] = []
        if not has_sqlx_table:
            if args.migration_source_dir is None:
                raise RuntimeError(
                    "_sqlx_migrations is absent and --migration-source-dir was not provided"
                )
            source_applied_migrations = apply_frozen_sqlx_migrations(
                connection, args.migration_source_dir.resolve()
            )
        migration_rows = list(
            connection.execute(
                'SELECT version,description,success,checksum FROM "_sqlx_migrations" ORDER BY version'
            )
        )
        if len(migration_rows) != EXPECTED_MIGRATIONS:
            raise RuntimeError(
                f"expected {EXPECTED_MIGRATIONS} SQLx migrations, got {len(migration_rows)}"
            )
        if any(row["success"] != 1 for row in migration_rows):
            raise RuntimeError("official v0.3 reported an unsuccessful SQLx migration")

        required_columns = {
            "meetings": {"folder_path"},
            "transcripts": {
                "audio_start_time",
                "audio_end_time",
                "duration",
                "speaker",
            },
            "summary_processes": {"result_backup", "result_backup_timestamp"},
            "settings": {
                "openRouterApiKey",
                "ollamaEndpoint",
                "customOpenAIConfig",
                "geminiApiKey",
            },
            "meeting_notes": {"notes_markdown", "notes_json"},
        }
        for table, columns in required_columns.items():
            actual = set(table_columns(connection, table))
            missing = columns - actual
            if missing:
                raise RuntimeError(f"{table} missing migrated columns: {sorted(missing)}")

        folder_en = str(SANDBOX_RECORDINGS / "Weekly Product Review_2025-09-18_090000")
        folder_zh = str(SANDBOX_RECORDINGS / "项目复盘会议_2025-09-19_103000")
        connection.execute(
            "UPDATE meetings SET folder_path=? WHERE id='meeting-en-001'", (folder_en,)
        )
        connection.execute(
            "UPDATE meetings SET folder_path=? WHERE id='meeting-zh-001'", (folder_zh,)
        )
        connection.execute(
            "UPDATE meetings SET folder_path=NULL WHERE id='meeting-mixed-001'"
        )
        timing = [
            (0.0, 4.2, 4.2, "mic", "segment-en-001"),
            (4.2, 9.6, 5.4, "system", "segment-en-002"),
            (0.0, 5.1, 5.1, "mic", "segment-zh-001"),
            (5.1, 9.8, 4.7, "system", "segment-zh-002"),
            (0.0, 6.25, 6.25, "mic", "segment-mixed-001"),
        ]
        connection.executemany(
            """
            UPDATE transcripts
            SET audio_start_time=?,audio_end_time=?,duration=?,speaker=?
            WHERE id=?
            """,
            timing,
        )
        connection.execute(
            """
            UPDATE summary_processes
            SET result_backup=?,result_backup_timestamp=?
            WHERE meeting_id='meeting-en-001'
            """,
            (
                '{"summary":"Previous deterministic English summary."}',
                "2025-09-18T09:41:59Z",
            ),
        )
        connection.execute(
            """
            UPDATE settings
            SET ollamaEndpoint='http://127.0.0.1:11434',
                openRouterApiKey=NULL,
                customOpenAIConfig=NULL,
                geminiApiKey=NULL
            WHERE id='settings-main'
            """
        )
        connection.executemany(
            """
            INSERT INTO meeting_notes(
              meeting_id,notes_markdown,notes_json,created_at,updated_at
            ) VALUES(?,?,?,?,?)
            """,
            [
                (
                    "meeting-en-001",
                    "# Release notes\n\n- Freeze the English baseline.\n- Preserve user data.",
                    '{"type":"doc","content":[{"type":"paragraph","text":"Preserve user data"}]}',
                    "2025-09-18T09:10:00Z",
                    "2025-09-18T09:40:00Z",
                ),
                (
                    "meeting-zh-001",
                    "# 验收记录\n\n- 校验中英文文本。\n- 验证失败恢复。",
                    '{"type":"doc","content":[{"type":"paragraph","text":"验证失败恢复"}]}',
                    "2025-09-19T02:40:00Z",
                    "2025-09-19T03:10:00Z",
                ),
            ],
        )
        connection.commit()
        connection.execute("PRAGMA wal_checkpoint(TRUNCATE)")
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        if integrity != "ok":
            raise RuntimeError(f"enriched database integrity failed: {integrity}")
        counts = {
            table: connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
            for table in (
                "meetings",
                "transcripts",
                "summary_processes",
                "transcript_chunks",
                "settings",
                "transcript_settings",
                "meeting_notes",
                "_sqlx_migrations",
            )
        }
        migrations = [
            {
                "version": row["version"],
                "description": row["description"],
                "success": bool(row["success"]),
                "checksumHex": bytes(row["checksum"]).hex().upper(),
            }
            for row in migration_rows
        ]
    finally:
        connection.close()

    files = sorted(
        path
        for path in output.rglob("*")
        if path.is_file() and path.name != "fixture-manifest.json"
    )
    manifest = {
        "schemaVersion": 1,
        "phase": "15.11 / Stage 5A-3",
        "fixtureKind": (
            "synthetic-redacted-frozen-v030-source-migrated-and-enriched"
            if source_applied_migrations
            else "synthetic-redacted-official-v030-runtime-migrated-and-enriched"
        ),
        "containsRealUserData": False,
        "containsSecrets": False,
        "officialV030MigrationCount": len(migrations),
        "officialV030Migrations": migrations,
        "migrationExecution": {
            "mode": (
                "frozen-source-sqlx-compatible"
                if source_applied_migrations
                else "preexisting-runtime-migrated-database"
            ),
            "runtimeMigrationClaimed": not source_applied_migrations,
            "sourceAppliedMigrations": source_applied_migrations,
        },
        "tableCounts": counts,
        "databaseSha256": sha256(database_path),
        "files": [
            {
                "path": path.relative_to(output).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
            }
            for path in files
        ],
    }
    write_json(output / "fixture-manifest.json", manifest)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    print(json.dumps(manifest, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
