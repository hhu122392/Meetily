import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DEFAULT_TRANSCRIPT_CONFIG, downloadSenseVoice, readSenseVoiceState } from './sensevoice';

test('new transcription config is SenseVoice', () => {
  assert.deepEqual(DEFAULT_TRANSCRIPT_CONFIG, { provider: 'sensevoice', model: 'sensevoice-small-int8', apiKey: null });
});
test('missing model downloads only SenseVoice and verifies completion', async () => {
  const calls: string[] = [];
  const call = async <T,>(command: string) => {
    calls.push(command);
    return (command === 'sensevoice_get_download_state' ? { status: 'available' } : undefined) as T;
  };
  assert.equal((await downloadSenseVoice(call)).status, 'available');
  assert.deepEqual(calls, ['sensevoice_init', 'sensevoice_download_model', 'sensevoice_get_download_state']);
});
test('failed or incomplete download cannot report ready', async () => {
  await assert.rejects(downloadSenseVoice(async <T,>(command: string) => {
    if (command === 'sensevoice_download_model') throw new Error('offline');
    return undefined as T;
  }), /offline/);
  await assert.rejects(downloadSenseVoice(async <T,>(command: string) =>
    (command === 'sensevoice_get_download_state' ? { status: 'partial' } : undefined) as T), /incomplete/);
});
test('reopening a page reads the active download instead of starting another', async () => {
  const calls: string[] = [];
  const state = await readSenseVoiceState(async <T,>(command: string) => {
    calls.push(command);
    return { status: 'downloading', downloaded_bytes: 100, total_bytes: 200 } as T;
  });
  assert.equal(state.status, 'downloading');
  assert.deepEqual(calls, ['sensevoice_init', 'sensevoice_get_download_state']);
});
