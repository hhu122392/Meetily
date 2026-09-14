import { useState, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface RawModelInfo {
  name: string;
  /** Whisper / Parakeet 直接给 MB；SenseVoice 只给字节数（P2：避免出现 "NaN MB"） */
  size_mb?: number;
  size_bytes?: number;
  status: 'Available' | 'Missing' | { Downloading: { progress: number } } | { Error: string };
}

function resolveSizeMb(model: RawModelInfo): number {
  if (typeof model.size_mb === 'number' && Number.isFinite(model.size_mb)) {
    return model.size_mb;
  }
  if (typeof model.size_bytes === 'number' && Number.isFinite(model.size_bytes)) {
    return model.size_bytes / (1024 * 1024);
  }
  return 0;
}

export interface ModelOption {
  provider: 'whisper' | 'parakeet' | 'sensevoice';
  name: string;
  displayName: string;
  size_mb: number;
}

interface TranscriptModelConfig {
  provider?: string;
  model?: string;
}

/**
 * Custom hook for fetching and managing transcription models (Whisper and Parakeet).
 *
 * This hook centralizes the model fetching logic that was previously duplicated
 * in ImportAudioDialog and RetranscribeDialog components.
 *
 * @param transcriptModelConfig - User's saved model configuration from context
 * @returns Object containing available models, selected model key, loading state, and fetch function
 */
export function useTranscriptionModels(transcriptModelConfig: TranscriptModelConfig | undefined) {
  const [availableModels, setAvailableModels] = useState<ModelOption[]>([]);
  const [selectedModelKey, setSelectedModelKey] = useState<string>('');
  const [loadingModels, setLoadingModels] = useState(false);
  // Track whether the user has manually changed the model selection
  const userSelectedRef = useRef(false);

  // Wrap setSelectedModelKey to track user-initiated changes
  const setSelectedModelKeyWithTracking = useCallback((key: string) => {
    userSelectedRef.current = true;
    setSelectedModelKey(key);
  }, []);

  const fetchModels = useCallback(async () => {
    setLoadingModels(true);
    const allModels: ModelOption[] = [];

    // Fetch Whisper models
    try {
      const whisperModels = await invoke<RawModelInfo[]>('whisper_get_available_models');
      const availableWhisper = whisperModels
        .filter((m) => String(m.status).toLowerCase() === 'available')
        .map((m) => ({
          provider: 'whisper' as const,
          name: m.name,
          displayName: `🏠 Whisper: ${m.name}`,
          size_mb: resolveSizeMb(m),
        }));
      allModels.push(...availableWhisper);
    } catch (err) {
      console.error('Failed to fetch Whisper models:', err);
    }

    // Fetch Parakeet models
    try {
      const parakeetModels = await invoke<RawModelInfo[]>('parakeet_get_available_models');
      const availableParakeet = parakeetModels
        .filter((m) => String(m.status).toLowerCase() === 'available')
        .map((m) => ({
          provider: 'parakeet' as const,
          name: m.name,
          displayName: `⚡ Parakeet: ${m.name}`,
          size_mb: resolveSizeMb(m),
        }));
      allModels.push(...availableParakeet);
    } catch (err) {
      console.error('Failed to fetch Parakeet models:', err);
    }

    // Fetch SenseVoice models (sherpa-onnx, Chinese-first)
    try {
      const sensevoiceModels = await invoke<RawModelInfo[]>('sensevoice_get_available_models');
      const availableSenseVoice = sensevoiceModels
        .filter((m) => String(m.status).toLowerCase() === 'available')
        .map((m) => ({
          provider: 'sensevoice' as const,
          name: m.name,
          displayName: `🀄 SenseVoice: ${m.name}`,
          size_mb: resolveSizeMb(m),
        }));
      allModels.push(...availableSenseVoice);
    } catch (err) {
      console.error('Failed to fetch SenseVoice models:', err);
    }

    setAvailableModels(allModels);

    // Set default model based on user's saved configuration
    const configuredProvider = transcriptModelConfig?.provider || '';
    const configuredModel = transcriptModelConfig?.model || '';

    // Try to match the configured model
    // Note: 'localWhisper' in config maps to 'whisper' provider in model list
    const configuredMatch = allModels.find(
      (m) =>
        (configuredProvider === 'localWhisper' && m.provider === 'whisper' && m.name === configuredModel) ||
        (configuredProvider === 'parakeet' && m.provider === 'parakeet' && m.name === configuredModel) ||
        (configuredProvider === 'sensevoice' && m.provider === 'sensevoice' && m.name === configuredModel)
    );

    // Only set default model if user hasn't manually selected one
    if (!userSelectedRef.current) {
      if (configuredMatch) {
        // Use the configured model if available
        setSelectedModelKey(`${configuredMatch.provider}:${configuredMatch.name}`);
      } else if (allModels.length > 0) {
        // Fall back to first available model
        setSelectedModelKey(`${allModels[0].provider}:${allModels[0].name}`);
      }
    }

    setLoadingModels(false);
  }, [transcriptModelConfig]);

  // Reset user selection tracking (call when dialog opens fresh)
  const resetSelection = useCallback(() => {
    userSelectedRef.current = false;
  }, []);

  return {
    availableModels,
    selectedModelKey,
    setSelectedModelKey: setSelectedModelKeyWithTracking,
    loadingModels,
    fetchModels,
    resetSelection,
  };
}
