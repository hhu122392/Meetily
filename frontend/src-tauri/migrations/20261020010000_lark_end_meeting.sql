ALTER TABLE lark_meeting_launches ADD COLUMN requester_id TEXT NOT NULL DEFAULT '';
ALTER TABLE lark_meeting_launches ADD COLUMN command_at INTEGER NOT NULL DEFAULT 0;
UPDATE lark_meeting_launches SET command_at=created_at*1000;
CREATE TABLE lark_meeting_end_requests (
    request_id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    requester_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    launch_id TEXT,
    meeting_id TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    detail TEXT NOT NULL DEFAULT '',
    reply_status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL
);
CREATE INDEX lark_end_pending ON lark_meeting_end_requests(app_id,owner_id,status,created_at);
