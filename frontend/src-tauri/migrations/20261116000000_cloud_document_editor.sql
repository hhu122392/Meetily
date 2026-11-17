CREATE TABLE lark_editable_documents (
    id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    project TEXT NOT NULL,
    parent_url TEXT NOT NULL,
    document_id TEXT,
    document_url TEXT,
    revision INTEGER,
    saved_hash TEXT,
    saved_content TEXT,
    status TEXT NOT NULL DEFAULT 'creating',
    detail TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL,
    UNIQUE(app_id, meeting_id, project)
);
