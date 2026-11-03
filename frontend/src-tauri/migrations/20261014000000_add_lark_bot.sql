CREATE TABLE lark_bot_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    config_json TEXT NOT NULL
);

CREATE TABLE lark_summary_deliveries (
    fingerprint TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    sent_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
