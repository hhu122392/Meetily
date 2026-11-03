CREATE TABLE recording_jobs (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    folder_path TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    resume_stage TEXT NOT NULL DEFAULT 'queued',
    options_json TEXT NOT NULL,
    target_chat_id TEXT,
    audio_ready INTEGER NOT NULL DEFAULT 0,
    detail TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX recording_jobs_pending ON recording_jobs(status, created_at);
