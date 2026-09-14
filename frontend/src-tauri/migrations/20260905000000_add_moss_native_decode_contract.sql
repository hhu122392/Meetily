-- Keep legacy MOSS rows readable while requiring every new run to record the
-- exact language and native decode parameters used by protocol v3.
ALTER TABLE moss_transcription_runs ADD COLUMN language_requested TEXT;
ALTER TABLE moss_transcription_runs ADD COLUMN language_resolved TEXT;
ALTER TABLE moss_transcription_runs ADD COLUMN decode_parameters_json TEXT;
ALTER TABLE moss_transcription_runs ADD COLUMN decode_parameters_sha256 TEXT
    CHECK (
        decode_parameters_sha256 IS NULL
        OR (
            length(decode_parameters_sha256) = 64
            AND decode_parameters_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    );

CREATE TRIGGER moss_runs_require_decode_contract_on_insert
BEFORE INSERT ON moss_transcription_runs
WHEN NEW.language_requested IS NULL
    OR NEW.language_requested <> 'zh-CN'
    OR NEW.decode_parameters_json IS NULL
    OR NEW.decode_parameters_json <> '{"language":"zh","timestamps":"segment","diarize":"on"}'
    OR NEW.decode_parameters_sha256 IS NULL
    OR NEW.decode_parameters_sha256 <> '1b8ee2dde060ce156ff43cd9b08a19f54060018bab428373b7094ee887e0d47e'
BEGIN
    SELECT RAISE(ABORT, 'MOSS native decode contract is required');
END;

CREATE TRIGGER moss_runs_require_resolved_language_on_completion
BEFORE UPDATE OF status ON moss_transcription_runs
WHEN NEW.status = 'completed'
    AND (
        NEW.language_requested IS NULL
        OR NEW.language_requested <> 'zh-CN'
        OR NEW.language_resolved IS NULL
        OR NEW.language_resolved <> 'zh-CN'
        OR NEW.decode_parameters_json IS NULL
        OR NEW.decode_parameters_json <> '{"language":"zh","timestamps":"segment","diarize":"on"}'
        OR NEW.decode_parameters_sha256 IS NULL
        OR NEW.decode_parameters_sha256 <> '1b8ee2dde060ce156ff43cd9b08a19f54060018bab428373b7094ee887e0d47e'
    )
BEGIN
    SELECT RAISE(ABORT, 'MOSS completed run lacks native decode proof');
END;
