#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sqlite3
import sys


def digest(value: object) -> str:
    payload = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("Usage: sqlite-audit.py <database> <output.json>")
    database = Path(sys.argv[1])
    output = Path(sys.argv[2])
    report: dict[str, object] = {
        "exists": database.is_file(),
        "path_name": database.name,
        "integrity": "NOT_RUN",
        "foreign_key_check": "NOT_RUN",
        "foreign_key_violation_count": None,
        "foreign_key_violations_sha256": None,
        "user_version": None,
        "migrations": [],
        "objects": [],
        "tables": [],
    }
    if database.is_file():
        connection = sqlite3.connect(f"file:{database.as_posix()}?mode=ro", uri=True, timeout=10)
        try:
            connection.execute("BEGIN")
            integrity_rows = [str(row[0]) for row in connection.execute("PRAGMA integrity_check")]
            report["integrity"] = "ok" if integrity_rows == ["ok"] else digest(integrity_rows)
            foreign_key_rows = [list(row) for row in connection.execute("PRAGMA foreign_key_check")]
            report["foreign_key_check"] = "ok" if not foreign_key_rows else "violations"
            report["foreign_key_violation_count"] = len(foreign_key_rows)
            report["foreign_key_violations_sha256"] = digest(foreign_key_rows)
            report["user_version"] = int(connection.execute("PRAGMA user_version").fetchone()[0])
            tables = [
                str(row[0])
                for row in connection.execute(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
                )
            ]
            if "_sqlx_migrations" in tables:
                report["migrations"] = [
                    {"version": int(row[0]), "description": str(row[1]), "success": int(row[2])}
                    for row in connection.execute(
                        "SELECT version, description, success FROM _sqlx_migrations ORDER BY version"
                    )
                ]
            report["objects"] = [
                {
                    "type": str(row[0]),
                    "name": str(row[1]),
                    "table_name": str(row[2]),
                    "sql_sha256": hashlib.sha256(str(row[3]).encode("utf-8")).hexdigest(),
                }
                for row in connection.execute(
                    """
                    SELECT type, name, tbl_name, sql
                    FROM sqlite_master
                    WHERE type IN ('table', 'index', 'trigger', 'view')
                      AND name NOT LIKE 'sqlite_%'
                      AND sql IS NOT NULL
                    ORDER BY type, name
                    """
                )
            ]
            table_reports: list[dict[str, object]] = []
            for table in tables:
                escaped = '"' + table.replace('"', '""') + '"'
                schema = connection.execute(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name=?", (table,)
                ).fetchone()[0]
                rows = []
                for row in connection.execute(f"SELECT * FROM {escaped}"):
                    normalized = [
                        {"blob_sha256": hashlib.sha256(value).hexdigest()} if isinstance(value, bytes) else value
                        for value in row
                    ]
                    rows.append(digest(normalized))
                rows.sort()
                table_reports.append(
                    {
                        "name": table,
                        "row_count": len(rows),
                        "schema_sha256": hashlib.sha256(str(schema).encode("utf-8")).hexdigest(),
                        "rowset_sha256": digest(rows),
                    }
                )
            report["tables"] = table_reports
        finally:
            connection.close()
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(output), "integrity": report["integrity"]}, ensure_ascii=False))
    return 0 if report["integrity"] == "ok" and report["foreign_key_check"] == "ok" else 1


if __name__ == "__main__":
    raise SystemExit(main())
