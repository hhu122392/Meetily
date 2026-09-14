import assert from 'node:assert/strict';
import test from 'node:test';
import { DEFAULT_BETA_FEATURES } from '../../src/types/betaFeatures';
import { formatMossTimestamp, isMossRunActive, mossRunFailureI18nKey, MossOperationGate, shortMossHash } from '../../src/features/moss/utils';
import { mossWorkspaceFixture } from './fixtures';

test('MOSS internal feature is off by default while existing retranscription remains unchanged', () => {
  assert.equal(DEFAULT_BETA_FEATURES.moss_post_meeting_enhancement, false);
  assert.equal(DEFAULT_BETA_FEATURES.importAndRetranscribe, true);
});

test('same-tick duplicate operations are coalesced before React can rerender', async () => {
  const gate = new MossOperationGate();
  let release!: (value: string) => void;
  const deferred = new Promise<string>((resolve) => { release = resolve; });
  let calls = 0;
  const operation = () => {
    calls += 1;
    return deferred;
  };

  const first = gate.run('start-run', operation);
  const second = gate.run('start-run', operation);
  assert.equal(calls, 1);
  assert.equal(await second, null);
  assert.equal(gate.has('start-run'), true);
  release('done');
  assert.equal(await first, 'done');
  assert.equal(gate.has('start-run'), false);
});

test('only non-terminal run states keep polling and block a second run', () => {
  const run = mossWorkspaceFixture().runs[0];
  assert.equal(isMossRunActive({ ...run, state: 'preparing' }), true);
  assert.equal(isMossRunActive({ ...run, state: 'running' }), true);
  assert.equal(isMossRunActive({ ...run, state: 'cancel_requested' }), true);
  assert.equal(isMossRunActive({ ...run, state: 'cancelled' }), false);
  assert.equal(isMossRunActive({ ...run, state: 'failed' }), false);
  assert.equal(isMossRunActive({ ...run, state: 'completed' }), false);
});

test('only the supported-duration failure gets specific safe copy', () => {
  assert.equal(mossRunFailureI18nKey('MOSS_AUDIO_TOO_LONG'), 'errors.audioTooLong');
  assert.equal(mossRunFailureI18nKey('D:\\private\\meeting.wav'), 'errors.operationFailed');
  assert.equal(mossRunFailureI18nKey(null), 'errors.operationFailed');
});

test('candidate fixtures remain anonymous until an explicit binding or segment override exists', () => {
  const review = mossWorkspaceFixture().review!;
  assert.deepEqual(review.bindings, [
    { speakerLabel: 'S01', personId: null },
    { speakerLabel: 'S02', personId: null },
  ]);
  assert.equal(review.candidate.segments[0].speakerLabel, 'S01');
  assert.equal(review.candidate.segments[0].resolvedPersonId, null);
  assert.equal(review.candidate.segments[0].speakerResolution, 'anonymous');
  assert.equal(review.candidate.segments[1].speakerResolution, 'anonymous');
  assert.equal(review.candidate.segments[1].speakerLabel, 'S02');
  assert.equal(review.candidate.segments[2].speakerResolution, 'segment_override');
  assert.equal(review.candidate.alignments.length, review.candidate.segments.length);
  assert.equal(review.candidate.alignments[0].alignmentMethod, 'moss_segment');
  assert.equal(review.candidate.alignments[1].alignmentMethod, 'source_transcript_segment');
  assert.deepEqual(review.candidate.alignments[1].sourceAnchorIds, ['current-1']);
  assert.equal(review.candidate.diagnostics?.tailDeltaMs, 50);
});

test('traceable correction keeps before, after, alias, canonical term, rule and state', () => {
  const correction = mossWorkspaceFixture().review!.corrections[0];
  assert.deepEqual(correction, {
    correctionId: 'correction-1',
    segmentId: 'candidate-1',
    originalText: '西吉艾斯进度正常',
    correctedText: 'CGS 进度正常',
    matchedAlias: '西吉艾斯',
    canonical: 'CGS',
    ruleId: 'term-cgs-alias-1',
    contextRevision: 2,
    state: 'applied',
    sourceLayer: 'context_alias',
    machineSource: null,
  });
});

test('hash and timestamp helpers are deterministic and do not reveal full hashes', () => {
  assert.equal(shortMossHash('a'.repeat(64)), 'aaaaaaaa…aaaaaaaa');
  assert.equal(formatMossTimestamp(0), '0:00');
  assert.equal(formatMossTimestamp(3_723_000), '1:02:03');
});
