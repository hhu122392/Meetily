ALTER TABLE lark_app_deliveries ADD COLUMN requires_duplicate_ack INTEGER NOT NULL DEFAULT 0;
ALTER TABLE lark_app_deliveries ADD COLUMN attachment_status TEXT;
ALTER TABLE lark_app_deliveries ADD COLUMN attachment_detail TEXT;
ALTER TABLE lark_app_deliveries ADD COLUMN attachment_last_sent_at INTEGER;
ALTER TABLE lark_chat_names ADD COLUMN error TEXT;
