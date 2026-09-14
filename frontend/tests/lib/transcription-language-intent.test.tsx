import assert from 'node:assert/strict';
import test, { type TestContext } from 'node:test';
import { webcrypto } from 'node:crypto';
import { setImmediate } from 'node:timers/promises';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import '@tauri-apps/api/event';
import { ConfigProvider, useConfig } from '../../src/contexts/ConfigContext';
import { LanguageSelection } from '../../src/components/LanguageSelection';
import { i18n } from '../../src/i18n';

async function mountConfig(t: TestContext, savedLanguage: string, provider = 'parakeet') {
  const values = new Map([['primaryLanguage', savedLanguage]]);
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    removeItem: (key: string) => { values.delete(key); },
  };
  const originals = ['window', 'localStorage'].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const);
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { localStorage: storage, crypto: webcrypto } });
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage });
  const languages: string[] = [];
  const unexpected: string[] = [];
  let rejectLanguage = false;
  mockIPC((command, args) => {
    switch (command) {
      case 'api_get_transcript_config': return { provider, model: provider === 'parakeet' ? 'parakeet-tdt-0.6b-v3-int8' : 'small', apiKey: null };
      case 'get_ollama_models': return [];
      case 'api_get_model_config':
      case 'api_get_api_key':
      case 'get_recording_preferences': return null;
      case 'set_language_preference':
        languages.push((args as { language: string }).language);
        if (rejectLanguage) throw new Error('Language preference could not be saved');
        return;
      default:
        unexpected.push(command);
        throw new Error(`Unexpected IPC in test: ${command}`);
    }
  }, { shouldMockEvents: true });
  let config!: ReturnType<typeof useConfig>;
  function Probe() { config = useConfig(); return null; }
  let renderer: TestRenderer.ReactTestRenderer | undefined;
  const mount = async () => {
    await act(async () => {
      renderer = TestRenderer.create(<ConfigProvider><Probe /></ConfigProvider>);
      await setImmediate();
    });
  };
  const unmount = async () => {
    await act(async () => { renderer?.unmount(); await setImmediate(); });
    renderer = undefined;
  };
  t.after(async () => {
    try { await unmount(); }
    finally {
      clearMocks();
      for (const [key, descriptor] of originals) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else Reflect.deleteProperty(globalThis, key);
      }
    }
    assert.deepEqual(unexpected, [], 'test must not hide unhandled IPC calls');
  });
  await mount();
  return { current: () => config, storage, languages, mount, unmount, failWrites: () => { rejectLanguage = true; } };
}

test('saved Chinese intent survives Parakeet startup without being replaced by auto', async t => {
  const harness = await mountConfig(t, 'zh');
  assert.equal(harness.current().selectedLanguage, 'zh');
  assert.equal(harness.storage.getItem('primaryLanguage'), 'zh');
  assert.equal(harness.languages.at(-1), 'zh');
});

test('explicit Chinese selection survives provider changes and remount', async t => {
  const harness = await mountConfig(t, 'auto');
  await act(async () => { await harness.current().setSelectedLanguage('zh'); });
  assert.equal(harness.current().selectedLanguage, 'zh');
  assert.equal(harness.languages.at(-1), 'zh');
  await act(async () => {
    harness.current().setTranscriptModelConfig({ provider: 'localWhisper', model: 'small', apiKey: null });
    await setImmediate();
  });
  assert.equal(harness.current().selectedLanguage, 'zh');
  await harness.unmount();
  await harness.mount();
  assert.equal(harness.current().selectedLanguage, 'zh');
  assert.equal(harness.storage.getItem('primaryLanguage'), 'zh');
});

test('Whisper preserves English and a failed save cannot change the stored language', async t => {
  const harness = await mountConfig(t, 'en', 'localWhisper');
  assert.equal(harness.current().selectedLanguage, 'en');
  assert.equal(harness.languages.at(-1), 'en');
  harness.failWrites();
  await act(async () => {
    await assert.rejects(harness.current().setSelectedLanguage('zh'), /could not be saved/);
  });
  assert.equal(harness.current().selectedLanguage, 'en');
  assert.equal(harness.storage.getItem('primaryLanguage'), 'en');
});

test('Parakeet settings show the chosen Chinese source and allow language changes', async t => {
  await i18n.changeLanguage('zh-CN');
  const changes: string[] = [];
  let renderer!: TestRenderer.ReactTestRenderer;
  t.after(async () => { await act(async () => renderer?.unmount()); });
  await act(async () => {
    renderer = TestRenderer.create(<LanguageSelection selectedLanguage="zh" provider="parakeet" onLanguageChange={async language => { changes.push(language); }} />);
  });
  const select = renderer.root.findByType('select');
  assert.equal(select.props.value, 'zh');
  assert.ok(select.findAllByType('option').some(option => option.props.value === 'zh'));
  await act(async () => { await select.props.onChange({ target: { value: 'en' } }); });
  assert.deepEqual(changes, ['en']);
});

test('language selection stays disabled until its actual save promise settles', async t => {
  let finish!: () => void;
  const pending = new Promise<void>(resolve => { finish = resolve; });
  let renderer!: TestRenderer.ReactTestRenderer;
  t.after(async () => { finish(); await act(async () => { await pending; renderer?.unmount(); }); });
  await act(async () => {
    renderer = TestRenderer.create(<LanguageSelection selectedLanguage="zh" onLanguageChange={() => pending} />);
  });
  let operation!: Promise<void>;
  act(() => { operation = renderer.root.findByType('select').props.onChange({ target: { value: 'en' } }); });
  assert.equal(renderer.root.findByType('select').props.disabled, true);
  await act(async () => { finish(); await operation; });
  assert.equal(renderer.root.findByType('select').props.disabled, false);
});
