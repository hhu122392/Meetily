import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';

const require = createRequire(import.meta.url);
const requireFromEditor = createRequire(require.resolve('@tiptap/react'));
const { mergeAttributes } = requireFromEditor('@tiptap/core');

test('editor attributes reject JSON prototype keys and inherited handlers', () => {
  const input = JSON.parse('{"__proto__":{"onerror":"untrusted","src":"invalid:"},"title":"safe"}');
  const attributes = mergeAttributes({ class: 'existing' }, input);
  assert.equal(Object.getPrototypeOf(attributes), Object.prototype);
  assert.equal(attributes.onerror, undefined);
  assert.equal(attributes.src, undefined);
  assert.deepEqual(Object.keys(attributes).sort(), ['class', 'title']);
});

test('editor attributes preserve ordinary class and style merging', () => {
  const attributes = mergeAttributes(
    { class: 'one two', style: 'color: red; font-size: 12px', title: 'old' },
    { class: 'two three', style: 'color: blue', title: 'new' },
  );
  assert.equal(attributes.class, 'one two three');
  assert.match(attributes.style, /color: blue/);
  assert.match(attributes.style, /font-size: 12px/);
  assert.equal(attributes.title, 'new');
});
