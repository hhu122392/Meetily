CREATE TABLE lark_app_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_json TEXT NOT NULL
);
CREATE TABLE lark_app_events (
    event_id TEXT PRIMARY KEY,
    received_at INTEGER NOT NULL
);
CREATE TABLE lark_app_routes (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    reference TEXT NOT NULL,
    reference_kind TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    remote_id TEXT,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY(app_id, owner_id, reference, reference_kind, chat_id)
);
CREATE TABLE lark_recording_links (
    folder_path TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    remote_id TEXT,
    meeting_no TEXT,
    topic TEXT,
    started_at INTEGER NOT NULL,
    stopped INTEGER NOT NULL DEFAULT 0,
    blocked INTEGER NOT NULL DEFAULT 0,
    auto_send INTEGER NOT NULL DEFAULT 0,
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TABLE lark_app_deliveries (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    app_id TEXT NOT NULL,
    remote_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    message_uuid TEXT NOT NULL UNIQUE,
    message_text TEXT NOT NULL,
    status TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    message_id TEXT,
    updated_at INTEGER NOT NULL
);
