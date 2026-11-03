CREATE TABLE lark_auto_recordings (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    remote_id TEXT NOT NULL,
    folder_path TEXT UNIQUE,
    status TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT '',
    postprocessed INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(app_id, owner_id, remote_id)
);
