import { invoke } from '@tauri-apps/api/core';
import { WhisperAPI, MODEL_CONFIGS } from './whisper';

// All catalogued Whisper models are multilingual. Never silently substitute an
// English-only or unknown model for a source language the user selected.
export function needsMultilingualSetup(provider: string, model: string): boolean {
  // SenseVoice-Small covers Chinese, English, Cantonese, Japanese and Korean,
  // so it satisfies the multilingual requirement without the Whisper catalog.
  if (provider === 'sensevoice') return false;
  return provider !== 'localWhisper' || !Object.hasOwn(MODEL_CONFIGS, model);
}

/** The language dialog only offers a Whisper preparation action for providers
 * that cannot use their own multilingual model manager. */
export function shouldShowWhisperPreparation(provider: string, model: string): boolean {
  return provider !== 'sensevoice' && needsMultilingualSetup(provider, model);
}

export type ModelPreparationStep = 'checking' | 'downloading' | 'loading' | 'saving';

export async function prepareWhisperModel(
  model: string,
  save = true,
  onStep: (step: ModelPreparationStep) => void = () => {},
): Promise<void> {
  if (needsMultilingualSetup('localWhisper', model)) throw new Error('Unsupported multilingual model');
  const assertIdle = async () => {
    const state = await invoke<{ is_recording: boolean; is_active: boolean }>('get_recording_state');
    if (state.is_recording || state.is_active) throw new Error('Stop recording before changing the transcription model');
  };
  await assertIdle();
  onStep('checking');
  await WhisperAPI.init();
  const available = await WhisperAPI.getAvailableModels();
  if (available.find(item => item.name === model)?.status !== 'Available') {
    onStep('downloading');
    await WhisperAPI.downloadModel(model);
    onStep('checking');
    const downloaded = await WhisperAPI.getAvailableModels();
    if (downloaded.find(item => item.name === model)?.status !== 'Available') {
      throw new Error('Downloaded model file did not pass validation');
    }
  }
  // Download can take minutes. Recheck before replacing the loaded model.
  await assertIdle();
  onStep('loading');
  await WhisperAPI.loadModel(model);
  if (await WhisperAPI.getCurrentModel() !== model || !await WhisperAPI.isModelLoaded()) {
    throw new Error('The selected transcription model did not finish loading');
  }
  if (save) {
    onStep('saving');
    await invoke('api_save_transcript_config', { provider: 'localWhisper', model, apiKey: null });
  }
}
