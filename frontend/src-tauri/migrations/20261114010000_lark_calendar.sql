CREATE TABLE lark_calendar_cache (
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    synced_at INTEGER NOT NULL,
    PRIMARY KEY(app_id, owner_id)
);
CREATE TABLE lark_calendar_instances (
    instance_key TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    snapshot TEXT NOT NULL
);
CREATE INDEX lark_calendar_owner ON lark_calendar_instances(app_id, owner_id);
CREATE TABLE lark_calendar_recordings (
    meeting_id TEXT PRIMARY KEY REFERENCES meetings(id) ON DELETE CASCADE,
    instance_key TEXT NOT NULL,
    app_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    snapshot TEXT NOT NULL
);
CREATE INDEX lark_calendar_recorded_instance ON lark_calendar_recordings(instance_key);
