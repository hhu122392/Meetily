ALTER TABLE lark_editable_documents ADD COLUMN title TEXT;
ALTER TABLE lark_manual_drafts ADD COLUMN attachment_status TEXT NOT NULL DEFAULT 'not_selected';
ALTER TABLE lark_manual_drafts ADD COLUMN attachment_message_id TEXT;
