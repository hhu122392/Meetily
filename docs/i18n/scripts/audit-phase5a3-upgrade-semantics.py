#!/usr/bin/env python3
"""Compare Meetily data semantics across v0.3, candidate upgrade, and rollback."""

from __future__ import annotations

import argparse
import hashlib
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any


DATABASE_RELATIVE_PATH = Path("app-data/meeting_minutes.sqlite")
DATABASE_SUFFIXES = (
    "meeting_minutes.sqlite",
    "meeting_minutes.sqlite-wal",
    "meeting_minutes.sqlite-shm",
)
STABLE_JSON_FILES = (
    "app-data/recording_preferences.json",
    "app-data/onboarding-status.json",
    "app-data/preferences.json",
)
STABLE_TEXT_FILES = ("app-data/phase5a3-preservation-sentinel.txt",)
EXPECTED_COUNTS = {
    "meetings": 3,
    "transcripts": 5,
    "summary_processes": 2,
    "transcript_chunks": 2,
    "settings": 1,
    "transcript_settings": 1,
    "meeting_notes": 2,
    "_sqlx_migrations": 10,
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def json_safe(value: Any) -> Any:
    if isinstance(value, bytes):
        return {"$bytesHex": value.hex().upper()}
    return value


def database_snapshot(root: Path) -> dict[str, Any]:
    path = root / DATABASE_RELATIVE_PATH
    if not path.is_file():
        raise RuntimeError(f"database snapshot is missing: {path}")
    connection = sqlite3.connect(path)
    connection.row_factory = sqlite3.Row
    try:
        integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
        table_names = [
            row[0]
            for row in connection.execute(
                """
                SELECT name FROM sqlite_master
                WHERE type='table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name
                """
            )
        ]
        tables: dict[str, Any] = {}
        for table in table_names:
            columns_info = list(connection.execute(f'PRAGMA table_info("{table}")'))
            columns = [row[1] for row in columns_info]
            primary_key = [
                row[1] for row in sorted(columns_info, key=lambda item: item[5]) if row[5] > 0
            ]
            order_columns = primary_key or columns
            order_clause = ",".join(f'"{column}"' for column in order_columns)
            rows = [
                {column: json_safe(row[column]) for column in columns}
                for row in connection.execute(
                    f'SELECT * FROM "{table}" ORDER BY {order_clause}'
                )
            ]
            tables[table] = {
                "columns": columns,
                "primaryKey": primary_key,
                "rowCount": len(rows),
                "rows": rows,
            }
        canonical = json.dumps(tables, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        return {
            "path": str(path),
            "bytes": path.stat().st_size,
            "fileSha256": sha256(path),
            "integrityCheck": integrity,
            "semanticSha256": hashlib.sha256(canonical.encode("utf-8")).hexdigest().upper(),
            "tables": tables,
        }
    finally:
        connection.close()


def file_snapshot(root: Path) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for path in sorted(item for item in root.rglob("*") if item.is_file()):
        if path.name in DATABASE_SUFFIXES:
            continue
        relative = path.relative_to(root).as_posix()
        result[relative] = {"bytes": path.stat().st_size, "sha256": sha256(path)}
    return result


def stable_notification_projection(root: Path) -> dict[str, Any]:
    value = json.loads((root / "config/meetily/notifications.json").read_text(encoding="utf-8"))
    value.pop("system_permission_granted", None)
    return value


def compare_protected_files(
    before: dict[str, dict[str, Any]], after: dict[str, dict[str, Any]]
) -> dict[str, Any]:
    missing = sorted(path for path in before if path not in after)
    changed = sorted(
        path
        for path in before
        if path in after and before[path]["sha256"] != after[path]["sha256"]
    )
    added = sorted(path for path in after if path not in before)
    return {"missing": missing, "changed": changed, "added": added}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--before", required=True, type=Path)
    parser.add_argument("--after", required=True, type=Path)
    parser.add_argument("--rollback", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    roots = {
        "before": args.before.resolve(),
        "after": args.after.resolve(),
        "rollback": args.rollback.resolve(),
    }
    databases = {name: database_snapshot(root) for name, root in roots.items()}
    files = {name: file_snapshot(root) for name, root in roots.items()}
    upgrade_files = compare_protected_files(files["before"], files["after"])
    rollback_files = compare_protected_files(files["before"], files["rollback"])

    # Runtime-managed notification permission may vary by disposable OS.  Every
    # user-controlled notification setting is compared after projecting it out.
    for comparison in (upgrade_files, rollback_files):
        if "config/meetily/notifications.json" in comparison["changed"]:
            comparison["changed"].remove("config/meetily/notifications.json")

    row_counts_correct = all(
        databases["before"]["tables"].get(table, {}).get("rowCount") == expected
        for table, expected in EXPECTED_COUNTS.items()
    )
    database_upgrade_equal = databases["before"]["tables"] == databases["after"]["tables"]
    database_rollback_equal = (
        databases["before"]["tables"] == databases["rollback"]["tables"]
    )
    stable_files_present = all(
        all((root / relative).is_file() for relative in STABLE_JSON_FILES + STABLE_TEXT_FILES)
        for root in roots.values()
    )
    notification_upgrade_equal = stable_notification_projection(
        roots["before"]
    ) == stable_notification_projection(roots["after"])
    notification_rollback_equal = stable_notification_projection(
        roots["before"]
    ) == stable_notification_projection(roots["rollback"])
    integrity_ok = all(value["integrityCheck"] == "ok" for value in databases.values())
    secret_columns_empty = True
    for table_name in ("settings", "transcript_settings"):
        for row in databases["before"]["tables"][table_name]["rows"]:
            for column, value in row.items():
                if column.lower().endswith("apikey") and value is not None:
                    secret_columns_empty = False
    assertions = {
        "allDatabaseIntegrityChecksPass": integrity_ok,
        "expectedHistoricalRowCountsPresent": row_counts_correct,
        "fixtureApiKeyColumnsAreEmpty": secret_columns_empty,
        "upgradePreservesAllDatabaseSchemasAndRows": database_upgrade_equal,
        "rollbackRestoresAllDatabaseSchemasAndRows": database_rollback_equal,
        "stablePreferenceAndSentinelFilesPresent": stable_files_present,
        "upgradePreservesEveryPreexistingNonDatabaseFile": not upgrade_files["missing"]
        and not upgrade_files["changed"],
        "rollbackRestoresEveryPreexistingNonDatabaseFile": not rollback_files["missing"]
        and not rollback_files["changed"],
        "upgradePreservesUserControlledNotificationSettings": notification_upgrade_equal,
        "rollbackRestoresUserControlledNotificationSettings": notification_rollback_equal,
    }
    report = {
        "schemaVersion": 1,
        "phase": "15.11 / Stage 5A-3",
        "scope": "Semantic equality of redacted official-v0.3-migrated data before upgrade, after 0.4.1 candidate launch, and after approved rollback",
        "containsRealUserData": False,
        "databaseSnapshots": databases,
        "nonDatabaseFileComparisons": {
            "upgrade": upgrade_files,
            "rollback": rollback_files,
        },
        "assertions": assertions,
        "passed": all(assertions.values()),
    }
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
