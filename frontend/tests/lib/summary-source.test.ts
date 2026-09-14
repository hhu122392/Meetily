import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import {
  evaluateSummaryFreshness,
  isSummarySourceBinding,
  readSummaryFreshness,
  readSummarySourceBinding,
  selectActivatedTranscriptVersion,
} from '../../src/lib/summary-source';
import type {
  SummarySourceBinding,
  TranscriptVersionPointer,
} from '../../src/types/summary-source';

const hash = (character: string) => character.repeat(64);
const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

function binding(): SummarySourceBinding {
  return {
    schemaVersion: 1,
    meetingId: 'meeting_1',
    transcriptVersionId: 'transcript_1',
    transcriptVersion: 1,
    transcriptSource: 'whisper',
    mossRunId: null,
    transcriptActivatedAt: '2026-08-29T01:02:03Z',
    transcriptSha256: hash('a'),
    speakerBindingSnapshotId: 'bindings_1',
    speakerBindingVersion: 1,
    speakerBindingSha256: hash('b'),
    template: {
      templateId: 'standard_meeting',
      templateVersion: 2,
      templateFileSha256: hash('c'),
      templateSemanticSha256: hash('d'),
    },
  };
}

function version(
  state: TranscriptVersionPointer['state'],
  sourceKind: TranscriptVersionPointer['sourceKind'],
): TranscriptVersionPointer {
  return {
    meetingId: 'meeting_1',
    transcriptVersionId: `${sourceKind}_${state}`,
    transcriptVersion: 1,
    sourceKind,
    mossRunId: sourceKind === 'moss' ? 'run_1' : null,
    state,
  };
}

test('candidate MOSS is never selected while Whisper remains active', () => {
  const selected = selectActivatedTranscriptVersion('meeting_1', [
    version('active', 'whisper'),
    version('candidate', 'moss'),
  ]);
  assert.equal(selected.sourceKind, 'whisper');
});

test('activated MOSS is selected and failed MOSS does not block Whisper fallback', () => {
  const activated = selectActivatedTranscriptVersion('meeting_1', [
    version('candidate', 'whisper'),
    version('active', 'moss'),
  ]);
  assert.equal(activated.sourceKind, 'moss');

  const fallback = selectActivatedTranscriptVersion('meeting_1', [
    version('active', 'whisper'),
    version('failed', 'moss'),
  ]);
  assert.equal(fallback.sourceKind, 'whisper');
});

test('an active manual transcript version remains a distinct traceable source', () => {
  const manual = version('active', 'manual');
  assert.equal(selectActivatedTranscriptVersion('meeting_1', [manual]).sourceKind, 'manual');
  assert.equal(manual.mossRunId, null);
});

test('zero or multiple active versions fail closed', () => {
  assert.throws(
    () => selectActivatedTranscriptVersion('meeting_1', [version('candidate', 'moss')]),
    /SUMMARY_SOURCE_NO_ACTIVE_VERSION/,
  );
  assert.throws(
    () =>
      selectActivatedTranscriptVersion('meeting_1', [
        version('active', 'whisper'),
        version('active', 'moss'),
      ]),
    /SUMMARY_SOURCE_MULTIPLE_ACTIVE_VERSIONS/,
  );
});

test('transcript, speaker binding, and template changes make a summary stale', () => {
  const generated = binding();
  const current = structuredClone(generated);
  current.transcriptVersion = 2;
  current.transcriptSha256 = hash('e');
  current.speakerBindingVersion = 2;
  current.speakerBindingSha256 = hash('f');
  current.template.templateVersion = 3;

  assert.deepEqual(evaluateSummaryFreshness(generated, current), {
    status: 'stale',
    reasons: [
      'transcript_version_changed',
      'transcript_content_changed',
      'speaker_bindings_changed',
      'template_changed',
    ],
  });
});

test('lineage reader accepts complete native metadata and rejects invented evidence', () => {
  const source = binding();
  const legacyWhisper = { ...source, transcriptActivatedAt: null };
  const unactivatedMoss = {
    ...source,
    transcriptSource: 'moss' as const,
    mossRunId: 'run_1',
    transcriptActivatedAt: null,
  };

  assert.equal(isSummarySourceBinding(legacyWhisper), true);
  assert.equal(isSummarySourceBinding(unactivatedMoss), false);
  assert.deepEqual(readSummarySourceBinding({ sourceBinding: source }), source);
  assert.deepEqual(readSummarySourceBinding({ summarySourceBinding: source }), source);
  assert.deepEqual(
    readSummarySourceBinding({ template_snapshot: { summarySourceBinding: source } }),
    source,
  );
  assert.equal(
    readSummarySourceBinding({ sourceBinding: { ...source, transcriptSha256: 'not-a-hash' } }),
    null,
  );
  assert.equal(readSummarySourceBinding({}), null);
});

