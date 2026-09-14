import assert from 'node:assert/strict';
import test from 'node:test';
import { createHash } from 'node:crypto';
import { resolveSummaryEvidence } from '../../src/lib/summary-evidence';
import type { SummaryFieldTrace } from '../../src/types/summary-source';

const text = '林岚：我今天18点前发纪要。现在14点30分，会议结束。';
const hash = createHash('sha256').update(text).digest('hex');
const reference = { segmentId: 'segment-12', startMs: null, endMs: null, excerptSha256: hash };
const trace: SummaryFieldTrace = {
  field: 'time', task: '发纪要', value: '14点30分',
  markdownLine: 4, markdownColumn: null, status: 'needs_review',
  evidence: [], relatedEvidence: [reference],
};

test('an unsupported time shows task context without presenting it as proof', async () => {
  const [result] = await resolveSummaryEvidence([trace], [{ id: 'segment-12', text }]);
  assert.ok(result, 'each checked field must have a review record');
  assert.equal(result.trace.status, 'needs_review');
  assert.equal(result.sources[0].related, true);
  assert.equal(result.sources[0].text, text);
  assert.equal(result.sources[0].startMs, null, 'text fixture must not acquire a fake audio time');
});

test('changed or absent source text is never displayed under an old evidence reference', async () => {
  for (const transcripts of [[], [{ id: 'segment-12', text: '这是一段修改后的文稿。' }]]) {
    const [result] = await resolveSummaryEvidence([trace], transcripts);
    assert.ok(result, 'keep the field even when its evidence is unavailable');
    assert.deepEqual(result.sources, []);
  }
});

test('supported evidence preserves the segment identity and actual recording offset', async () => {
  const supported: SummaryFieldTrace = {
    ...trace, value: '今天18点前', status: 'supported',
    evidence: [{ ...reference, startMs: 330000 }], relatedEvidence: [],
  };
  const [result] = await resolveSummaryEvidence([supported], [{ id: 'segment-12', text }]);
  assert.ok(result, 'supported field must retain its source record');
  assert.equal(result.sources[0].segmentId, 'segment-12');
  assert.equal(result.sources[0].startMs, 330000);
  assert.equal(result.sources[0].related, false);
});
