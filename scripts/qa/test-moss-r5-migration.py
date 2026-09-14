#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sqlite3
import sys
import platform
import traceback
from typing import Any


R4_MIGRATION = "20260831000000_add_moss_alignment_provenance.sql"
R5_MIGRATION = "20260831010000_add_moss_audio_token_provenance.sql"
ALIGNMENT_TABLE = "moss_candidate_segment_alignment"
DIAGNOSTICS_TABLE = "moss_run_diagnostics"
NEW_R5_TABLES = (
    "moss_audio_token_runs",
    "moss_audio_token_source_chunks",
    "moss_audio_tokens",
    "moss_candidate_audio_token_boundary",
    "moss_machine_term_correction_source",
)

# These hashes are over the exact CREATE TABLE SQL stored by SQLite in
# sqlite_master. They are an explicit schema allowlist, not a loose
# "the schema changed" check.
EXPECTED_SCHEMA_SHA256 = {
    "r4_alignment": "8a61f71877ef30aeafbf6adfa43f9796a86888194e5216edbc37889e6f8fac77",
    "r5_alignment": "9912ea13b1558d18380171fafd5e63d082fa11b775251f32eca57504a3a3fbe7",
    "run_diagnostics": "a711327ab0154dc97860ffc82d01619d368276e871e9711864bda3c48462ac28",
    "moss_audio_token_runs": "797aea33530d9375742356576251b840df066e48a6a3577599137d62310dca2b",
    "moss_audio_token_source_chunks": "616076658e481f698f8922bf548e2b6e2ed8dfb1aee132967a86da668951341c",
    "moss_audio_tokens": "8f9410a8957d0bc86ad7376572e9ba38275d0c63e18b3ee95615deed1a9c7172",
    "moss_candidate_audio_token_boundary": "417ccaf096d9479db863114157980ba2053d0a74ee36740e8c66582efc51be7d",
    "moss_machine_term_correction_source": "aab106f9021cd05351b0eee2e2ac36631ccb7e81fc6b36daea640dfc6a83588e",
}
EXPECTED_ALIGNMENT_INDEX_SHA256 = (
    "8c2acc79cca466b6989b8fec6fd45b65aaa9990861cb9179e015d559e8c57cab"
)


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def canonical_sha256(value: Any) -> str:
    payload = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return sha256_bytes(payload)


def file_evidence(path: Path) -> dict[str, Any]:
    payload = path.read_bytes()
    return {
        "name": path.name,
        "bytes": len(payload),
        "sha256": sha256_bytes(payload),
    }


def schema_sql(connection: sqlite3.Connection, table: str) -> str:
    row = connection.execute(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?", (table,)
    ).fetchone()
    if row is None or row[0] is None:
        raise AssertionError(f"Required table is missing: {table}")
    return str(row[0])


def schema_sha256(connection: sqlite3.Connection, table: str) -> str:
    return sha256_bytes(schema_sql(connection, table).encode("utf-8"))


def object_sha256(connection: sqlite3.Connection, object_type: str, name: str) -> str:
    row = connection.execute(
        "SELECT sql FROM sqlite_master WHERE type = ? AND name = ?", (object_type, name)
    ).fetchone()
    if row is None or row[0] is None:
        raise AssertionError(f"Required {object_type} is missing: {name}")
    return sha256_bytes(str(row[0]).encode("utf-8"))


def table_snapshot(
    connection: sqlite3.Connection, table: str, order_by: str
) -> dict[str, Any]:
    columns = [str(row[1]) for row in connection.execute(f'PRAGMA table_info("{table}")')]
    if not columns:
        raise AssertionError(f"Required table is missing: {table}")
    rows = [
        list(row)
        for row in connection.execute(f'SELECT * FROM "{table}" ORDER BY "{order_by}"')
    ]
    return {
        "table": table,
        "row_count": len(rows),
        "columns": columns,
        "rowset_sha256": canonical_sha256(rows),
        "schema_sha256": schema_sha256(connection, table),
    }


def apply_migration(connection: sqlite3.Connection, migration: Path) -> None:
    connection.executescript(migration.read_text(encoding="utf-8"))


