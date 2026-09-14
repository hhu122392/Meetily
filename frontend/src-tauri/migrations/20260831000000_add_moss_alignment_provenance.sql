-- MOSS R4 alignment provenance and objective audio-tail diagnostics.
-- Append-only: existing runs and candidates remain valid without rows here.
CREATE TABLE IF NOT EXISTS moss_candidate_segment_alignment (
    segment_id TEXT PRIMARY KEY NOT NULL,
    raw_segment_index INTEGER NOT NULL CHECK (raw_segment_index >= 0),
    raw_start_ms INTEGER NOT NULL CHECK (raw_start_ms >= 0),
    raw_end_ms INTEGER NOT NULL CHECK (raw_end_ms >= raw_start_ms),
    raw_text_sha256 TEXT NOT NULL
        CHECK (length(raw_text_sha256) = 64 AND raw_text_sha256 NOT GLOB '*[^0-9a-f]*'),
    alignment_method TEXT NOT NULL
        CHECK (alignment_method IN ('moss_segment', 'source_transcript_segment')),
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    source_anchor_ids_json TEXT NOT NULL,
    source_transcript_sha256 TEXT NOT NULL
        CHECK (length(source_transcript_sha256) = 64 AND source_transcript_sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    CHECK (
        (alignment_method = 'moss_segment' AND confidence IS NULL AND source_anchor_ids_json = '[]')
        OR
        (alignment_method = 'source_transcript_segment' AND confidence IS NOT NULL AND source_anchor_ids_json <> '[]')
    ),
    FOREIGN KEY (segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_moss_alignment_raw_segment
    ON moss_candidate_segment_alignment(raw_segment_index, segment_id);

CREATE TABLE IF NOT EXISTS moss_run_diagnostics (
    run_id TEXT PRIMARY KEY NOT NULL,
    audio_duration_ms INTEGER NOT NULL CHECK (audio_duration_ms > 0),
    activity_frame_ms INTEGER NOT NULL CHECK (activity_frame_ms > 0),
    activity_threshold_dbfs REAL NOT NULL,
    first_active_ms INTEGER CHECK (first_active_ms IS NULL OR first_active_ms >= 0),
    last_active_ms INTEGER CHECK (last_active_ms IS NULL OR last_active_ms >= first_active_ms),
    model_last_timestamp_ms INTEGER NOT NULL CHECK (model_last_timestamp_ms >= 0),
    tail_delta_ms INTEGER,
    aligned_segment_count INTEGER NOT NULL CHECK (aligned_segment_count >= 0),
    fallback_segment_count INTEGER NOT NULL CHECK (fallback_segment_count >= 0),
    source_anchor_count INTEGER NOT NULL CHECK (source_anchor_count >= 0),
    source_hash_verified INTEGER NOT NULL CHECK (source_hash_verified IN (0, 1)),
    source_expected_sha256 TEXT NOT NULL
        CHECK (length(source_expected_sha256) = 64 AND source_expected_sha256 NOT GLOB '*[^0-9a-f]*'),
    source_actual_sha256 TEXT NOT NULL
        CHECK (length(source_actual_sha256) = 64 AND source_actual_sha256 NOT GLOB '*[^0-9a-f]*'),
    fallback_reason TEXT,
    created_at TEXT NOT NULL,
    CHECK (
        (last_active_ms IS NULL AND tail_delta_ms IS NULL)
        OR
        (last_active_ms IS NOT NULL AND tail_delta_ms = model_last_timestamp_ms - last_active_ms)
    ),
    CHECK (
        (source_hash_verified = 1 AND source_expected_sha256 = source_actual_sha256)
        OR source_hash_verified = 0
    ),
    FOREIGN KEY (run_id) REFERENCES moss_transcription_runs(run_id) ON DELETE CASCADE
);
