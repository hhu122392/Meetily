import { invoke } from '@tauri-apps/api/core';

export const DEFAULT_TRANSCRIPT_CONFIG = { provider: 'sensevoice' as const, model: 'sensevoice-small-int8', apiKey: null };
export const SENSEVOICE_BYTES = 239_233_841 + 315_894;
export interface SenseVoiceState {
  status: 'checking' | 'missing' | 'partial' | 'downloading' | 'available' | 'error';
  downloaded_bytes: number;
  total_bytes: number;
}
type Command = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
export async function readSenseVoiceState(call: Command = invoke): Promise<SenseVoiceState> {
  await call('sensevoice_init');
  return call<SenseVoiceState>('sensevoice_get_download_state');
}
export async function downloadSenseVoice(call: Command = invoke): Promise<SenseVoiceState> {
  await call('sensevoice_init');
  await call('sensevoice_download_model', { modelName: DEFAULT_TRANSCRIPT_CONFIG.model });
  const state = await call<SenseVoiceState>('sensevoice_get_download_state');
  if (state.status !== 'available') throw new Error('SenseVoice download incomplete');
  return state;
}
