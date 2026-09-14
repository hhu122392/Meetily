import datetime
import json
import pathlib
import sqlite3
import sys


if len(sys.argv) != 4:
    raise SystemExit("Usage: audit-t06-db.py <database.sqlite> <meeting-id> <meeting-folder>")

database_path, meeting_id, meeting_folder_arg = sys.argv[1:]
folder = pathlib.Path(meeting_folder_arg)
connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
connection.row_factory = sqlite3.Row

history = [
    dict(row)
    for row in connection.execute(
        """
        SELECT generation_id, meeting_id, status, created_at, updated_at, completed_at,
               template_id, template_version, file_sha256, semantic_sha256,
               snapshot_path_relative, resolution_source, model_provider, model_name,
               summary_language, error_category, snapshot_state, cleanup_batch_id
        FROM summary_generation_history
        WHERE meeting_id = ? ORDER BY created_at DESC
        """,
        (meeting_id,),
    ).fetchall()
]
process = connection.execute(
    """
    SELECT meeting_id, status, created_at, updated_at, error, result, start_time,
           end_time, chunk_count, processing_time, metadata,
           result_backup, result_backup_timestamp
    FROM summary_processes WHERE meeting_id = ?
    """,
    (meeting_id,),
).fetchone()
notes = connection.execute(
    "SELECT meeting_id, notes_markdown, notes_json, created_at, updated_at FROM meeting_notes WHERE meeting_id = ?",
    (meeting_id,),
).fetchone()

snapshots = []
for row in history:
    relative = row["snapshot_path_relative"]
    snapshot_path = folder / relative if relative else None
    snapshots.append(
        {
            "generation_id": row["generation_id"],
            "relative_path": relative,
            "exists": bool(snapshot_path and snapshot_path.is_file()),
            "bytes": snapshot_path.stat().st_size if snapshot_path and snapshot_path.is_file() else None,
            "content": json.loads(snapshot_path.read_text(encoding="utf-8"))
            if snapshot_path and snapshot_path.is_file()
            else None,
        }
    )

result = {
    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "integrity_check": connection.execute("PRAGMA integrity_check").fetchone()[0],
    "history_count": len(history),
    "history": history,
    "process": dict(process) if process else None,
    "meeting_notes": dict(notes) if notes else None,
    "snapshots": snapshots,
    "pending_or_processing_global": connection.execute(
        "SELECT COUNT(1) FROM summary_processes WHERE status IN ('pending', 'processing')"
    ).fetchone()[0],
    "verdict": {
        "integrity_ok": connection.execute("PRAGMA integrity_check").fetchone()[0] == "ok",
        "newest_completed": bool(history and history[0]["status"] == "completed"),
        "newest_snapshot_available": bool(history and history[0]["snapshot_state"] == "available"),
        "newest_snapshot_exists": bool(snapshots and snapshots[0]["exists"]),
        "newest_meeting_override": bool(history and history[0]["resolution_source"] == "meeting_override"),
        "newest_model_correct": bool(
            history
            and history[0]["model_provider"] == "builtin-ai"
            and history[0]["model_name"] == "qwen3.5:4b"
        ),
        "no_running_summary_tasks": connection.execute(
            "SELECT COUNT(1) FROM summary_processes WHERE status IN ('pending', 'processing')"
        ).fetchone()[0]
        == 0,
    },
}

print(json.dumps(result, ensure_ascii=False, indent=2))
