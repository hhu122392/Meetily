CREATE TABLE lark_cloud_recordings (
    meeting_id TEXT PRIMARY KEY REFERENCES recording_jobs(meeting_id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    remote_id TEXT NOT NULL,
    meeting_no TEXT NOT NULL,
    auto_send INTEGER NOT NULL DEFAULT 0,
    next_attempt INTEGER NOT NULL DEFAULT 0,
    deadline INTEGER NOT NULL,
    ended_at INTEGER,
    UNIQUE(app_id, owner_id, remote_id)
);
