CREATE TABLE lark_action_drafts (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    draft_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(meeting_id, source_key)
);
CREATE TABLE lark_action_receipts (
    action_id TEXT NOT NULL REFERENCES lark_action_drafts(id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    status TEXT NOT NULL,
    request_json TEXT NOT NULL,
    client_token TEXT NOT NULL,
    task_guid TEXT,
    task_url TEXT,
    detail TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(action_id, app_id, owner_id)
);
CREATE TABLE lark_action_previews (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    source_hash TEXT NOT NULL,
    config_revision TEXT NOT NULL,
    draft_revision TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
