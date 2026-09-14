-- MOSS P3 candidate store. This migration is append-only: it creates new
-- tables and indexes without changing or deleting existing Meetily data.
CREATE TABLE IF NOT EXISTS moss_transcription_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('running', 'completed', 'failed', 'cancelled')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    source_transcript_sha256 TEXT NOT NULL
        CHECK (length(source_transcript_sha256) = 64 AND source_transcript_sha256 NOT GLOB '*[^0-9a-f]*'),
    audio_sha256 TEXT NOT NULL
        CHECK (length(audio_sha256) = 64 AND audio_sha256 NOT GLOB '*[^0-9a-f]*'),
    model_sha256 TEXT NOT NULL
        CHECK (length(model_sha256) = 64 AND model_sha256 NOT GLOB '*[^0-9a-f]*'),
    runtime_sha256 TEXT NOT NULL
        CHECK (length(runtime_sha256) = 64 AND runtime_sha256 NOT GLOB '*[^0-9a-f]*'),
    context_sha256 TEXT NOT NULL
        CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    backend_name TEXT NOT NULL CHECK (length(trim(backend_name)) > 0),
    backend_version TEXT NOT NULL CHECK (length(trim(backend_version)) > 0),
    runtime_version TEXT NOT NULL CHECK (length(trim(runtime_version)) > 0),
    model_revision TEXT NOT NULL CHECK (length(trim(model_revision)) > 0),
    device_name TEXT,
    raw_output_sha256 TEXT
        CHECK (raw_output_sha256 IS NULL OR (length(raw_output_sha256) = 64 AND raw_output_sha256 NOT GLOB '*[^0-9a-f]*')),
    clean_output_sha256 TEXT
        CHECK (clean_output_sha256 IS NULL OR (length(clean_output_sha256) = 64 AND clean_output_sha256 NOT GLOB '*[^0-9a-f]*')),
    candidate_sha256 TEXT
        CHECK (candidate_sha256 IS NULL OR (length(candidate_sha256) = 64 AND candidate_sha256 NOT GLOB '*[^0-9a-f]*')),
    segment_count INTEGER NOT NULL DEFAULT 0 CHECK (segment_count >= 0),
    wall_elapsed_ms INTEGER CHECK (wall_elapsed_ms IS NULL OR wall_elapsed_ms >= 0),
    wall_rtf REAL CHECK (wall_rtf IS NULL OR wall_rtf >= 0.0),
    peak_memory_bytes INTEGER CHECK (peak_memory_bytes IS NULL OR peak_memory_bytes >= 0),
    error_code TEXT,
    CHECK (
        status <> 'completed'
        OR (
            completed_at IS NOT NULL
            AND candidate_sha256 IS NOT NULL
            AND raw_output_sha256 IS NOT NULL
            AND clean_output_sha256 IS NOT NULL
            AND segment_count > 0
        )
    ),
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

-- SQLite enforces the one-live-run rule even when two callers race.
CREATE UNIQUE INDEX IF NOT EXISTS idx_moss_runs_one_running_per_meeting
    ON moss_transcription_runs(meeting_id)
    WHERE status = 'running';

CREATE INDEX IF NOT EXISTS idx_moss_runs_meeting_created
    ON moss_transcription_runs(meeting_id, created_at DESC);

