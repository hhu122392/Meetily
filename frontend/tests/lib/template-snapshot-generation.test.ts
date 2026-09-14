import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

test('generation resolves the persisted meeting template before spawning the summary service', () => {
  const commands = read('src-tauri/src/summary/commands.rs');
  const service = read('src-tauri/src/summary/service.rs');

  assert.match(commands, /resolve_template_for_generation[\s\S]+capture_snapshot[\s\S]+create_or_reset_process_with_generation_history[\s\S]+process_transcript_background/);
  assert.match(commands, /runtime_template[\s\S]+template_fingerprint[\s\S]+snapshot_link/);
  assert.doesNotMatch(service, /templates::get_template\(&template_id\)/);
});

test('snapshot preflight is rolled back on database initialization failures', () => {
  const commands = read('src-tauri/src/summary/commands.rs');
  const snapshot = read('src-tauri/src/summary/template_snapshot.rs');

  assert.match(commands, /remove_snapshot_after_preflight_failure\(&snapshot_path\)/);
  assert.match(snapshot, /create_new\(true\)/);
  assert.match(snapshot, /file\.sync_all\(\)/);
  assert.match(snapshot, /fs::rename\(&temporary_path, &final_path\)/);
});

test('auto summary waits for meeting template state and claims a single generation', () => {
  const page = read('src/app/meeting-details/page-content.tsx');

  assert.match(page, /!templates\.isLoading/);
  assert.match(page, /!templates\.isSaving/);
  assert.match(page, /!templates\.error/);
  assert.match(page, /!templates\.issue/);
  assert.match(page, /autoGenerationStartedForMeetingRef\.current = meeting\.id/);
});

test('regeneration explicitly offers historical snapshot and latest-template modes', () => {
  const controls = read('src/components/MeetingDetails/SummaryGeneratorButtonGroup.tsx');
  const hook = read('src/hooks/meeting-details/useSummaryGeneration.ts');

  assert.match(controls, /chooseRegenerationMode\('historical'\)/);
  assert.match(controls, /chooseRegenerationMode\('latest'\)/);
  assert.match(hook, /api_list_meeting_template_snapshots/);
  assert.match(hook, /historicalGenerationId/);
});

test('summary results and process metadata carry the immutable snapshot link', () => {
  const commands = read('src-tauri/src/summary/commands.rs');
  const service = read('src-tauri/src/summary/service.rs');
  const repository = read('src-tauri/src/database/repositories/summary.rs');

  assert.match(commands, /snapshot_link_json\(&snapshot_link\)/);
  assert.match(service, /"template_snapshot"\.to_owned\(\)/);
  assert.match(repository, /authoritative_snapshot/);
  assert.match(repository, /object\.insert\("template_snapshot"\.to_owned\(\), snapshot\)/);
  assert.match(repository, /object\.remove\("template_snapshot"\)/);
});

test('generation terminal writes use a metadata compare-and-swap guard', () => {
  const service = read('src-tauri/src/summary/service.rs');
  const repository = read('src-tauri/src/database/repositories/summary.rs');

  assert.match(service, /update_process_completed_for_generation/);
  assert.match(service, /update_process_failed_for_generation/);
  assert.match(service, /update_process_cancelled_for_generation/);
  assert.match(repository, /WHERE meeting_id = \? AND status = 'PENDING' AND metadata = \?/);
  assert.match(repository, /metadata_generation_matches\(&metadata_raw, generation_id\)/);
  assert.match(service, /Ignored stale completed generation/);
});

test('failed regeneration restores both the prior result and its snapshot link', () => {
  const repository = read('src-tauri/src/database/repositories/summary.rs');

  assert.match(repository, /let restored_result = result_backup\.or\(current_result\)/);
  assert.match(repository, /template_snapshot_metadata_from_result/);
  assert.match(repository, /metadata = \?/);
  assert.match(repository, /result_backup = NULL/);
});

test('cancellation registry is generation-scoped and supersedes older work', () => {
  const service = read('src-tauri/src/summary/service.rs');
  const commands = read('src-tauri/src/summary/commands.rs');

  assert.match(service, /struct RegisteredCancellation/);
  assert.match(service, /previous\.token\.cancel\(\)/);
  assert.match(service, /registered\.generation_id == generation_id/);
  assert.match(commands, /register_summary_generation\(&m_id, &generation_id\)/);
  assert.match(commands, /update_process_cancelled_for_generation/);
});

