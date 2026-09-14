import assert from 'node:assert/strict';
import test from 'node:test';
import type { Transcript } from '../../src/types';
import { completedSnapshotRows, mergeLiveTranscripts } from '../../src/lib/live-transcript';

const row = (sequence_id: number, revision: number, text: string, is_partial = true): Transcript => ({
  id: `render-${sequence_id}-${revision}`, sequence_id, revision, text, is_partial,
  timestamp: '12:00:00', chunk_start_time: sequence_id * 20,
  audio_start_time: sequence_id * 20, audio_end_time: sequence_id * 20 + 20, duration: 20,
});

test('same sequence zero revises the existing row and preserves its render identity', () => {
  const original = row(0, 1, '自20世纪。');
  const result = mergeLiveTranscripts([original], [row(0, 2, '自20世纪60年代以来。', false)]);
  assert.equal(result.length, 1);
  assert.equal(result[0].text, '自20世纪60年代以来。');
  assert.equal(result[0].id, original.id);
  assert.equal(result[0].is_partial, false);
});

test('out of order revisions and duplicate delivery cannot overwrite the final text', () => {
  const result = mergeLiveTranscripts([], [row(0, 3, '完整句子。', false), row(0, 1, '旧'), row(0, 2, '旧句子')]);
  assert.equal(result.length, 1);
  assert.equal(result[0].text, '完整句子。');
  assert.deepEqual(mergeLiveTranscripts(result, [row(0, 3, '完整句子。', false)]), result);
});

test('repeated words in distinct audio rows are preserved', () => {
  const result = mergeLiveTranscripts([], [row(1, 1, '可以可以。'), row(0, 1, '可以可以。')]);
  assert.deepEqual(result.map(item => item.sequence_id), [0, 1]);
  assert.deepEqual(result.map(item => item.text), ['可以可以。', '可以可以。']);
});

test('legacy partial to final updates still work without a revision counter', () => {
  const original = { ...row(0, 0, '前半句'), revision: undefined };
  const done = { ...row(0, 0, '完整一句。', false), revision: undefined };
  const result = mergeLiveTranscripts([original], [done]);
  assert.equal(result[0].text, '完整一句。');
  assert.equal(mergeLiveTranscripts(result, [original])[0].is_partial, false);
});

test('completed snapshot retains repeated rows, final revisions and recording times for database save', () => {
  const saved = [0, 1].map(sequence_id => ({ id: `seg_${sequence_id}`, sequence_id,
    text: '可以可以。', display_time: '12:00:00', revision: 3, is_partial: false,
    audio_start_time: sequence_id * 2, audio_end_time: sequence_id * 2 + 2, duration: 2 }));
  const rows = completedSnapshotRows(saved);
  assert.deepEqual(rows.map(r => [r.text, r.audio_start_time, r.revision]), [['可以可以。', 0, 3], ['可以可以。', 2, 3]]);
  assert.throws(() => completedSnapshotRows([{ ...saved[0], is_partial: true }]));
  assert.throws(() => completedSnapshotRows([saved[0], saved[0]]));
});
