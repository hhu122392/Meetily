import datetime
import hashlib
import json
import pathlib
import sqlite3
import sys


if len(sys.argv) != 6:
    raise SystemExit(
        "Usage: audit-t04-db.py <database.sqlite> <live-id> <import-id> <source-audio> <imported-audio>"
    )

database_path, live_id, import_id, source_audio_arg, imported_audio_arg = sys.argv[1:]
connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
connection.row_factory = sqlite3.Row


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def meeting(meeting_id: str) -> dict:
    row = connection.execute(
        "SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?",
        (meeting_id,),
    ).fetchone()
    if row is None:
        raise SystemExit(f"Meeting not found: {meeting_id}")
    folder = pathlib.Path(row["folder_path"])
    metadata_path = folder / "metadata.json"
    transcript_path = folder / "transcripts.json"
    transcripts = connection.execute(
        """
        SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker
        FROM transcripts WHERE meeting_id = ? ORDER BY timestamp, id
        """,
        (meeting_id,),
    ).fetchall()
    chunk = connection.execute(
        """
        SELECT meeting_name, model, model_name, chunk_size, overlap, created_at
        FROM transcript_chunks WHERE meeting_id = ?
        """,
        (meeting_id,),
    ).fetchone()
    return {
        "database": dict(row),
        "metadata": json.loads(metadata_path.read_text(encoding="utf-8")),
        "transcripts_file_exists": transcript_path.exists(),
        "transcript_count": len(transcripts),
        "saved_marker_count": sum("【人工校正】" in item["transcript"] for item in transcripts),
        "cancelled_marker_count": sum("【不应保存】" in item["transcript"] for item in transcripts),
        "segments": [dict(item) for item in transcripts],
        "transcript_chunk": dict(chunk) if chunk else None,
        "summary_history_count": connection.execute(
            "SELECT COUNT(1) FROM summary_generation_history WHERE meeting_id = ?",
            (meeting_id,),
        ).fetchone()[0],
        "summary_processes": [
            list(item)
            for item in connection.execute(
                "SELECT status, COUNT(1) FROM summary_processes WHERE meeting_id = ? GROUP BY status",
                (meeting_id,),
            ).fetchall()
        ],
    }


source_audio = pathlib.Path(source_audio_arg)
imported_audio = pathlib.Path(imported_audio_arg)
source_hash = sha256(source_audio)
imported_hash = sha256(imported_audio)
live = meeting(live_id)
imported = meeting(import_id)

result = {
    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "integrity_check": connection.execute("PRAGMA integrity_check").fetchone()[0],
    "exact_import_title_count": connection.execute(
        "SELECT COUNT(1) FROM meetings WHERE title = ?", ("QA-CORE-20260824-IMPORT",)
    ).fetchone()[0],
    "live": live,
    "imported": imported,
    "audio": {
        "source": {"path": str(source_audio), "bytes": source_audio.stat().st_size, "sha256": source_hash},
        "imported": {"path": str(imported_audio), "bytes": imported_audio.stat().st_size, "sha256": imported_hash},
        "same_bytes": source_audio.stat().st_size == imported_audio.stat().st_size,
        "same_sha256": source_hash == imported_hash,
    },
    "global_pending_or_processing_summary_count": connection.execute(
        "SELECT COUNT(1) FROM summary_processes WHERE status IN ('pending', 'processing')"
    ).fetchone()[0],
    "verdict": {
        "database_integrity_ok": connection.execute("PRAGMA integrity_check").fetchone()[0] == "ok",
        "only_one_import_meeting": connection.execute(
            "SELECT COUNT(1) FROM meetings WHERE title = ?", ("QA-CORE-20260824-IMPORT",)
        ).fetchone()[0]
        == 1,
        "import_source_recorded": imported["metadata"].get("source") == "import",
        "import_status_completed": imported["metadata"].get("status") == "completed",
        "import_not_recording": imported["metadata"].get("status") != "recording",
        "audio_hash_unchanged": source_hash == imported_hash,
        "live_saved_edit_unchanged": live["saved_marker_count"] == 1,
        "live_cancelled_edit_absent": live["cancelled_marker_count"] == 0,
    },
}

print(json.dumps(result, ensure_ascii=False, indent=2))
