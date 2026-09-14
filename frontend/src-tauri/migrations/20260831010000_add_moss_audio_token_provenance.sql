-- MOSS R5: real Whisper audio-token alignment and machine correction provenance.
-- Existing R4 rows are copied byte-for-byte into the widened alignment table.
DROP INDEX IF EXISTS idx_moss_alignment_raw_segment;

ALTER TABLE moss_candidate_segment_alignment
    RENAME TO moss_candidate_segment_alignment_r4;

CREATE TABLE moss_candidate_segment_alignment (
    segment_id TEXT PRIMARY KEY NOT NULL,
    raw_segment_index INTEGER NOT NULL CHECK (raw_segment_index >= 0),
    raw_start_ms INTEGER NOT NULL CHECK (raw_start_ms >= 0),
    raw_end_ms INTEGER NOT NULL CHECK (raw_end_ms >= raw_start_ms),
    raw_text_sha256 TEXT NOT NULL
        CHECK (length(raw_text_sha256) = 64 AND raw_text_sha256 NOT GLOB '*[^0-9a-f]*'),
    alignment_method TEXT NOT NULL
        CHECK (alignment_method IN ('moss_segment', 'source_transcript_segment', 'whisper_audio_token')),
    confidence REAL CHECK (confidence IS NULL OR (confidence >= 0.0 AND confidence <= 1.0)),
    source_anchor_ids_json TEXT NOT NULL,
    source_transcript_sha256 TEXT NOT NULL
        CHECK (length(source_transcript_sha256) = 64 AND source_transcript_sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    CHECK (
        (alignment_method = 'moss_segment' AND confidence IS NULL AND source_anchor_ids_json = '[]')
        OR
        (alignment_method IN ('source_transcript_segment', 'whisper_audio_token')
            AND confidence IS NOT NULL AND source_anchor_ids_json <> '[]')
    ),
    FOREIGN KEY (segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE
);

INSERT INTO moss_candidate_segment_alignment (
    segment_id, raw_segment_index, raw_start_ms, raw_end_ms,
    raw_text_sha256, alignment_method, confidence,
    source_anchor_ids_json, source_transcript_sha256, created_at
)
SELECT segment_id, raw_segment_index, raw_start_ms, raw_end_ms,
       raw_text_sha256, alignment_method, confidence,
       source_anchor_ids_json, source_transcript_sha256, created_at
  FROM moss_candidate_segment_alignment_r4;

DROP TABLE moss_candidate_segment_alignment_r4;

CREATE INDEX idx_moss_alignment_raw_segment
    ON moss_candidate_segment_alignment(raw_segment_index, segment_id);

CREATE TABLE moss_audio_token_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('verified', 'fallback')),
    audio_sha256 TEXT NOT NULL
        CHECK (length(audio_sha256) = 64 AND audio_sha256 NOT GLOB '*[^0-9a-f]*'),
    audio_duration_ms INTEGER CHECK (audio_duration_ms IS NULL OR audio_duration_ms > 0),
    model_name TEXT,
    model_sha256 TEXT
        CHECK (model_sha256 IS NULL OR (length(model_sha256) = 64 AND model_sha256 NOT GLOB '*[^0-9a-f]*')),
    program_sha256 TEXT
        CHECK (program_sha256 IS NULL OR (length(program_sha256) = 64 AND program_sha256 NOT GLOB '*[^0-9a-f]*')),
    parameters_sha256 TEXT
        CHECK (parameters_sha256 IS NULL OR (length(parameters_sha256) = 64 AND parameters_sha256 NOT GLOB '*[^0-9a-f]*')),
    token_track_sha256 TEXT
        CHECK (token_track_sha256 IS NULL OR (length(token_track_sha256) = 64 AND token_track_sha256 NOT GLOB '*[^0-9a-f]*')),
    backend TEXT,
    parameters_json TEXT,
    global_match_coverage REAL
        CHECK (global_match_coverage IS NULL OR (global_match_coverage >= 0.0 AND global_match_coverage <= 1.0)),
    token_aligned_segment_count INTEGER NOT NULL CHECK (token_aligned_segment_count >= 0),
    fallback_raw_segment_count INTEGER NOT NULL CHECK (fallback_raw_segment_count >= 0),
    fallback_reason TEXT,
    created_at TEXT NOT NULL,
    CHECK (
        (status = 'verified'
            AND audio_duration_ms IS NOT NULL
            AND model_name IS NOT NULL
            AND model_sha256 IS NOT NULL
            AND program_sha256 IS NOT NULL
            AND parameters_sha256 IS NOT NULL
            AND token_track_sha256 IS NOT NULL
            AND backend IS NOT NULL
            AND parameters_json IS NOT NULL)
        OR
        (status = 'fallback'
            AND audio_duration_ms IS NULL
            AND model_name IS NULL
            AND model_sha256 IS NULL
            AND program_sha256 IS NULL
            AND parameters_sha256 IS NULL
            AND token_track_sha256 IS NULL
            AND backend IS NULL
            AND parameters_json IS NULL
            AND global_match_coverage IS NULL
            AND token_aligned_segment_count = 0
            AND fallback_reason IS NOT NULL)
    ),
    CHECK ((fallback_raw_segment_count > 0) = (fallback_reason IS NOT NULL)),
    UNIQUE (run_id, token_track_sha256),
    FOREIGN KEY (run_id) REFERENCES moss_transcription_runs(run_id) ON DELETE CASCADE
);

