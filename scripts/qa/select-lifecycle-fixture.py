from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import sqlite3
import sys


def transcript_fingerprint(rows: list[tuple[object, object]]) -> str:
    lines = [f"{str(row_id)}|{str(text)}" for row_id, text in rows]
    lines.sort()
    return hashlib.sha256("\n".join(lines).encode("utf-8")).hexdigest().upper()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest().upper()


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit("Usage: select-lifecycle-fixture.py <database> <snapshot.sqlite> <output.json>")
    database = Path(sys.argv[1]).resolve()
    snapshot = Path(sys.argv[2]).resolve()
    output = Path(sys.argv[3]).resolve()
    if not database.is_file():
        raise FileNotFoundError(database)
    if snapshot.exists():
        raise FileExistsError(snapshot)
    source_set = snapshot.parent / f"{snapshot.name}.source-set"
    if source_set.exists():
        raise FileExistsError(source_set)
    wal = Path(f"{database}-wal")
    shm = Path(f"{database}-shm")
    allowed_names = {database.name, wal.name, shm.name}
    discovered_sidecars = sorted(
        path for path in database.parent.glob(f"{database.name}-*") if path.is_file()
    )
    rejected_sidecars = [path for path in discovered_sidecars if path.name not in allowed_names]
    if rejected_sidecars:
        raise RuntimeError(
            "Unapproved SQLite sidecar(s) are present: "
            + ", ".join(path.name for path in rejected_sidecars)
        )
    if not wal.is_file():
        raise RuntimeError("The frozen lifecycle fixture must include its approved WAL")
    # The frozen manifest binds the main database and WAL. SHM is deliberately not
    # copied: it is transient coordination state and SQLite recreates it only in
    # this private source-set copy, never in the protected fixture directory.
    source_files = [database, wal]
    source_before = {
        path.name: {"bytes": path.stat().st_size, "sha256": file_sha256(path)} for path in source_files
    }
    source_set.mkdir(parents=True)
    for path in source_files:
        shutil.copy2(path, source_set / path.name)
    source_after = {
        path.name: {"bytes": path.stat().st_size, "sha256": file_sha256(path)} for path in source_files
    }
    if source_before != source_after:
        raise RuntimeError("The source SQLite file set changed while it was being frozen")
    for path in source_files:
        copied = source_set / path.name
        expected = source_before[path.name]
        if copied.stat().st_size != expected["bytes"] or file_sha256(copied) != expected["sha256"]:
            raise RuntimeError(f"Frozen SQLite source-set copy does not match: {path.name}")

    copied_database = source_set / database.name
    uri = f"file:{copied_database.as_posix()}?mode=ro"
    source = sqlite3.connect(uri, uri=True, timeout=10)
    snapshot.parent.mkdir(parents=True, exist_ok=True)
    destination = sqlite3.connect(snapshot, timeout=10)
    try:
        source.backup(destination)
        destination.commit()
        integrity = destination.execute("PRAGMA integrity_check").fetchone()
        if integrity is None or integrity[0] != "ok":
            raise RuntimeError(f"Snapshot integrity_check failed: {integrity}")
        foreign_key_rows = list(destination.execute("PRAGMA foreign_key_check"))
        if foreign_key_rows:
            raise RuntimeError(f"Snapshot foreign_key_check failed with {len(foreign_key_rows)} violation(s)")
        migration_rows = [
            (int(item[0]), int(item[1]))
            for item in destination.execute(
                "SELECT version, success FROM _sqlx_migrations ORDER BY version"
            )
        ]
        meeting_count = int(destination.execute("SELECT COUNT(*) FROM meetings").fetchone()[0])
        total_transcript_count = int(destination.execute("SELECT COUNT(*) FROM transcripts").fetchone()[0])
        row = destination.execute(
            """
            SELECT m.id, m.title
            FROM meetings AS m
            WHERE EXISTS (SELECT 1 FROM transcripts AS t WHERE t.meeting_id = m.id)
            ORDER BY m.created_at, m.id
            LIMIT 1
            """
        ).fetchone()
        if row is None:
            raise RuntimeError("The fixture has no meeting with transcripts")
        meeting_id, title = str(row[0]), str(row[1])
        transcript_rows = [
            (item[0], item[1])
            for item in destination.execute(
                "SELECT id, transcript FROM transcripts WHERE meeting_id = ? ORDER BY COALESCE(audio_start_time, 0), id",
                (meeting_id,),
            )
        ]
    finally:
        destination.close()
        source.close()

    result = {
        "schema_version": 1,
        "database_name": database.name,
        "source_database_bytes": source_before[database.name]["bytes"],
        "source_database_sha256": source_before[database.name]["sha256"],
        "source_file_set": source_before,
        "source_file_set_unchanged_while_copying": True,
        "snapshot_database_bytes": snapshot.stat().st_size,
        "snapshot_database_sha256": file_sha256(snapshot),
        "snapshot_integrity_check": "ok",
        "snapshot_foreign_key_check": "ok",
        "snapshot_migration_count": len(migration_rows),
        "snapshot_last_migration_version": migration_rows[-1][0] if migration_rows else None,
        "snapshot_all_migrations_succeeded": all(item[1] == 1 for item in migration_rows),
        "snapshot_meeting_count": meeting_count,
        "snapshot_transcript_count": total_transcript_count,
        "source_sidecars_present": [path.name for path in source_files if path != database],
        "sqlite_sidecar_policy": "main_and_wal_bound_shm_ignored_and_rebuilt",
        "ignored_source_sidecars": [shm.name] if shm.is_file() else [],
        "rejected_unapproved_sidecars": [],
        "meeting_id": meeting_id,
        "meeting_title": title,
        "transcript_count": len(transcript_rows),
        "transcript_fingerprint_sha256": transcript_fingerprint(transcript_rows),
        "anchor_transcript_id": str(transcript_rows[0][0]),
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "output": str(output),
                "transcript_count": result["transcript_count"],
                "transcript_fingerprint_sha256": result["transcript_fingerprint_sha256"],
            },
            ensure_ascii=False,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
