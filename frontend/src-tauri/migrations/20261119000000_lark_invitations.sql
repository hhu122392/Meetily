ALTER TABLE lark_app_routes ADD COLUMN invitation_id TEXT;
CREATE TABLE lark_invitations (
    id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    source_key TEXT NOT NULL,
    invitation TEXT NOT NULL,
    topic TEXT NOT NULL,
    starts_at INTEGER NOT NULL,
    ends_at INTEGER NOT NULL,
    meeting_no TEXT,
    meeting_url TEXT,
    event_id TEXT,
    command_at INTEGER NOT NULL,
    record_bound_at INTEGER,
    route_armed INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'queued',
    detail TEXT NOT NULL DEFAULT '已接收邀请，等待核对',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt INTEGER NOT NULL DEFAULT 0,
    sent_at INTEGER,
    UNIQUE(app_id, owner_id, chat_id, source_key)
);
CREATE INDEX lark_invitations_pending ON lark_invitations(app_id,owner_id,status,next_attempt);
