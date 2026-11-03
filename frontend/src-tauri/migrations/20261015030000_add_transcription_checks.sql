CREATE TABLE recording_transcription_checks (
    folder_path TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);