CREATE TABLE moss_audio_token_source_chunks (
    run_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK (end_ms > start_ms),
    sample_count INTEGER NOT NULL CHECK (sample_count > 0),
    text_sha256 TEXT NOT NULL
        CHECK (length(text_sha256) = 64 AND text_sha256 NOT GLOB '*[^0-9a-f]*'),
    first_global_token_index INTEGER NOT NULL CHECK (first_global_token_index >= 0),
    token_count INTEGER NOT NULL CHECK (token_count >= 0),
    PRIMARY KEY (run_id, chunk_index),
    FOREIGN KEY (run_id) REFERENCES moss_audio_token_runs(run_id) ON DELETE CASCADE
);

CREATE TABLE moss_audio_tokens (
    run_id TEXT NOT NULL,
    global_token_index INTEGER NOT NULL CHECK (global_token_index >= 0),
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    whisper_segment_index INTEGER NOT NULL CHECK (whisper_segment_index >= 0),
    whisper_token_index INTEGER NOT NULL CHECK (whisper_token_index >= 0),
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK (end_ms >= start_ms),
    token_text TEXT NOT NULL CHECK (length(trim(token_text)) > 0),
    probability REAL NOT NULL CHECK (probability >= 0.0 AND probability <= 1.0),
    PRIMARY KEY (run_id, global_token_index),
    FOREIGN KEY (run_id, chunk_index)
        REFERENCES moss_audio_token_source_chunks(run_id, chunk_index) ON DELETE CASCADE
);

CREATE TABLE moss_candidate_audio_token_boundary (
    segment_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    first_token_index INTEGER NOT NULL CHECK (first_token_index >= 0),
    last_token_index INTEGER NOT NULL CHECK (last_token_index >= first_token_index),
    token_track_sha256 TEXT NOT NULL
        CHECK (length(token_track_sha256) = 64 AND token_track_sha256 NOT GLOB '*[^0-9a-f]*'),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    created_at TEXT NOT NULL,
    FOREIGN KEY (segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE,
    FOREIGN KEY (run_id, first_token_index)
        REFERENCES moss_audio_tokens(run_id, global_token_index),
    FOREIGN KEY (run_id, last_token_index)
        REFERENCES moss_audio_tokens(run_id, global_token_index),
    FOREIGN KEY (run_id, token_track_sha256)
        REFERENCES moss_audio_token_runs(run_id, token_track_sha256)
);

CREATE TABLE moss_machine_term_correction_source (
    correction_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    term_id TEXT NOT NULL,
    context_sha256 TEXT NOT NULL
        CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    token_track_sha256 TEXT NOT NULL
        CHECK (length(token_track_sha256) = 64 AND token_track_sha256 NOT GLOB '*[^0-9a-f]*'),
    model_sha256 TEXT NOT NULL
        CHECK (length(model_sha256) = 64 AND model_sha256 NOT GLOB '*[^0-9a-f]*'),
    first_token_index INTEGER NOT NULL CHECK (first_token_index >= 0),
    last_token_index INTEGER NOT NULL CHECK (last_token_index >= first_token_index),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    created_at TEXT NOT NULL,
    FOREIGN KEY (correction_id) REFERENCES moss_term_corrections(correction_id) ON DELETE CASCADE,
    FOREIGN KEY (run_id, first_token_index)
        REFERENCES moss_audio_tokens(run_id, global_token_index),
    FOREIGN KEY (run_id, last_token_index)
        REFERENCES moss_audio_tokens(run_id, global_token_index),
    FOREIGN KEY (run_id, token_track_sha256)
        REFERENCES moss_audio_token_runs(run_id, token_track_sha256)
);
