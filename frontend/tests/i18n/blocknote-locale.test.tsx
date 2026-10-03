import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { createInstance } from 'i18next';
import { I18nextProvider } from 'react-i18next';
import { BlockNoteSchema, getDefaultSlashMenuItems } from '@blocknote/core';
import { en, zh } from '@blocknote/core/locales';
import { BlockNoteLocale, useBlockNoteLocale } from '../../src/lib/blocknote-locale';

test('actual BlockNote menu factory and placeholders follow live locale without replacing dictionary', async () => {
  const i18n = createInstance();
  await i18n.init({ lng: 'zh-CN', fallbackLng: 'en', resources: { 'zh-CN': { translation: {} }, en: { translation: {} } } });
  const originals = JSON.stringify({ en, zh });
  let current!: ReturnType<typeof useBlockNoteLocale>;
  let mounts = 0;
  function Probe() {
    current = useBlockNoteLocale();
    React.useEffect(() => { mounts++; }, []);
    return <BlockNoteLocale {...current}><p>未保存正文</p></BlockNoteLocale>;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => { renderer = TestRenderer.create(<I18nextProvider i18n={i18n}><Probe /></I18nextProvider>); });
  const dictionary = current.dictionary;
  // Run the installed library's real menu builder; editor commands are never invoked.
  const editor = { dictionary, schema: BlockNoteSchema.create(), settings: { heading: { levels: [1, 2, 3] } } } as Parameters<typeof getDefaultSlashMenuItems>[0];
  const titles = () => getDefaultSlashMenuItems(editor).map(item => item.title);
  const css = () => renderer.root.findByType('style').children.join('');
  assert.ok(titles().includes(zh.slash_menu.heading.title));
  assert.ok(css().includes(JSON.stringify(zh.placeholders.default)));
  assert.equal(dictionary.drag_handle.delete_menuitem, zh.drag_handle.delete_menuitem);
  await act(async () => { await i18n.changeLanguage('en'); });
  assert.equal(current.dictionary, dictionary);
  assert.ok(titles().includes(en.slash_menu.heading.title));
  assert.ok(css().includes(JSON.stringify(en.placeholders.default)));
  assert.equal(dictionary.drag_handle.delete_menuitem, en.drag_handle.delete_menuitem);
  assert.equal(renderer.root.findByType('p').children.join(''), '未保存正文');
  await act(async () => { await i18n.changeLanguage('zh-CN'); });
  assert.equal(current.dictionary, dictionary);
  assert.ok(titles().includes(zh.slash_menu.heading.title));
  assert.equal(mounts, 1);
  assert.equal(JSON.stringify({ en, zh }), originals, 'shared built-in dictionaries must not be mutated');
  act(() => renderer.unmount());
});
