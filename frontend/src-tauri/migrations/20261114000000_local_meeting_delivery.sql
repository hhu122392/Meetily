CREATE TABLE meeting_template_people (
    template_id TEXT PRIMARY KEY,
    participants TEXT NOT NULL,
    host TEXT NOT NULL DEFAULT ''
);
CREATE TABLE lark_manual_drafts (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    config_revision TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'preview',
    detail TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    sent_at INTEGER,
    message_id TEXT
);
CREATE INDEX lark_manual_meeting ON lark_manual_drafts(meeting_id, created_at);
CREATE TABLE lark_archive_projects (
    app_id TEXT NOT NULL,
    name TEXT NOT NULL,
    parent_url TEXT NOT NULL,
    PRIMARY KEY(app_id,name)
);
CREATE TABLE lark_document_archives (
    fingerprint TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    document_id TEXT,
    document_url TEXT,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE meeting_processing_timings (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    transcription_seconds REAL,
    audio_seconds REAL,
    model TEXT,
    provider TEXT
);
