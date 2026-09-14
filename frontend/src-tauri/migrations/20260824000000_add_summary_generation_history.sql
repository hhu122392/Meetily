-- P2-300: durable, non-sensitive history for every summary generation.
-- Raw prompts, transcript text, API keys, absolute paths, and backend error strings
-- are deliberately excluded from this audit table.
CREATE TABLE IF NOT EXISTS summary_generation_history (
    generation_id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'failed', 'cancelled', 'superseded', 'legacy')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    template_id TEXT NOT NULL,
    template_version INTEGER NOT NULL,
    file_sha256 TEXT NOT NULL,
    semantic_sha256 TEXT NOT NULL,
    snapshot_path_relative TEXT NOT NULL,
    resolution_source TEXT NOT NULL,
    model_provider TEXT NOT NULL,
    model_name TEXT NOT NULL,
    summary_language TEXT,
    error_category TEXT,
    snapshot_state TEXT NOT NULL DEFAULT 'available'
        CHECK (snapshot_state IN ('available', 'missing', 'corrupt', 'quarantined')),
    cleanup_batch_id TEXT,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_meeting_created
    ON summary_generation_history(meeting_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_snapshot_state
    ON summary_generation_history(snapshot_state, updated_at);

CREATE TABLE IF NOT EXISTS summary_snapshot_cleanup_audit (
    cleanup_batch_id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('preparing', 'completed', 'rolled_back')),
    file_count INTEGER NOT NULL,
    byte_count INTEGER NOT NULL,
    plan_changed INTEGER NOT NULL DEFAULT 0,
    candidate_generation_ids TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
