import datetime
import json
import sqlite3
import sys


if len(sys.argv) != 3:
    raise SystemExit("Usage: audit-t03-db.py <database.sqlite> <meeting-id>")

database_path, meeting_id = sys.argv[1:]
connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
connection.row_factory = sqlite3.Row

result = {
    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "integrity_check": connection.execute("PRAGMA integrity_check").fetchone()[0],
    "meeting": dict(
        connection.execute(
            "SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?",
            (meeting_id,),
        ).fetchone()
    ),
    "transcript_count": connection.execute(
        "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?", (meeting_id,)
    ).fetchone()[0],
    "saved_marker_rows": connection.execute(
        "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND transcript LIKE ?",
        (meeting_id, "%【人工校正】%"),
    ).fetchone()[0],
    "cancelled_marker_rows": connection.execute(
        "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ? AND transcript LIKE ?",
        (meeting_id, "%【不应保存】%"),
    ).fetchone()[0],
    "summary_processes": [
        list(row)
        for row in connection.execute(
            "SELECT status, COUNT(*) FROM summary_processes WHERE meeting_id = ? GROUP BY status",
            (meeting_id,),
        ).fetchall()
    ],
    "summary_history_count": connection.execute(
        "SELECT COUNT(*) FROM summary_generation_history WHERE meeting_id = ?", (meeting_id,)
    ).fetchone()[0],
    "pending_processes_global": connection.execute(
        "SELECT COUNT(*) FROM summary_processes WHERE status IN ('pending', 'processing')"
    ).fetchone()[0],
}

print(json.dumps(result, ensure_ascii=False, indent=2))
