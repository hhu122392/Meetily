from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sqlite3
import sys


REQUIRED_NONEMPTY_TABLES = (
    "summary_manual_revisions",
    "moss_transcription_runs",
    "moss_candidate_segments",
    "moss_term_corrections",
    "moss_speaker_bindings",
    "moss_segment_overrides",
    "moss_activation_snapshots",
    "moss_activation_segments",
)


def main() -> int:
    if len(sys.argv) != 4:
        raise SystemExit(
            "Usage: seed-lifecycle-preservation-fixture.py <database> <fixture-binding.json> <output.json>"
        )
    database = Path(sys.argv[1]).resolve()
    binding_path = Path(sys.argv[2]).resolve()
    output = Path(sys.argv[3]).resolve()
    binding = json.loads(binding_path.read_text(encoding="utf-8"))
    meeting_id = str(binding["meeting_id"])
    transcript_id = str(binding["anchor_transcript_id"])
    token = hashlib.sha256(meeting_id.encode("utf-8")).hexdigest()[:16]
    ids = {
        "revision_id": f"ft-r05-revision-{token}",
        "run_id": f"ft-r05-run-{token}",
        "segment_id": f"ft-r05-segment-{token}",
        "correction_id": f"ft-r05-correction-{token}",
        "binding_id": f"ft-r05-binding-{token}",
        "override_id": f"ft-r05-override-{token}",
        "activation_id": f"ft-r05-activation-{token}",
    }
    sha_values = {name: hashlib.sha256(f"{name}|{token}".encode()).hexdigest() for name in (
        "source", "audio", "model", "runtime", "context", "raw", "clean", "candidate", "pre", "active"
    )}
    timestamp = "2026-09-02T00:00:00.000Z"

    connection = sqlite3.connect(database, timeout=30)
    try:
        connection.execute("PRAGMA foreign_keys = ON")
        with connection:
            connection.execute(
                """
                INSERT INTO summary_manual_revisions
                    (revision_id, meeting_id, created_at, source_generation_id, summary_json, markdown)
                VALUES (?, ?, ?, NULL, ?, ?)
                """,
                (
                    ids["revision_id"], meeting_id, timestamp,
                    json.dumps({"title": "R05 preservation sentinel", "sections": [{"text": "must survive"}]}),
                    "# R05 preservation sentinel\n\nThis manual revision must survive upgrade and rollback.\n",
                ),
            )
            connection.execute(
                """
                INSERT INTO moss_transcription_runs
                    (run_id, meeting_id, status, created_at, updated_at, started_at, completed_at,
                     source_transcript_sha256, audio_sha256, model_sha256, runtime_sha256, context_sha256,
                     backend_name, backend_version, runtime_version, model_revision, device_name,
                     raw_output_sha256, clean_output_sha256, candidate_sha256, segment_count,
                     wall_elapsed_ms, wall_rtf, peak_memory_bytes, error_code)
                VALUES (?, ?, 'completed', ?, ?, ?, ?, ?, ?, ?, ?, ?, 'moss', 'ft', 'ft', 'ft',
                        'fixture', ?, ?, ?, 1, 1, 0.001, 1, NULL)
                """,
                (
                    ids["run_id"], meeting_id, timestamp, timestamp, timestamp, timestamp,
                    sha_values["source"], sha_values["audio"], sha_values["model"],
                    sha_values["runtime"], sha_values["context"], sha_values["raw"],
                    sha_values["clean"], sha_values["candidate"],
                ),
            )
            connection.execute(
                """
                INSERT INTO moss_candidate_segments
                    (segment_id, run_id, segment_index, start_ms, end_ms, speaker_label, raw_text, created_at)
                VALUES (?, ?, 0, 0, 1000, 'S01', 'R05 original sentinel text', ?)
                """,
                (ids["segment_id"], ids["run_id"], timestamp),
            )
            connection.execute(
                """
                INSERT INTO moss_term_corrections
                    (correction_id, segment_id, revision, original_text, replacement_text, result_text,
                     start_char, end_char, rule_id, context_sha256, created_at, reverted_at)
                VALUES (?, ?, 1, 'original', 'corrected', 'R05 corrected sentinel text',
                        4, 12, 'ft-r05-rule', ?, ?, NULL)
                """,
                (ids["correction_id"], ids["segment_id"], sha_values["context"], timestamp),
            )
            connection.execute(
                """
                INSERT INTO moss_speaker_bindings
                    (binding_id, run_id, speaker_label, person_id, person_display_name,
                     context_sha256, created_at, revoked_at)
                VALUES (?, ?, 'S01', 'ft-r05-person', 'R05 Test Person', ?, ?, NULL)
                """,
                (ids["binding_id"], ids["run_id"], sha_values["context"], timestamp),
            )
            connection.execute(
                """
                INSERT INTO moss_segment_overrides
                    (override_id, segment_id, revision, replacement_text, person_id,
                     person_display_name, context_sha256, reason_code, created_at, revoked_at)
                VALUES (?, ?, 1, 'R05 override sentinel text', 'ft-r05-person',
                        'R05 Test Person', ?, 'functional-test', ?, NULL)
                """,
                (ids["override_id"], ids["segment_id"], sha_values["context"], timestamp),
            )
            connection.execute(
                """
                INSERT INTO moss_activation_snapshots
                    (activation_id, meeting_id, run_id, status, activated_at, rolled_back_at,
                     pre_activation_transcript_sha256, activated_transcript_sha256, candidate_sha256,
                     pre_activation_transcripts_json, activated_transcripts_json)
                VALUES (?, ?, ?, 'active', ?, NULL, ?, ?, ?, '[]', '[]')
                """,
                (
                    ids["activation_id"], meeting_id, ids["run_id"], timestamp,
                    sha_values["pre"], sha_values["active"], sha_values["candidate"],
                ),
            )
            connection.execute(
                """
                INSERT INTO moss_activation_segments
                    (activation_id, segment_index, transcript_id, candidate_segment_id,
                     start_ms, end_ms, speaker_label, resolved_person_id,
                     resolved_person_display_name, text, correction_id, override_id, binding_id)
                VALUES (?, 0, ?, ?, 0, 1000, 'S01', 'ft-r05-person', 'R05 Test Person',
                        'R05 activated sentinel text', ?, ?, ?)
                """,
                (
                    ids["activation_id"], transcript_id, ids["segment_id"], ids["correction_id"],
                    ids["override_id"], ids["binding_id"],
                ),
            )

        counts = {
            table: int(connection.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0])
            for table in REQUIRED_NONEMPTY_TABLES
        }
        integrity = str(connection.execute("PRAGMA integrity_check").fetchone()[0])
        if integrity != "ok" or any(value < 1 for value in counts.values()):
            raise RuntimeError(f"Seed verification failed: integrity={integrity}, counts={counts}")
    finally:
        connection.close()

    result = {
        "schema_version": 1,
        "status": "PASS",
        "meeting_id": meeting_id,
        "anchor_transcript_id": transcript_id,
        "sentinel_ids": ids,
        "required_nonempty_table_counts": counts,
        "integrity_check": integrity,
    }
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": "PASS", "counts": counts}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
