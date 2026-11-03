import { invoke } from '@tauri-apps/api/core';
import type { ParakeetModelInfo, CancelDownloadOutcome } from './parakeet';
import { getSenseVoiceDownloadSource, type SenseVoiceDownloadSource } from './sensevoice-download-source';
export type { ParakeetDownloadProgressEvent as SenseVoiceDownloadProgressEvent } from './parakeet';
export class SenseVoiceAPI {
  static async init(): Promise<void> {
    await invoke('sensevoice_init');
  }

  static async getAvailableModels(): Promise<ParakeetModelInfo[]> {
    return await invoke('sensevoice_get_available_models');
  }

  static async loadModel(modelName: string): Promise<void> {
    await invoke('sensevoice_load_model', { modelName });
  }

  static async getCurrentModel(): Promise<string | null> {
    return await invoke('sensevoice_get_current_model');
  }

  static async isModelLoaded(): Promise<boolean> {
    return await invoke('sensevoice_is_model_loaded');
  }

  static async transcribeAudio(audioData: number[]): Promise<string> {
    return await invoke('sensevoice_transcribe_audio', { audioData });
  }

  static async getModelsDirectory(): Promise<string> {
    return await invoke('sensevoice_get_models_directory');
  }

  static async downloadModel(modelName: string, source: SenseVoiceDownloadSource = getSenseVoiceDownloadSource()): Promise<void> {
    await invoke('sensevoice_download_model', { modelName, source });
  }

  static async cancelDownload(modelName: string): Promise<CancelDownloadOutcome> {
    return await invoke('sensevoice_cancel_download', { modelName });
  }

  static async deleteCorruptedModel(modelName: string): Promise<string> {
    return await invoke('sensevoice_delete_corrupted_model', { modelName });
  }

  static async hasAvailableModels(): Promise<boolean> {
    return await invoke('sensevoice_has_available_models');
  }

  static async validateModelReady(): Promise<string> {
    return await invoke('sensevoice_validate_model_ready');
  }

  static async openModelsFolder(): Promise<void> {
    await invoke('open_sensevoice_models_folder');
  }
}
