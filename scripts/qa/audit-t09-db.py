import datetime
import hashlib
import json
import pathlib
import sqlite3
import sys


if len(sys.argv) != 5:
    raise SystemExit("Usage: audit-t09-db.py <database.sqlite> <live-id> <import-id> <phase>")

database_path, live_id, import_id, phase = sys.argv[1:]
connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
connection.row_factory = sqlite3.Row


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest().upper()


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def parse_json(value):
    if not value:
        return None
    try:
        return json.loads(value)
    except (TypeError, json.JSONDecodeError):
        return None


def file_evidence(folder: pathlib.Path) -> list[dict]:
    allowed = {".json", ".mp4", ".wav", ".m4a", ".mp3", ".webm"}
    items = []
    if not folder.is_dir():
        return items
    for path in sorted(folder.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in allowed:
            continue
        items.append({
            "relative_path": str(path.relative_to(folder)),
            "bytes": path.stat().st_size,
            "sha256": sha256_file(path),
        })
    return items


def meeting_snapshot(meeting_id: str) -> dict:
    row = connection.execute(
        "SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?",
        (meeting_id,),
    ).fetchone()
    if row is None:
        raise SystemExit(f"Meeting not found: {meeting_id}")
    folder = pathlib.Path(row["folder_path"])
    metadata_path = folder / "metadata.json"
    transcripts_path = folder / "transcripts.json"
    metadata = parse_json(metadata_path.read_text(encoding="utf-8")) if metadata_path.is_file() else None
    transcripts = [
        dict(item)
        for item in connection.execute(
            """
            SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker
            FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time, timestamp, id
            """,
            (meeting_id,),
        ).fetchall()
    ]
    transcript_payload = json.dumps(transcripts, ensure_ascii=False, sort_keys=True).encode("utf-8")
    process = connection.execute(
        """
        SELECT status, created_at, updated_at, start_time, end_time, error, result, metadata
        FROM summary_processes WHERE meeting_id = ?
        """,
        (meeting_id,),
    ).fetchone()
    process_result = parse_json(process["result"]) if process else None
    markdown = ""
    if isinstance(process_result, dict):
        if isinstance(process_result.get("markdown"), str):
            markdown = process_result["markdown"]
        else:
            data = process_result.get("data")
            if isinstance(data, dict):
                markdown = data.get("markdown") or ""
    history = [
        dict(item)
        for item in connection.execute(
            """
            SELECT generation_id, status, created_at, completed_at, template_id, template_version,
                   resolution_source, model_provider, model_name, summary_language,
                   error_category, snapshot_state
            FROM summary_generation_history WHERE meeting_id = ? ORDER BY created_at
            """,
            (meeting_id,),
        ).fetchall()
    ]
    manual = [
        dict(item)
        for item in connection.execute(
            """
            SELECT revision_id, created_at, source_generation_id, markdown
            FROM summary_manual_revisions WHERE meeting_id = ? ORDER BY created_at
            """,
            (meeting_id,),
        ).fetchall()
    ]
    files = file_evidence(folder)
    media = [item for item in files if pathlib.Path(item["relative_path"]).suffix.lower() != ".json"]
    return {
        "database": dict(row),
        "database_id_count": connection.execute(
            "SELECT COUNT(1) FROM meetings WHERE id = ?", (meeting_id,)
        ).fetchone()[0],
        "same_title_count": connection.execute(
            "SELECT COUNT(1) FROM meetings WHERE title = ?", (row["title"],)
        ).fetchone()[0],
        "folder_exists": folder.is_dir(),
        "metadata": metadata,
        "metadata_sha256": sha256_file(metadata_path) if metadata_path.is_file() else None,
        "transcripts_file_sha256": sha256_file(transcripts_path) if transcripts_path.is_file() else None,
        "transcript_count": len(transcripts),
        "transcript_sha256": sha256_bytes(transcript_payload),
        "saved_transcript_marker_count": sum("【人工校正】" in item["transcript"] for item in transcripts),
        "cancelled_transcript_marker_count": sum("【不应保存】" in item["transcript"] for item in transcripts),
        "transcripts": transcripts,
        "files": files,
        "media": media,
        "summary_process": {
            "status": process["status"],
            "created_at": process["created_at"],
            "updated_at": process["updated_at"],
            "start_time": process["start_time"],
            "end_time": process["end_time"],
            "error": process["error"],
            "metadata": parse_json(process["metadata"]),
            "result_sha256": sha256_bytes((process["result"] or "").encode("utf-8")),
        } if process else None,
        "summary_markdown": markdown,
        "summary_markdown_sha256": sha256_bytes(markdown.encode("utf-8")) if markdown else None,
        "summary_saved_marker_count": markdown.count("【摘要人工校正】"),
        "summary_restart_marker_count": markdown.count("【重启复核】"),
        "history": history,
        "history_count": len(history),
        "manual_revisions": manual,
        "manual_revision_count": len(manual),
    }


live = meeting_snapshot(live_id)
imported = meeting_snapshot(import_id)
integrity = connection.execute("PRAGMA integrity_check").fetchone()[0]
global_running_summary = connection.execute(
    "SELECT COUNT(1) FROM summary_processes WHERE lower(status) IN ('pending', 'processing')"
).fetchone()[0]
all_metadata_recording = []
for (folder_path,) in connection.execute("SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL"):
    metadata_path = pathlib.Path(folder_path) / "metadata.json"
    if not metadata_path.is_file():
        continue
    metadata = parse_json(metadata_path.read_text(encoding="utf-8"))
    if isinstance(metadata, dict) and str(metadata.get("status", "")).lower() in {
        "recording", "starting", "pausing", "paused", "resuming", "stopping", "finalizing"
    }:
        all_metadata_recording.append({"folder": folder_path, "status": metadata.get("status")})

result = {
    "captured_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "phase": phase,
    "database_path": database_path,
    "integrity_check": integrity,
    "live": live,
    "imported": imported,
    "global_running_summary_count": global_running_summary,
    "historical_recording_metadata_observations": all_metadata_recording,
    "verdict": {
        "database_integrity_ok": integrity == "ok",
        "live_single_record": live["database_id_count"] == 1,
        "imported_single_record": imported["database_id_count"] == 1,
        "live_audio_present": len(live["media"]) >= 1 and all(item["bytes"] > 0 for item in live["media"]),
        "imported_audio_present": len(imported["media"]) >= 1 and all(item["bytes"] > 0 for item in imported["media"]),
        "live_transcript_edit_preserved": live["saved_transcript_marker_count"] == 1
            and live["cancelled_transcript_marker_count"] == 0,
        "imported_not_polluted": imported["saved_transcript_marker_count"] == 0
            and imported["summary_saved_marker_count"] == 0,
        "live_template_bound": isinstance(live["metadata"], dict)
            and isinstance(live["metadata"].get("summary_template"), dict)
            and live["metadata"]["summary_template"].get("mode") == "meeting_override",
        "imported_template_independent": not isinstance(imported["metadata"], dict)
            or "summary_template" not in imported["metadata"],
        "live_summary_edit_preserved": live["summary_saved_marker_count"] == 1,
        "live_history_completed": live["history_count"] >= 1
            and all(item["status"] == "completed" for item in live["history"]),
        "no_running_summary_tasks": global_running_summary == 0,
        "target_meetings_not_recording": str(live["metadata"].get("status", "")).lower() == "completed"
            and str(imported["metadata"].get("status", "")).lower() == "completed",
    },
}
print(json.dumps(result, ensure_ascii=False, indent=2))
