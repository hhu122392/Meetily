ALTER TABLE lark_archive_projects ADD COLUMN last_used_at INTEGER NOT NULL DEFAULT 0;
CREATE TABLE lark_directory_tree_cache (
    cache_key TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
