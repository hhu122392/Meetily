import { afterAll, describe, expect, test } from 'bun:test';
import { createRequire } from 'node:module';
import { JSDOM } from 'jsdom';

const require = createRequire(import.meta.url);
const pmRequire = createRequire(require.resolve('@tiptap/pm/model'));
const { Schema, DOMSerializer } = pmRequire('prosemirror-model');
const { tableNodes } = pmRequire('prosemirror-tables');
const dom = new JSDOM('<!doctype html><html><body></body></html>');
const globals = ['window', 'document', 'navigator', 'CSSStyleRule'];
const descriptors = new Map(globals.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
for (const key of globals) Object.defineProperty(globalThis, key, { configurable: true, value: dom.window[key] });
const { __parseFromClipboard } = pmRequire('prosemirror-view');

afterAll(() => {
  dom.window.close();
  for (const key of globals) {
    const descriptor = descriptors.get(key);
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else delete globalThis[key];
  }
});

const schema = new Schema({
  nodes: {
    doc: { content: 'block+' },
    text: { group: 'inline' },
    paragraph: { group: 'block', content: 'inline*', parseDOM: [{ tag: 'p' }], toDOM: () => ['p', 0] },
    quote: {
      group: 'block', content: 'block+',
      attrs: { variant: { default: 'normal', validate(value) {
        if (value !== 'normal') throw new RangeError('Invalid clipboard variant');
      } } },
      parseDOM: [{ tag: 'blockquote' }], toDOM: node => ['blockquote', { 'data-variant': node.attrs.variant }, 0],
    },
    ...tableNodes({ tableGroup: 'block', cellContent: 'paragraph+' }),
  },
  marks: { strong: { parseDOM: [{ tag: 'strong' }], toDOM: () => ['strong', 0] } },
});

function parse(html, text = '', plain = false) {
  const doc = schema.node('doc', null, [schema.node('paragraph')]);
  return __parseFromClipboard({ state: { schema }, someProp: () => undefined }, text, html, plain, doc.resolve(1));
}

function contextualHTML(attributes) {
  const p = document.createElement('p');
  p.textContent = '会议结论';
  p.setAttribute('data-pm-slice', `0 0 ${JSON.stringify(['quote', attributes])}`);
  return p.outerHTML;
}

describe('editor clipboard security and compatibility', () => {
  test('rejects unvalidated attributes supplied by clipboard slice context', () => {
    const slice = parse(contextualHTML({ variant: ['span', { 'data-audit': 'unexpected' }] }));
    expect(slice.content.firstChild.type.name).toBe('paragraph');
    expect(slice.content.textBetween(0, slice.content.size)).toBe('会议结论');
    const wrapper = document.createElement('div');
    wrapper.append(DOMSerializer.fromSchema(schema).serializeFragment(slice.content));
    expect(wrapper.querySelector('blockquote')).toBeNull();
    expect(wrapper.querySelector('[data-audit]')).toBeNull();
  });

  test('preserves valid clipboard context', () => {
    const slice = parse(contextualHTML({ variant: 'normal' }));
    expect(slice.content.firstChild.type.name).toBe('quote');
    expect(slice.content.firstChild.attrs.variant).toBe('normal');
    expect(slice.content.textBetween(0, slice.content.size)).toBe('会议结论');
  });

  test('preserves Chinese text, inline formatting and table cells', () => {
    const slice = parse('<p><strong>会议纪要</strong></p><table><tbody><tr><td><p>任务</p></td><td><p>小明</p></td></tr></tbody></table>');
    const nodes = [];
    slice.content.descendants(node => nodes.push(node));
    expect(nodes.some(node => node.marks.some(mark => mark.type.name === 'strong'))).toBe(true);
    expect(nodes.filter(node => node.type.name === 'table_cell')).toHaveLength(2);
    expect(slice.content.textBetween(0, slice.content.size)).toContain('小明');
    expect(schema.nodeFromJSON(schema.node('doc', null, slice.content).toJSON()).eq(schema.node('doc', null, slice.content))).toBe(true);
  });

  test('plain-text paste does not restore hostile HTML context', () => {
    const slice = parse(contextualHTML({ variant: 'invalid' }), '纯文本纪要', true);
    expect(slice.content.firstChild.type.name).toBe('paragraph');
    expect(slice.content.textBetween(0, slice.content.size)).toBe('纯文本纪要');
  });
});
