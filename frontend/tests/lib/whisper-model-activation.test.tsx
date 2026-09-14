import assert from 'node:assert/strict';
import test from 'node:test';
import { webcrypto } from 'node:crypto';
import { setImmediate } from 'node:timers/promises';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import '@tauri-apps/api/event';
import { ModelManager } from '../../src/components/WhisperModelManager';
import { i18n } from '../../src/i18n';

test('model selection loads the exact model and confirms it before saving or notifying the UI', async t => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'window');
  const oldStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const storage = { getItem: () => null, setItem: () => {} };
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { localStorage: storage, crypto: webcrypto } });
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: storage });
  const steps: string[] = [];
  const unexpected: string[] = [];
  let loaded = false;
  let renderer: TestRenderer.ReactTestRenderer | undefined;
  mockIPC(command => {
    switch (command) {
      case 'whisper_init': return;
      case 'whisper_get_available_models': return [{ name: 'small', status: 'Available', accuracy: 'Good', speed: 'Medium', size_mb: 466 }];
      case 'get_recording_state': return { is_recording: false, is_active: false };
      case 'whisper_load_model': steps.push('load'); loaded = true; return;
      case 'whisper_get_current_model': steps.push('confirm-model'); return loaded ? 'small' : null;
      case 'whisper_is_model_loaded': steps.push('confirm-loaded'); return loaded;
      case 'api_save_transcript_config': steps.push('save'); return;
      default: unexpected.push(command); throw new Error(command);
    }
  }, { shouldMockEvents: true });
  t.after(async () => {
    try { await act(async () => { renderer?.unmount(); await setImmediate(); }); }
    finally {
      clearMocks();
      for (const [key, descriptor] of [['window', original], ['localStorage', oldStorage]] as const) {
        if (descriptor) Object.defineProperty(globalThis, key, descriptor);
        else Reflect.deleteProperty(globalThis, key);
      }
    }
    assert.deepEqual(unexpected, []);
  });
  await i18n.changeLanguage('zh-CN');
  await act(async () => {
    renderer = TestRenderer.create(<ModelManager autoSave onModelSelect={() => steps.push('notify')} />);
    await setImmediate();
  });
  const card = renderer!.root.find(node => node.props.model?.name === 'small' && typeof node.props.onSelect === 'function');
  await act(async () => { card.props.onSelect(); await setImmediate(); });
  assert.deepEqual(steps, ['load', 'confirm-model', 'confirm-loaded', 'save', 'notify']);
});
