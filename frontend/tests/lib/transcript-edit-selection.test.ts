import assert from 'node:assert/strict';
import test from 'node:test';
import { hasConflictingTranscriptEdits, transcriptCandidateKey } from '../../src/lib/transcript-edit-selection';
import type { ProofreadCandidate } from '../../src/lib/transcript-revision';

const candidate = (segmentId: string, start: number, end: number): ProofreadCandidate => ({
  segment_id: segmentId, segment_index: 0, audio_start_time: 0,
  original: '店', suggested: '商店', start_char: start, end_char: end,
  reason: 'term', confidence: 'high', segment_text: '店和店', proposed_text: '店和商店',
});

test('repeated words have separate selection identities', () => {
  assert.notEqual(transcriptCandidateKey(candidate('a', 0, 1)), transcriptCandidateKey(candidate('a', 2, 3)));
});
test('overlap conflicts only within the same segment', () => {
  assert.equal(hasConflictingTranscriptEdits([candidate('a', 0, 3), candidate('a', 2, 3)]), true);
  assert.equal(hasConflictingTranscriptEdits([candidate('a', 0, 3), candidate('b', 2, 3)]), false);
  assert.equal(hasConflictingTranscriptEdits([candidate('a', 0, 1), candidate('a', 1, 2)]), false);
});
test('checking conflicts preserves the displayed candidate order', () => {
  const entries = [candidate('a', 2, 3), candidate('a', 0, 1)];
  hasConflictingTranscriptEdits(entries);
  assert.deepEqual(entries.map(item => item.start_char), [2, 0]);
});
