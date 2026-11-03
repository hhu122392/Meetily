CREATE TABLE lark_meeting_signals (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    remote_id TEXT NOT NULL,
    meeting_no TEXT NOT NULL DEFAULT '',
    event_kind TEXT NOT NULL,
    event_at INTEGER NOT NULL,
    ended INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(app_id, owner_id, remote_id)
);
CREATE INDEX lark_signals_number ON lark_meeting_signals(app_id, owner_id, meeting_no);
