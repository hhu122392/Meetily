CREATE TABLE transcription_checkpoints (
    meeting_id TEXT PRIMARY KEY REFERENCES recording_jobs(meeting_id) ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    next_segment INTEGER NOT NULL,
    total_segments INTEGER NOT NULL,
    transcripts_json TEXT NOT NULL,
    total_confidence REAL NOT NULL
);