CREATE TABLE IF NOT EXISTS moss_candidate_segments (
    segment_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    segment_index INTEGER NOT NULL CHECK (segment_index >= 0),
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK (end_ms >= start_ms),
    speaker_label TEXT NOT NULL
        CHECK (
            length(speaker_label) >= 3
            AND substr(speaker_label, 1, 1) = 'S'
            AND substr(speaker_label, 2) NOT GLOB '*[^0-9]*'
        ),
    raw_text TEXT NOT NULL CHECK (length(trim(raw_text)) > 0),
    created_at TEXT NOT NULL,
    UNIQUE (run_id, segment_index),
    FOREIGN KEY (run_id) REFERENCES moss_transcription_runs(run_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_moss_candidate_segments_run_order
    ON moss_candidate_segments(run_id, segment_index);

-- A correction stores both the exact replacement and the full materialized
-- segment text. Reverting the newest active revision exposes the prior one.
CREATE TABLE IF NOT EXISTS moss_term_corrections (
    correction_id TEXT PRIMARY KEY NOT NULL,
    segment_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    original_text TEXT NOT NULL CHECK (length(original_text) > 0),
    replacement_text TEXT NOT NULL CHECK (length(replacement_text) > 0),
    result_text TEXT NOT NULL CHECK (length(trim(result_text)) > 0),
    start_char INTEGER NOT NULL CHECK (start_char >= 0),
    end_char INTEGER NOT NULL CHECK (end_char > start_char),
    rule_id TEXT NOT NULL CHECK (length(trim(rule_id)) > 0),
    context_sha256 TEXT NOT NULL
        CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    reverted_at TEXT,
    UNIQUE (segment_id, revision),
    FOREIGN KEY (segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_moss_term_corrections_segment_revision
    ON moss_term_corrections(segment_id, revision DESC);

-- Speaker bindings are snapshots of the meeting-context identity. They do not
-- reuse transcripts.speaker, whose existing meaning is microphone/system audio.
CREATE TABLE IF NOT EXISTS moss_speaker_bindings (
    binding_id TEXT PRIMARY KEY NOT NULL,
    run_id TEXT NOT NULL,
    speaker_label TEXT NOT NULL,
    person_id TEXT NOT NULL CHECK (length(trim(person_id)) > 0),
    person_display_name TEXT NOT NULL CHECK (length(trim(person_display_name)) > 0),
    context_sha256 TEXT NOT NULL
        CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    FOREIGN KEY (run_id) REFERENCES moss_transcription_runs(run_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_moss_speaker_bindings_one_active
    ON moss_speaker_bindings(run_id, speaker_label)
    WHERE revoked_at IS NULL;

-- One active per-segment override can replace text, the resolved person, or
-- both. Historical overrides remain available after they are revoked.
CREATE TABLE IF NOT EXISTS moss_segment_overrides (
    override_id TEXT PRIMARY KEY NOT NULL,
    segment_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    replacement_text TEXT
        CHECK (replacement_text IS NULL OR length(trim(replacement_text)) > 0),
    person_id TEXT,
    person_display_name TEXT,
    context_sha256 TEXT NOT NULL
        CHECK (length(context_sha256) = 64 AND context_sha256 NOT GLOB '*[^0-9a-f]*'),
    reason_code TEXT NOT NULL CHECK (length(trim(reason_code)) > 0),
    created_at TEXT NOT NULL,
    revoked_at TEXT,
    CHECK (
        replacement_text IS NOT NULL
        OR (person_id IS NOT NULL AND person_display_name IS NOT NULL)
    ),
    CHECK (
        (person_id IS NULL AND person_display_name IS NULL)
        OR (
            person_id IS NOT NULL
            AND length(trim(person_id)) > 0
            AND person_display_name IS NOT NULL
            AND length(trim(person_display_name)) > 0
        )
    ),
    UNIQUE (segment_id, revision),
    FOREIGN KEY (segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_moss_segment_overrides_one_active
    ON moss_segment_overrides(segment_id)
    WHERE revoked_at IS NULL;

-- The before/after transcript documents make activation and rollback auditable
-- and independent from later correction or binding edits.
CREATE TABLE IF NOT EXISTS moss_activation_snapshots (
    activation_id TEXT PRIMARY KEY NOT NULL,
    meeting_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'rolled_back')),
    activated_at TEXT NOT NULL,
    rolled_back_at TEXT,
    pre_activation_transcript_sha256 TEXT NOT NULL
        CHECK (length(pre_activation_transcript_sha256) = 64 AND pre_activation_transcript_sha256 NOT GLOB '*[^0-9a-f]*'),
    activated_transcript_sha256 TEXT NOT NULL
        CHECK (length(activated_transcript_sha256) = 64 AND activated_transcript_sha256 NOT GLOB '*[^0-9a-f]*'),
    candidate_sha256 TEXT NOT NULL
        CHECK (length(candidate_sha256) = 64 AND candidate_sha256 NOT GLOB '*[^0-9a-f]*'),
    pre_activation_transcripts_json TEXT NOT NULL,
    activated_transcripts_json TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE,
    FOREIGN KEY (run_id) REFERENCES moss_transcription_runs(run_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_moss_activation_one_active_per_meeting
    ON moss_activation_snapshots(meeting_id)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_moss_activation_meeting_created
    ON moss_activation_snapshots(meeting_id, activated_at DESC);

CREATE TABLE IF NOT EXISTS moss_activation_segments (
    activation_id TEXT NOT NULL,
    segment_index INTEGER NOT NULL CHECK (segment_index >= 0),
    transcript_id TEXT NOT NULL,
    candidate_segment_id TEXT NOT NULL,
    start_ms INTEGER NOT NULL CHECK (start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK (end_ms >= start_ms),
    speaker_label TEXT NOT NULL,
    resolved_person_id TEXT,
    resolved_person_display_name TEXT,
    text TEXT NOT NULL CHECK (length(trim(text)) > 0),
    correction_id TEXT,
    override_id TEXT,
    binding_id TEXT,
    PRIMARY KEY (activation_id, segment_index),
    UNIQUE (activation_id, transcript_id),
    FOREIGN KEY (activation_id) REFERENCES moss_activation_snapshots(activation_id) ON DELETE CASCADE,
    FOREIGN KEY (candidate_segment_id) REFERENCES moss_candidate_segments(segment_id) ON DELETE CASCADE,
    FOREIGN KEY (correction_id) REFERENCES moss_term_corrections(correction_id) ON DELETE SET NULL,
    FOREIGN KEY (override_id) REFERENCES moss_segment_overrides(override_id) ON DELETE SET NULL,
    FOREIGN KEY (binding_id) REFERENCES moss_speaker_bindings(binding_id) ON DELETE SET NULL,
    CHECK (
        (resolved_person_id IS NULL AND resolved_person_display_name IS NULL)
        OR (resolved_person_id IS NOT NULL AND resolved_person_display_name IS NOT NULL)
    )
);
