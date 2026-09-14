import assert from 'node:assert/strict';
import test from 'node:test';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { needsMultilingualSetup, prepareWhisperModel, shouldShowWhisperPreparation } from '../../src/lib/transcription-setup';
import { chooseDefaultModelKey, type ModelOption } from '../../src/hooks/useTranscriptionModels';

test('Chinese-capable automatic mode excludes Parakeet, English-only and unknown models', () => {
  assert.equal(needsMultilingualSetup('parakeet', 'parakeet-tdt-0.6b-v3-int8'), true);
  assert.equal(needsMultilingualSetup('localWhisper', 'small.en'), true);
  assert.equal(needsMultilingualSetup('localWhisper', 'unknown'), true);
  assert.equal(needsMultilingualSetup('localWhisper', 'small'), false);
  assert.equal(needsMultilingualSetup('sensevoice', 'sensevoice-small-int8'), false);
});

test('SenseVoice uses its own model manager instead of the Whisper preparation panel', () => {
  assert.equal(shouldShowWhisperPreparation('sensevoice', 'sensevoice-small-int8'), false);
  assert.equal(shouldShowWhisperPreparation('parakeet', 'parakeet-tdt-0.6b-v3-int8'), true);
  assert.equal(shouldShowWhisperPreparation('localWhisper', 'unknown'), true);
});

test('a missing configured SenseVoice model is not silently replaced by another provider', () => {
  const available: ModelOption[] = [{ provider: 'whisper', name: 'small', displayName: 'Whisper small', size_mb: 1 }];
  assert.equal(chooseDefaultModelKey(available, { provider: 'sensevoice', model: 'sensevoice-small-int8' }), '');
  assert.equal(chooseDefaultModelKey(available, { provider: 'localWhisper', model: 'small' }), 'whisper:small');
});

for (const failure of [null, 'download', 'corrupt-download', 'load', 'wrong-model', 'not-loaded', 'save', 'recording-after-download'] as const) {
  test(`preparation acknowledges success only after every step: ${failure ?? 'success'}`, async t => {
    const oldWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
    Object.defineProperty(globalThis, 'window', { configurable: true, value: {} });
    const calls: string[] = [];
    const unexpected: string[] = [];
    let stateReads = 0;
    let downloaded = false;
    mockIPC((command, payload) => {
      calls.push(command);
      switch (command) {
        case 'get_recording_state': return { is_recording: ++stateReads > 1 && failure === 'recording-after-download', is_active: false };
        case 'whisper_init': return;
        case 'whisper_get_available_models': return [{ name: 'small', status: downloaded && failure !== 'corrupt-download' ? 'Available' : 'Missing' }];
        case 'whisper_download_model':
          assert.deepEqual(payload, { modelName: 'small' });
          if (failure === 'download') throw new Error('download');
          downloaded = true;
          return;
        case 'whisper_load_model':
          assert.deepEqual(payload, { modelName: 'small' });
          if (failure === 'load') throw new Error('load');
          return;
        case 'whisper_get_current_model': return failure === 'wrong-model' ? 'base' : 'small';
        case 'whisper_is_model_loaded': return failure !== 'not-loaded';
        case 'api_save_transcript_config':
          assert.deepEqual(payload, { provider: 'localWhisper', model: 'small', apiKey: null });
          if (failure === 'save') throw new Error('save');
          return;
        default: unexpected.push(command); throw new Error(`Unexpected IPC ${command}`);
      }
    });
    t.after(() => {
      clearMocks();
      if (oldWindow) Object.defineProperty(globalThis, 'window', oldWindow);
      else Reflect.deleteProperty(globalThis, 'window');
      assert.deepEqual(unexpected, [], 'unexpected IPC must not masquerade as an expected failure');
    });
    const stages: string[] = [];
    const result = prepareWhisperModel('small', true, stage => stages.push(stage));
    if (failure) {
      await assert.rejects(result);
      if (failure !== 'save') assert.ok(!calls.includes('api_save_transcript_config'));
      if (failure === 'recording-after-download') assert.ok(!calls.includes('whisper_load_model'));
    } else {
      await result;
      assert.deepEqual(stages, ['checking', 'downloading', 'checking', 'loading', 'saving']);
      assert.deepEqual(calls, ['get_recording_state', 'whisper_init', 'whisper_get_available_models',
        'whisper_download_model', 'whisper_get_available_models', 'get_recording_state', 'whisper_load_model',
        'whisper_get_current_model', 'whisper_is_model_loaded', 'api_save_transcript_config']);
    }
  });
}
