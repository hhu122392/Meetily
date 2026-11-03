ALTER TABLE lark_app_deliveries ADD COLUMN message_type TEXT NOT NULL DEFAULT 'text';
ALTER TABLE lark_app_deliveries ADD COLUMN message_content TEXT;
