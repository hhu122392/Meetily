import assert from 'node:assert/strict';
import test from 'node:test';
import { summaryBlocksFingerprint } from '../../src/lib/summary-editor-state';

test('undo restores the saved content fingerprint despite regenerated block ids', () => {
  const saved = [{ id: 'one', type: 'paragraph', content: [{ type: 'text', text: '截止：今天18点前', styles: {} }] }];
  const edited = [{ ...saved[0], content: [{ type: 'text', text: '截止：14点30分', styles: {} }] }];
  const undone = [{ ...saved[0], id: 'replacement', children: [] }];
  assert.notEqual(summaryBlocksFingerprint(saved), summaryBlocksFingerprint(edited));
  assert.equal(summaryBlocksFingerprint(saved), summaryBlocksFingerprint(undone));
});

test('formatting, nested content and mention ids remain meaningful changes', () => {
  const saved = [{ id: 'a', type: 'paragraph', content: [{ type: 'mention', id: 'person-a' }], children: [{ id: 'b', type: 'paragraph', content: [] }] }];
  assert.equal(summaryBlocksFingerprint(saved), summaryBlocksFingerprint([{ ...saved[0], id: 'new-a', children: [{ ...saved[0].children[0], id: 'new-b' }] }]));
  assert.notEqual(summaryBlocksFingerprint(saved), summaryBlocksFingerprint([{ ...saved[0], type: 'heading' }]));
  assert.notEqual(summaryBlocksFingerprint(saved), summaryBlocksFingerprint([{ ...saved[0], content: [{ type: 'mention', id: 'person-b' }] }]));
});
