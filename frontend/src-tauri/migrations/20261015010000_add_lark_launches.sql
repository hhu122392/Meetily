CREATE TABLE lark_meeting_launches (
    request_id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    topic TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    detail TEXT NOT NULL DEFAULT '',
    reserve_id TEXT,
    meeting_no TEXT,
    meeting_url TEXT,
    reply_id TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    UNIQUE(app_id, message_id)
);
CREATE INDEX lark_launch_pending ON lark_meeting_launches(app_id, owner_id, status, created_at);
