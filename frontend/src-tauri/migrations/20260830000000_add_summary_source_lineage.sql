-- P5 summary-source lineage. Existing history rows remain readable with NULL
-- lineage; every new P5 generation writes the complete bundle atomically.
ALTER TABLE summary_generation_history
    ADD COLUMN source_binding_schema_version INTEGER
        CHECK (source_binding_schema_version IS NULL OR source_binding_schema_version > 0);

ALTER TABLE summary_generation_history
    ADD COLUMN transcript_source TEXT
        CHECK (transcript_source IS NULL OR transcript_source IN ('whisper', 'moss', 'manual'));

ALTER TABLE summary_generation_history
    ADD COLUMN transcript_version_id TEXT
        CHECK (transcript_version_id IS NULL OR length(trim(transcript_version_id)) > 0);

ALTER TABLE summary_generation_history
    ADD COLUMN transcript_version INTEGER
        CHECK (transcript_version IS NULL OR transcript_version > 0);

ALTER TABLE summary_generation_history
    ADD COLUMN moss_run_id TEXT
        CHECK (moss_run_id IS NULL OR length(trim(moss_run_id)) > 0);

ALTER TABLE summary_generation_history
    ADD COLUMN transcript_activated_at TEXT;

ALTER TABLE summary_generation_history
    ADD COLUMN transcript_sha256 TEXT
        CHECK (
            transcript_sha256 IS NULL
            OR (
                length(transcript_sha256) = 64
                AND transcript_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        );

ALTER TABLE summary_generation_history
    ADD COLUMN speaker_binding_snapshot_id TEXT
        CHECK (
            speaker_binding_snapshot_id IS NULL
            OR length(trim(speaker_binding_snapshot_id)) > 0
        );

ALTER TABLE summary_generation_history
    ADD COLUMN speaker_binding_version INTEGER
        CHECK (speaker_binding_version IS NULL OR speaker_binding_version > 0);

ALTER TABLE summary_generation_history
    ADD COLUMN speaker_binding_sha256 TEXT
        CHECK (
            speaker_binding_sha256 IS NULL
            OR (
                length(speaker_binding_sha256) = 64
                AND speaker_binding_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        );

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_source
    ON summary_generation_history(meeting_id, transcript_source, moss_run_id);

CREATE INDEX IF NOT EXISTS idx_summary_generation_history_transcript_hash
    ON summary_generation_history(transcript_sha256);
