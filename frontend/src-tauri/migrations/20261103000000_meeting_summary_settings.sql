CREATE TABLE meeting_summary_settings (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    settings_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE summary_request_snapshots (
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    process_id TEXT NOT NULL,
    settings_json TEXT NOT NULL,
    template_json TEXT NOT NULL,
    PRIMARY KEY (meeting_id, process_id)
);