def seed_nonempty_r4_state(connection: sqlite3.Connection) -> dict[str, str]:
    ids = {
        "meeting_id": "ft-r5-migration-meeting",
        "run_id": "ft-r5-migration-run",
        "segment_id": "ft-r5-migration-segment",
    }
    timestamp = "2026-09-02T00:00:00.000Z"
    digest = sha256_bytes(b"moss-r5-nonempty-migration-sentinel")
    with connection:
        connection.execute(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            (ids["meeting_id"], "R5 migration sentinel", timestamp, timestamp),
        )
        connection.execute(
            """
            INSERT INTO moss_transcription_runs
                (run_id, meeting_id, status, created_at, updated_at, started_at, completed_at,
                 source_transcript_sha256, audio_sha256, model_sha256, runtime_sha256,
                 context_sha256, backend_name, backend_version, runtime_version,
                 model_revision, device_name, raw_output_sha256, clean_output_sha256,
                 candidate_sha256, segment_count, wall_elapsed_ms, wall_rtf,
                 peak_memory_bytes, error_code)
            VALUES (?, ?, 'completed', ?, ?, ?, ?, ?, ?, ?, ?, ?, 'moss', 'fixture',
                    'fixture', 'fixture', 'test-device', ?, ?, ?, 1, 1000, 1.0, 1024, NULL)
            """,
            (
                ids["run_id"],
                ids["meeting_id"],
                timestamp,
                timestamp,
                timestamp,
                timestamp,
                digest,
                digest,
                digest,
                digest,
                digest,
                digest,
                digest,
                digest,
            ),
        )
        connection.execute(
            """
            INSERT INTO moss_candidate_segments
                (segment_id, run_id, segment_index, start_ms, end_ms,
                 speaker_label, raw_text, created_at)
            VALUES (?, ?, 0, 0, 1000, 'S01', 'R5 migration sentinel text', ?)
            """,
            (ids["segment_id"], ids["run_id"], timestamp),
        )
        connection.execute(
            """
            INSERT INTO moss_candidate_segment_alignment
                (segment_id, raw_segment_index, raw_start_ms, raw_end_ms,
                 raw_text_sha256, alignment_method, confidence,
                 source_anchor_ids_json, source_transcript_sha256, created_at)
            VALUES (?, 0, 0, 1000, ?, 'moss_segment', NULL, '[]', ?, ?)
            """,
            (ids["segment_id"], digest, digest, timestamp),
        )
        connection.execute(
            """
            INSERT INTO moss_run_diagnostics
                (run_id, audio_duration_ms, activity_frame_ms, activity_threshold_dbfs,
                 first_active_ms, last_active_ms, model_last_timestamp_ms, tail_delta_ms,
                 aligned_segment_count, fallback_segment_count, source_anchor_count,
                 source_hash_verified, source_expected_sha256, source_actual_sha256,
                 fallback_reason, created_at)
            VALUES (?, 1000, 20, -40.0, 0, 900, 950, 50, 1, 0, 0, 1, ?, ?, NULL, ?)
            """,
            (ids["run_id"], digest, digest, timestamp),
        )
    return ids


def add_check(checks: list[dict[str, Any]], name: str, passed: bool, **details: Any) -> None:
    checks.append({"name": name, "verdict": "PASS" if passed else "FAIL", **details})


