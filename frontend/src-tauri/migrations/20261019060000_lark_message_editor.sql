ALTER TABLE lark_app_deliveries ADD COLUMN draft_title TEXT;
ALTER TABLE lark_app_deliveries ADD COLUMN draft_summary TEXT;
ALTER TABLE lark_app_deliveries ADD COLUMN source_revision TEXT;
ALTER TABLE lark_app_deliveries ADD COLUMN message_edited INTEGER NOT NULL DEFAULT 0;
