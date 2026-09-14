CREATE TABLE IF NOT EXISTS summary_manual_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    source_generation_id TEXT,
    summary_json TEXT NOT NULL,
    markdown TEXT,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_summary_manual_revisions_meeting_created
    ON summary_manual_revisions(meeting_id, created_at DESC);
