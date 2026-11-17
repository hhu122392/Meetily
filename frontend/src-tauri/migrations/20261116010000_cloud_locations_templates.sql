ALTER TABLE lark_archive_projects ADD COLUMN target_json TEXT;
ALTER TABLE lark_archive_projects ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
ALTER TABLE lark_archive_projects ADD COLUMN display_name TEXT;
ALTER TABLE lark_editable_documents ADD COLUMN credential_owner TEXT NOT NULL DEFAULT '';
CREATE TABLE lark_cloud_preferences (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    last_project TEXT,
    PRIMARY KEY(app_id, owner_id)
);
CREATE TABLE lark_message_templates (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    name TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(app_id, owner_id, name)
);
