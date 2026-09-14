import { test } from 'node:test';
import assert from 'node:assert/strict';
import { judge } from './proofread-score.mjs';
const fixture = { annotation_complete: true, segments: ['年想赚到钱，影片拍完了。'], expect: [{ segment: 0, original: '年想', suggested: '你想' }] };
const parsed = (edits, extra = {}) => ({ candidates: edits.map(e => ({ segment_index: 0, ...e })), answered: [0], dropped: [], ...extra });
test('deleting 年 is a miss and an unapproved change', () => {
  const result = judge(parsed([{ original: '年想', suggested: '想' }]), fixture);
  assert.equal(result.pass, false); assert.equal(result.misses.length, 1); assert.equal(result.violations.length, 1);
});
test('identical final text accepts a longer original span', () => {
  assert.equal(judge(parsed([{ original: '年想赚到', suggested: '你想赚到' }]), fixture).pass, true);
});
test('unlisted synonyms fail even when required correction is present', () => {
  assert.equal(judge(parsed([{ original: '年想', suggested: '你想' }, { original: '影片', suggested: '视频' }]), fixture).pass, false);
});
test('clean empty/malformed/missing responses cannot pass', () => {
  const clean = { annotation_complete: true, segments: ['没有错字'], expect: [] };
  assert.equal(judge(parsed([], { answered: [] }), clean).pass, false);
  assert.equal(judge(parsed([], { dropped: ['no-json-object'] }), clean).pass, false);
  assert.equal(judge(parsed([]), clean).pass, true);
});
test('partially annotated historical recordings cannot certify a model', () => {
  assert.equal(judge(parsed([{ original: '年想', suggested: '你想' }]), { ...fixture, annotation_complete: false }).pass, false);
});
test('individually correct overlapping edits cannot pass together', () => {
  assert.equal(judge(parsed([{ original: '年想', suggested: '你想' }, { original: '年想赚到', suggested: '你想赚到' }]), fixture).pass, false);
});