test('summary completion cannot rename the meeting even after its result is accepted', () => {
  const service = read('src-tauri/src/summary/service.rs');
  const productionService = service.split(/\r?\n#\[cfg\(test\)\]\r?\nmod tests/)[0];

  assert.match(productionService, /persist_completed_summary\(/);
  assert.doesNotMatch(
    productionService,
    /MeetingsRepository::update_meeting_(?:name|title)/,
  );
});

test('every accepted generation appends a non-sensitive durable history record', () => {
  const migration = read('src-tauri/migrations/20260824000000_add_summary_generation_history.sql');
  const lineageMigration = read('src-tauri/migrations/20260830000000_add_summary_source_lineage.sql');
  const commands = read('src-tauri/src/summary/commands.rs');
  const repository = read('src-tauri/src/database/repositories/summary.rs');

  assert.match(migration, /CREATE TABLE IF NOT EXISTS summary_generation_history/);
  assert.doesNotMatch(migration, /\b(transcript_text|custom_prompt|api_key|absolute_path)\s+TEXT\b/i);
  assert.match(lineageMigration, /transcript_source TEXT/);
  assert.match(lineageMigration, /moss_run_id TEXT/);
  assert.match(lineageMigration, /transcript_sha256 TEXT/);
  assert.match(lineageMigration, /speaker_binding_sha256 TEXT/);
  assert.doesNotMatch(lineageMigration, /\b(transcript_text|custom_prompt|api_key|absolute_path)\s+TEXT\b/i);
  assert.match(commands, /NewSummaryGenerationHistory/);
  assert.match(repository, /generation\.transcript_source/);
  assert.match(repository, /generation\.moss_run_id/);
  assert.match(repository, /generation\.transcript_sha256/);
  assert.match(repository, /generation\.speaker_binding_sha256/);
  assert.match(repository, /list_generation_history[\s\S]+transcript_source/);
  assert.match(repository, /status = 'superseded'/);
  assert.match(repository, /UPDATE summary_generation_history[\s\S]+status = 'completed'/);
  assert.match(repository, /summary_error_category/);
});

test('snapshot cleanup is previewed, reference-revalidated, and quarantined transactionally', () => {
  const lifecycle = read('src-tauri/src/summary/generation_lifecycle.rs');
  const snapshot = read('src-tauri/src/summary/template_snapshot.rs');

  assert.match(lifecycle, /api_preview_template_snapshot_cleanup/);
  assert.match(lifecycle, /preview_token/);
  assert.match(lifecycle, /expected[\s\S]+\.intersection\(&current_candidate_ids\)/);
  assert.match(lifecycle, /current_references_in_transaction/);
  assert.match(lifecycle, /restore_quarantined_snapshot/);
  assert.match(snapshot, /fs::rename\(&source, &destination\)/);
  assert.doesNotMatch(lifecycle, /remove_file/);
});

test('generation history UI displays safe categories and retries an exact snapshot', () => {
  const dialog = read('src/components/MeetingDetails/SummaryGenerationHistoryDialog.tsx');
  const hook = read('src/hooks/meeting-details/useSummaryGeneration.ts');

  assert.match(dialog, /item\.generationId/);
  assert.match(dialog, /item\.transcriptSource/);
  assert.match(dialog, /item\.mossRunId/);
  assert.match(dialog, /details\.summarySourceBinding\.transcriptSha256/);
  assert.match(dialog, /details\.summarySourceBinding\.speakerBindingSha256/);
  assert.match(dialog, /item\.errorCategory/);
  assert.match(dialog, /canRetryWithSnapshot/);
  assert.match(dialog, /onRetryGeneration\(generationId\)/);
  assert.match(dialog, /previewSnapshotCleanup/);
  assert.match(dialog, /executeSnapshotCleanup/);
  assert.match(hook, /requestedHistoricalGenerationId/);
});

test('every accepted generation appends a non-sensitive durable history record', () => {
  const migration = read('src-tauri/migrations/20260824000000_add_summary_generation_history.sql');
  const commands = read('src-tauri/src/summary/commands.rs');
  const repository = read('src-tauri/src/database/repositories/summary.rs');

  assert.match(migration, /CREATE TABLE IF NOT EXISTS summary_generation_history/);
  assert.doesNotMatch(migration, /\b(transcript_text|custom_prompt|api_key|absolute_path)\s+TEXT\b/i);
  assert.match(commands, /NewSummaryGenerationHistory/);
  assert.match(repository, /status = 'superseded'/);
  assert.match(repository, /UPDATE summary_generation_history[\s\S]+status = 'completed'/);
  assert.match(repository, /summary_error_category/);
});

test('snapshot cleanup is previewed, reference-revalidated, and quarantined transactionally', () => {
  const lifecycle = read('src-tauri/src/summary/generation_lifecycle.rs');
  const snapshot = read('src-tauri/src/summary/template_snapshot.rs');

  assert.match(lifecycle, /api_preview_template_snapshot_cleanup/);
  assert.match(lifecycle, /preview_token/);
  assert.match(lifecycle, /expected[\s\S]+\.intersection\(&current_candidate_ids\)/);
  assert.match(lifecycle, /current_references_in_transaction/);
  assert.match(lifecycle, /restore_quarantined_snapshot/);
  assert.match(snapshot, /fs::rename\(&source, &destination\)/);
  assert.doesNotMatch(lifecycle, /remove_file/);
});

test('generation history UI displays safe categories and retries an exact snapshot', () => {
  const dialog = read('src/components/MeetingDetails/SummaryGenerationHistoryDialog.tsx');
  const hook = read('src/hooks/meeting-details/useSummaryGeneration.ts');

  assert.match(dialog, /item\.generationId/);
  assert.match(dialog, /item\.errorCategory/);
  assert.match(dialog, /canRetryWithSnapshot/);
  assert.match(dialog, /onRetryGeneration\(generationId\)/);
  assert.match(dialog, /previewSnapshotCleanup/);
  assert.match(dialog, /executeSnapshotCleanup/);
  assert.match(hook, /requestedHistoricalGenerationId/);
});