test('freshness reader accepts native stale and unavailable markers but rejects malformed data', () => {
  assert.deepEqual(
    readSummaryFreshness({
      summaryFreshness: { status: 'stale', reasons: ['transcript_content_changed'] },
    }),
    { status: 'stale', reasons: ['transcript_content_changed'] },
  );
  assert.deepEqual(
    readSummaryFreshness({
      summaryFreshness: { status: 'unavailable', reasons: ['source_binding_unavailable'] },
    }),
    { status: 'unavailable', reasons: ['source_binding_unavailable'] },
  );
  assert.equal(
    readSummaryFreshness({ summaryFreshness: { status: 'current', reasons: ['template_changed'] } }),
    null,
  );
  assert.equal(
    readSummaryFreshness({ summaryFreshness: { status: 'stale', reasons: ['invented_reason'] } }),
    null,
  );
});

test('native summary generation ignores WebView text and snapshots the active source', () => {
  const commands = read('src-tauri/src/summary/commands.rs');
  const repository = read('src-tauri/src/summary/source_repository.rs');
  const productionRepository = repository.split('#[cfg(test)]')[0];
  const service = read('src-tauri/src/summary/service.rs');
  const dropCallerText = commands.indexOf('drop(text);');
  const resolveActiveSource = commands.indexOf('resolve_active_summary_input(&pool, &m_id)');
  const saveTranscriptChunks = commands.indexOf('TranscriptChunksRepository::save_transcript_data');

  assert.ok(dropCallerText >= 0);
  assert.ok(resolveActiveSource > dropCallerText);
  assert.ok(saveTranscriptChunks > resolveActiveSource);
  assert.match(commands, /summary_source_binding: Some\(source_binding\.clone\(\)\)/);
  assert.match(commands, /process_transcript_background\([\s\S]+active_source,/);
  assert.match(service, /validate_summary_markdown_with_source[\s\S]+&summary_source/);
  assert.doesNotMatch(service, /validate_summary_markdown_with_transcript\(/);
  assert.match(productionRepository, /moss_activation_snapshots[\s\S]+status = 'active'/);
  assert.match(productionRepository, /moss_activation_segments[\s\S]+ORDER BY segment_index/);
  assert.doesNotMatch(productionRepository, /moss_candidate_segments/);

  const hook = read('src/hooks/meeting-details/useSummaryGeneration.ts');
  assert.match(hook, /error\.code === 'SUMMARY_SOURCE_BINDING_FAILED'/);
  assert.match(hook, /t\('errors\.sourceBindingFailed'\)/);
  assert.match(hook, /generationRequestInFlightRef\.current/);
  assert.match(hook, /\.\.\.pollingResult\.data/);
});

test('native reads attach a current/stale marker and the summary panel displays it', () => {
  const commands = read('src-tauri/src/summary/commands.rs');
  const panel = read('src/components/MeetingDetails/SummaryPanel.tsx');
  const restoreStart = commands.indexOf('pub async fn api_restore_manual_summary_revision');
  const restoreEnd = commands.indexOf(
    'pub async fn api_get_meeting_summary_language',
    restoreStart,
  );
  const restoreCommand = commands.slice(restoreStart, restoreEnd);

  assert.match(commands, /calculate_summary_freshness/);
  assert.match(commands, /evaluate_summary_freshness\(&generated_from, &current\)/);
  assert.match(commands, /resolve_template_for_generation/);
  assert.match(commands, /"summaryFreshness"/);
  assert.ok(restoreStart >= 0 && restoreEnd > restoreStart);
  assert.match(restoreCommand, /attach_summary_freshness/);
  assert.match(panel, /readSummaryFreshness\(aiSummary\)/);
  assert.match(panel, /selectedTemplateChanged/);
  assert.match(panel, /sourceFreshness\.staleTitle/);
});

test('Chinese fact-conflict copy explicitly says 需检查', () => {
  const locale = JSON.parse(read('src/i18n/locales/zh-CN/summary.json')) as {
    status: { needsReview: string };
    factValidation: {
      needsReviewTitle: string;
      untraceableActionOwner: string;
      untraceableActionTime: string;
    };
    sourceFreshness: { staleTitle: string; unavailableTitle: string };
  };

  assert.match(locale.status.needsReview, /需检查/);
  assert.match(locale.factValidation.needsReviewTitle, /需检查/);
  assert.match(locale.factValidation.untraceableActionOwner, /需检查/);
  assert.match(locale.factValidation.untraceableActionTime, /需检查/);
  assert.match(locale.sourceFreshness.staleTitle, /需检查/);
  assert.match(locale.sourceFreshness.unavailableTitle, /需检查/);
});
