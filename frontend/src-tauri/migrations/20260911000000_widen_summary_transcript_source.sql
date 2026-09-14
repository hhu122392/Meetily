-- P1-6: summary provenance must name the local engine that actually produced
-- the transcript (SenseVoice / Parakeet) instead of a hardcoded "whisper".
-- SQLite cannot widen a column CHECK in place, so the table is rebuilt with the
-- same shape (columns, defaults, cascade delete) and every row is copied.

CREATE TABLE summary_generation_history_rebuilt (
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
    source_binding_schema_version INTEGER
        CHECK (source_binding_schema_version IS NULL OR source_binding_schema_version > 0),
    transcript_source TEXT
        CHECK (
            transcript_source IS NULL
            OR transcript_source IN ('whisper', 'sensevoice', 'parakeet', 'moss', 'manual')
        ),
    transcript_version_id TEXT
        CHECK (transcript_version_id IS NULL OR length(trim(transcript_version_id)) > 0),
    transcript_version INTEGER
        CHECK (transcript_version IS NULL OR transcript_version > 0),
    moss_run_id TEXT
        CHECK (moss_run_id IS NULL OR length(trim(moss_run_id)) > 0),
    transcript_activated_at TEXT,
    transcript_sha256 TEXT
        CHECK (
            transcript_sha256 IS NULL
            OR (
                length(transcript_sha256) = 64
                AND transcript_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        ),
    speaker_binding_snapshot_id TEXT
        CHECK (
            speaker_binding_snapshot_id IS NULL
            OR length(trim(speaker_binding_snapshot_id)) > 0
        ),
    speaker_binding_version INTEGER
        CHECK (speaker_binding_version IS NULL OR speaker_binding_version > 0),
    speaker_binding_sha256 TEXT
        CHECK (
            speaker_binding_sha256 IS NULL
            OR (
                length(speaker_binding_sha256) = 64
                AND speaker_binding_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        ),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

INSERT INTO summary_generation_history_rebuilt (
    generation_id, meeting_id, status, created_at, updated_at, completed_at,
    template_id, template_version, file_sha256, semantic_sha256,
    snapshot_path_relative, resolution_source, model_provider, model_name,
    summary_language, error_category, snapshot_state, cleanup_batch_id,
    source_binding_schema_version, transcript_source, transcript_version_id,
    transcript_version, moss_run_id, transcript_activated_at, transcript_sha256,
    speaker_binding_snapshot_id, speaker_binding_version, speaker_binding_sha256
)
SELECT
    generation_id, meeting_id, status, created_at, updated_at, completed_at,
    template_id, template_version, file_sha256, semantic_sha256,
    snapshot_path_relative, resolution_source, model_provider, model_name,
    summary_language, error_category, snapshot_state, cleanup_batch_id,
    source_binding_schema_version, transcript_source, transcript_version_id,
    transcript_version, moss_run_id, transcript_activated_at, transcript_sha256,
    speaker_binding_snapshot_id, speaker_binding_version, speaker_binding_sha256
FROM summary_generation_history;

DROP TABLE summary_generation_history;

ALTER TABLE summary_generation_history_rebuilt RENAME TO summary_generation_history;

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_meeting_created
    ON summary_generation_history(meeting_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_snapshot_state
    ON summary_generation_history(snapshot_state, updated_at);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_source
    ON summary_generation_history(meeting_id, transcript_source, moss_run_id);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_transcript_hash
    ON summary_generation_history(transcript_sha256);
