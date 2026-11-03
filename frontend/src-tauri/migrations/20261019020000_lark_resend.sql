ALTER TABLE lark_app_deliveries ADD COLUMN last_sent_at INTEGER;
UPDATE lark_app_deliveries SET last_sent_at=updated_at WHERE status='sent';

CREATE TABLE lark_chat_names (
    app_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    name TEXT,
    checked_at INTEGER NOT NULL,
    PRIMARY KEY(app_id, chat_id)
);