def run_test(migrations_dir: Path, output_dir: Path) -> dict[str, Any]:
    migration_files = sorted(migrations_dir.glob("*.sql"), key=lambda path: path.name)
    migration_names = [path.name for path in migration_files]
    if migration_names.count(R4_MIGRATION) != 1 or migration_names.count(R5_MIGRATION) != 1:
        raise AssertionError("R4 and R5 migrations must each exist exactly once")
    r4_index = migration_names.index(R4_MIGRATION)
    r5_index = migration_names.index(R5_MIGRATION)
    if r5_index != r4_index + 1:
        raise AssertionError("R5 migration must immediately follow the R4 migration")

    database = output_dir / "moss-r5-migration.sqlite"
    connection = sqlite3.connect(database, timeout=30)
    checks: list[dict[str, Any]] = []
    try:
        connection.execute("PRAGMA foreign_keys = ON")
        for migration in migration_files[: r4_index + 1]:
            apply_migration(connection, migration)

        sentinel_ids = seed_nonempty_r4_state(connection)
        before_alignment = table_snapshot(connection, ALIGNMENT_TABLE, "segment_id")
        before_diagnostics = table_snapshot(connection, DIAGNOSTICS_TABLE, "run_id")
        before_alignment_index_sha256 = object_sha256(
            connection, "index", "idx_moss_alignment_raw_segment"
        )
        add_check(
            checks,
            "r4_alignment_schema_is_approved",
            before_alignment["schema_sha256"] == EXPECTED_SCHEMA_SHA256["r4_alignment"],
            actual=before_alignment["schema_sha256"],
            expected=EXPECTED_SCHEMA_SHA256["r4_alignment"],
        )
        add_check(
            checks,
            "r4_run_diagnostics_schema_is_approved",
            before_diagnostics["schema_sha256"] == EXPECTED_SCHEMA_SHA256["run_diagnostics"],
            actual=before_diagnostics["schema_sha256"],
            expected=EXPECTED_SCHEMA_SHA256["run_diagnostics"],
        )
        add_check(checks, "r4_alignment_is_nonempty", before_alignment["row_count"] > 0)
        add_check(checks, "r4_run_diagnostics_is_nonempty", before_diagnostics["row_count"] > 0)
        add_check(
            checks,
            "r4_alignment_index_schema_is_approved",
            before_alignment_index_sha256 == EXPECTED_ALIGNMENT_INDEX_SHA256,
            actual=before_alignment_index_sha256,
            expected=EXPECTED_ALIGNMENT_INDEX_SHA256,
        )

        apply_migration(connection, migration_files[r5_index])

        after_alignment = table_snapshot(connection, ALIGNMENT_TABLE, "segment_id")
        after_diagnostics = table_snapshot(connection, DIAGNOSTICS_TABLE, "run_id")
        add_check(
            checks,
            "r5_alignment_schema_is_approved",
            after_alignment["schema_sha256"] == EXPECTED_SCHEMA_SHA256["r5_alignment"],
            actual=after_alignment["schema_sha256"],
            expected=EXPECTED_SCHEMA_SHA256["r5_alignment"],
        )
        add_check(
            checks,
            "alignment_rows_are_preserved_exactly",
            before_alignment["columns"] == after_alignment["columns"]
            and before_alignment["row_count"] == after_alignment["row_count"]
            and before_alignment["rowset_sha256"] == after_alignment["rowset_sha256"],
            before_rowset_sha256=before_alignment["rowset_sha256"],
            after_rowset_sha256=after_alignment["rowset_sha256"],
        )
        add_check(
            checks,
            "run_diagnostics_schema_and_rows_are_unchanged",
            before_diagnostics == after_diagnostics,
            before_schema_sha256=before_diagnostics["schema_sha256"],
            after_schema_sha256=after_diagnostics["schema_sha256"],
            before_rowset_sha256=before_diagnostics["rowset_sha256"],
            after_rowset_sha256=after_diagnostics["rowset_sha256"],
        )

        new_table_schemas: dict[str, str] = {}
        for table in NEW_R5_TABLES:
            actual = schema_sha256(connection, table)
            new_table_schemas[table] = actual
            add_check(
                checks,
                f"{table}_schema_is_approved",
                actual == EXPECTED_SCHEMA_SHA256[table],
                actual=actual,
                expected=EXPECTED_SCHEMA_SHA256[table],
            )

        alignment_index_sha256 = object_sha256(
            connection, "index", "idx_moss_alignment_raw_segment"
        )
        add_check(
            checks,
            "r5_alignment_index_schema_is_approved",
            alignment_index_sha256 == EXPECTED_ALIGNMENT_INDEX_SHA256,
            actual=alignment_index_sha256,
            expected=EXPECTED_ALIGNMENT_INDEX_SHA256,
        )
        stale_table = connection.execute(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?",
            ("moss_candidate_segment_alignment_r4",),
        ).fetchone()
        add_check(
            checks,
            "temporary_r4_alignment_table_is_absent",
            stale_table is None,
        )
        integrity_rows = [str(row[0]) for row in connection.execute("PRAGMA integrity_check")]
        foreign_key_rows = [list(row) for row in connection.execute("PRAGMA foreign_key_check")]
        add_check(checks, "sqlite_integrity_check_is_ok", integrity_rows == ["ok"], actual=integrity_rows)
        add_check(checks, "foreign_key_check_is_empty", foreign_key_rows == [], actual=foreign_key_rows)

        failed = [check for check in checks if check["verdict"] != "PASS"]
        return {
            "schema_version": 1,
            "suite": "moss-r5-nonempty-migration",
            "verdict": "PASS" if not failed else "FAIL",
            "total": len(checks),
            "passed": len(checks) - len(failed),
            "failed": len(failed),
            "checks": checks,
            "sentinel_ids": sentinel_ids,
            "applied_through_r4": [file_evidence(path) for path in migration_files[: r4_index + 1]],
            "applied_r5": file_evidence(migration_files[r5_index]),
            "before": {
                "alignment": before_alignment,
                "run_diagnostics": before_diagnostics,
            },
            "after": {
                "alignment": after_alignment,
                "run_diagnostics": after_diagnostics,
                "new_table_schema_sha256": new_table_schemas,
            },
            "database": file_evidence(database),
            "producer": file_evidence(Path(__file__).resolve()),
            "invocation": {
                "python_executable": str(Path(sys.executable).resolve()),
                "python_version": platform.python_version(),
                "argv": [str(item) for item in sys.argv],
            },
        }
    finally:
        connection.close()


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit("Usage: test-moss-r5-migration.py <migrations-dir> <output-dir>")
    migrations_dir = Path(sys.argv[1]).resolve()
    output_dir = Path(sys.argv[2]).resolve()
    if not migrations_dir.is_dir():
        raise SystemExit(f"Migrations directory does not exist: {migrations_dir}")
    if output_dir.exists() and any(output_dir.iterdir()):
        raise SystemExit(f"Output directory must be empty: {output_dir}")
    output_dir.mkdir(parents=True, exist_ok=True)
    report_path = output_dir / "moss-r5-migration.private.json"
    try:
        report = run_test(migrations_dir, output_dir)
    except Exception as error:
        report = {
            "schema_version": 1,
            "suite": "moss-r5-nonempty-migration",
            "verdict": "FAIL",
            "total": 1,
            "passed": 0,
            "failed": 1,
            "error_type": type(error).__name__,
            "error": str(error),
            "traceback": traceback.format_exc(),
        }
    report_path.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
