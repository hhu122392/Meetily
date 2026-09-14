import React, { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { motion, AnimatePresence } from 'framer-motion';
import { toast } from 'sonner';
import {
  ModelInfo,
  ModelStatus,
  getModelIcon,
  formatFileSize,
  getModelPerformanceBadge,
  isQuantizedModel,
  getModelTagline,
  WhisperAPI
} from '../lib/whisper';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { useTranslation } from 'react-i18next';
import { prepareWhisperModel } from '@/lib/transcription-setup';

interface ModelManagerProps {
  selectedModel?: string;
  onModelSelect?: (modelName: string) => void;
  className?: string;
  autoSave?: boolean;
}

export function ModelManager({
  selectedModel,
  onModelSelect,
  className = '',
  autoSave = false
}: ModelManagerProps) {
  const { t } = useTranslation(['models', 'transcription']);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [initialized, setInitialized] = useState(false);
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());
  const selectingRef = useRef(false);
  const [selecting, setSelecting] = useState(false);

  // Progress throttle map to prevent rapid updates
  const progressThrottleRef = useRef<Map<string, { progress: number; timestamp: number }>>(new Map());

  // Load persisted downloading state from localStorage
  const getPersistedDownloadingModels = (): Set<string> => {
    try {
      const saved = localStorage.getItem('downloading-models');
      return saved ? new Set<string>(JSON.parse(saved) as string[]) : new Set<string>();
    } catch {
      return new Set<string>();
    }
  };

  // Persist downloading state to localStorage
  const updateDownloadingModels = (updater: (prev: Set<string>) => Set<string>) => {
    setDownloadingModels(prev => {
      const newSet = updater(prev);
      localStorage.setItem('downloading-models', JSON.stringify(Array.from(newSet)));
      return newSet;
    });
  };

  // Initialize models
  useEffect(() => {
    if (initialized) return;

    const initializeModels = async () => {
      try {
        setLoading(true);
        await WhisperAPI.init();
        const modelList = await WhisperAPI.getAvailableModels();

        // Apply persisted downloading states
        const persistedDownloading = getPersistedDownloadingModels();
        const modelsWithDownloadState = modelList.map(model => {
          if (persistedDownloading.has(model.name) && model.status !== 'Available') {
            if (typeof model.status === 'object' && 'Corrupted' in model.status) {
              updateDownloadingModels(prev => {
                const newSet = new Set(prev);
                newSet.delete(model.name);
                return newSet;
              });
              return model;
            } else if (model.status === 'Missing') {
              updateDownloadingModels(prev => {
                const newSet = new Set(prev);
                newSet.delete(model.name);
                return newSet;
              });
              return model;
            } else {
              return { ...model, status: { Downloading: 0 } as ModelStatus };
            }
          }
          return model;
        });

        setModels(modelsWithDownloadState);
        setInitialized(true);
      } catch (err) {
        console.error('Failed to initialize Whisper:', err);
        setError(true);
        toast.error(t('errors.loadTranscriptionModels'), {
          description: t('errors.loadModels'),
          duration: 5000
        });
      } finally {
        setLoading(false);
      }
    };

    initializeModels();
  }, [initialized, selectedModel, onModelSelect]);

  // Set up event listeners for download progress
  useEffect(() => {
    let unlistenProgress: (() => void) | null = null;
    let unlistenComplete: (() => void) | null = null;
    let unlistenError: (() => void) | null = null;

    const setupListeners = async () => {
      console.log('[ModelManager] Setting up event listeners...');

      // Download progress with throttling
      unlistenProgress = await listen<{ modelName: string; progress: number }>(
        'model-download-progress',
        (event) => {
          const { modelName, progress } = event.payload;
          const now = Date.now();
          const throttleData = progressThrottleRef.current.get(modelName);

          // Throttle: only update if 300ms passed OR progress jumped by 5%+
          const shouldUpdate = !throttleData ||
            now - throttleData.timestamp > 300 ||
            Math.abs(progress - throttleData.progress) >= 5;

          if (shouldUpdate) {
            console.log(`[ModelManager] Progress update for ${modelName}: ${progress}%`);
            progressThrottleRef.current.set(modelName, { progress, timestamp: now });

            setModels(prevModels =>
              prevModels.map(model =>
                model.name === modelName
                  ? { ...model, status: { Downloading: progress } as ModelStatus }
                  : model
              )
            );
          }
        }
      );

      // Download complete
      unlistenComplete = await listen<{ modelName: string }>(
        'model-download-complete',
        (event) => {
          const { modelName } = event.payload;
          const model = models.find(m => m.name === modelName);
          const displayName = getDisplayName(modelName);

          setModels(prevModels =>
            prevModels.map(model =>
              model.name === modelName
                ? { ...model, status: 'Available' as ModelStatus }
                : model
            )
          );

          setDownloadingModels(prev => {
            const newSet = new Set(prev);
            newSet.delete(modelName);
            return newSet;
          });

          // Clean up throttle data
          progressThrottleRef.current.delete(modelName);

          toast.success(`${getModelIcon(model?.accuracy || 'Good')} ${t('messages.downloaded', { model: displayName })}`, {
            description: t('transcription.modelDownloaded'),
            duration: 4000
          });

          // A download event is not a selection or a model-ready acknowledgement.
        }
      );

      // Download error
      unlistenError = await listen<{ modelName: string; error: string }>(
        'model-download-error',
        (event) => {
          const { modelName, error } = event.payload;
          const displayName = getDisplayName(modelName);

          setModels(prevModels =>
            prevModels.map(model =>
              model.name === modelName
                ? { ...model, status: { Error: error } as ModelStatus }
                : model
            )
          );

          setDownloadingModels(prev => {
            const newSet = new Set(prev);
            newSet.delete(modelName);
            return newSet;
          });

          // Clean up throttle data
          progressThrottleRef.current.delete(modelName);

          toast.error(t('errors.download', { model: displayName }), {
            description: t('errors.download', { model: displayName }),
            duration: 6000,
            action: {
              label: t('actions.retry'),
              onClick: () => downloadModel(modelName)
            }
          });
        }
      );
    };

    setupListeners();

    return () => {
      console.log('[ModelManager] Cleaning up event listeners...');
      if (unlistenProgress) unlistenProgress();
      if (unlistenComplete) unlistenComplete();
      if (unlistenError) unlistenError();
    };
  }, [t]);

  const cancelDownload = async (modelName: string) => {
    const displayName = getDisplayName(modelName);

    try {
      await WhisperAPI.cancelDownload(modelName);

      updateDownloadingModels(prev => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });

      setModels(prevModels =>
        prevModels.map(model =>
          model.name === modelName
            ? { ...model, status: 'Missing' as ModelStatus }
            : model
        )
      );

      // Clean up throttle data
      progressThrottleRef.current.delete(modelName);

      toast.info(t('messages.downloadCancelled', { model: displayName }), {
        duration: 3000
      });
    } catch (err) {
      console.error('Failed to cancel download:', err);
      toast.error(t('errors.cancelDownload'), {
        description: t('errors.cancelDownload'),
        duration: 4000
      });
    }
  };

  const downloadModel = async (modelName: string) => {
    if (downloadingModels.has(modelName)) return;

    const displayName = getDisplayName(modelName);

    try {
      updateDownloadingModels(prev => new Set([...prev, modelName]));

      setModels(prevModels =>
        prevModels.map(model =>
          model.name === modelName
            ? { ...model, status: { Downloading: 0 } as ModelStatus }
            : model
        )
      );

      toast.info(t('status.downloadingModelEllipsis', { model: displayName }), {
        description: t('transcription.downloadMayTakeTime'),
        duration: 5000
      });

      await WhisperAPI.downloadModel(modelName);
    } catch (err) {
      console.error('Download failed:', err);
      updateDownloadingModels(prev => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });

      const errorMessage = err instanceof Error ? err.message : 'Download failed';
      setModels(prev =>
        prev.map(model =>
          model.name === modelName ? { ...model, status: { Error: errorMessage } } : model
        )
      );
    }
  };

  const selectModel = async (modelName: string) => {
    if (selectingRef.current) return;
    selectingRef.current = true;
    setSelecting(true);
    try {
      await prepareWhisperModel(modelName, autoSave);
      onModelSelect?.(modelName);
      toast.success(t('messages.switched', { model: getDisplayName(modelName) }));
    } catch (error) {
      console.error('Model preparation failed:', error);
      toast.error(t('errors.loadTranscriptionModels'), { description: t('transcription:setup.failed') });
    } finally {
      selectingRef.current = false;
      setSelecting(false);
    }
  };

  const deleteModel = async (modelName: string) => {
    const displayName = getDisplayName(modelName);

    try {
      await WhisperAPI.deleteCorruptedModel(modelName);

      // Refresh models list
      const modelList = await WhisperAPI.getAvailableModels();
      setModels(modelList);

      toast.success(t('messages.deleted', { model: displayName }), {
        description: t('transcription.modelRemoved'),
        duration: 3000
      });

      // If deleted model was selected, clear selection
      if (selectedModel === modelName && onModelSelect) {
        onModelSelect('');
      }
    } catch (err) {
      console.error('Failed to delete model:', err);
      toast.error(t('errors.delete', { model: displayName }), {
        description: t('errors.delete', { model: displayName }),
        duration: 4000
      });
    }
  };

  const getDisplayName = (modelName: string): string => {
    const modelNameMapping: { [key: string]: string } = {
      "small": t('whisper.small'),
      "medium-q5_0": t('whisper.medium'),
      "large-v3-q5_0": t('whisper.largeCompressed'),
      "large-v3-turbo": t('whisper.largeTurbo'),
      "large-v3": t('whisper.large')
    };

    const basicModelNames = ["small", "medium-q5_0", "large-v3-q5_0", "large-v3-turbo", "large-v3"];
    if (basicModelNames.includes(modelName)) {
      return modelNameMapping[modelName] || modelName;
    }
    return t('whisper.advancedName', { model: modelName });
  };

  if (loading) {
    return (
      <div className={`space-y-3 ${className}`}>
        <div className="animate-pulse space-y-3">
          <div className="h-20 bg-gray-100 rounded-lg"></div>
          <div className="h-20 bg-gray-100 rounded-lg"></div>
          <div className="h-20 bg-gray-100 rounded-lg"></div>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className={`bg-red-50 border border-red-200 rounded-lg p-4 ${className}`}>
        <p className="text-sm text-red-800">{t('transcription.failedToLoad')}</p>
      </div>
    );
  }

  const basicModelNames = ["small", "medium-q5_0", "large-v3-q5_0", "large-v3-turbo", "large-v3"];
  const basicModels = models.filter(m => basicModelNames.includes(m.name))
    .sort((a, b) => basicModelNames.indexOf(a.name) - basicModelNames.indexOf(b.name));
  const advancedModels = models.filter(m => !basicModelNames.includes(m.name));

  return (
    <div className={`space-y-3 ${className}`} aria-busy={selecting}>
      {selecting && <p role="status">{t('transcription:setup.loading')}</p>}
      {/* Basic Models */}
      <div className="space-y-3">
        {basicModels.map((model) => {
          const isRecommended = model.name === 'base';
          return (
            <ModelCard
              key={model.name}
              model={model}
              isSelected={selectedModel === model.name}
              isRecommended={isRecommended}
              onSelect={() => {
                if (model.status === 'Available') {
                  selectModel(model.name);
                }
              }}
              onDownload={() => downloadModel(model.name)}
              onCancel={() => cancelDownload(model.name)}
              onDelete={() => deleteModel(model.name)}
              isDownloading={downloadingModels.has(model.name)}
              displayName={getDisplayName(model.name)}
            />
          );
        })}
      </div>

      {/* Advanced Models */}
      {advancedModels.length > 0 && (
        <Accordion type="single" collapsible className="w-full">
          <AccordionItem value="advanced-models">
            <AccordionTrigger>
              <span className='text-lg'>{t('transcription.advancedModels')}</span>
            </AccordionTrigger>
            <AccordionContent>
              <div className="space-y-3 pt-4">
                {advancedModels.map((model) => (
                  <ModelCard
                    key={model.name}
                    model={model}
                    isSelected={selectedModel === model.name}
                    isRecommended={false}
                    onSelect={() => {
                      if (model.status === 'Available') {
                        selectModel(model.name);
                      }
                    }}
                    onDownload={() => downloadModel(model.name)}
                    onCancel={() => cancelDownload(model.name)}
                    onDelete={() => deleteModel(model.name)}
                    isDownloading={downloadingModels.has(model.name)}
                    displayName={getDisplayName(model.name)}
                  />
                ))}
              </div>
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      )}

      {/* Helper text */}
      {selectedModel && (
        <motion.div
          initial={{ opacity: 0, y: -5 }}
          animate={{ opacity: 1, y: 0 }}
          className="text-xs text-gray-500 text-center pt-2"
        >
          {t('status.usingForTranscription', { model: getDisplayName(selectedModel) })}
        </motion.div>
      )}
    </div>
  );
}

// Model Card Component
interface ModelCardProps {
  model: ModelInfo;
  isSelected: boolean;
  isRecommended: boolean;
  onSelect: () => void;
  onDownload: () => void;
  onCancel: () => void;
  onDelete: () => void;
  isDownloading: boolean;
  displayName: string;
}

function ModelCard({
  model,
  isSelected,
  isRecommended,
  onSelect,
  onDownload,
  onCancel,
  onDelete,
  isDownloading,
  displayName
}: ModelCardProps) {
  const { t } = useTranslation(['models', 'transcription']);
  const [isHovered, setIsHovered] = useState(false);
  const localizedAccuracy = model.accuracy === 'High'
    ? t('values.accuracyHigh')
    : model.accuracy === 'Good'
      ? t('values.accuracyGood')
      : t('values.accuracyDecent');
  const localizedSpeed = model.speed === 'Slow'
    ? t('values.speedSlow')
    : model.speed === 'Medium'
      ? t('values.speedMedium')
      : model.speed === 'Fast'
        ? t('values.speedFast')
        : model.speed === 'Very Fast'
          ? t('values.speedVeryFast')
          : t('values.speedUltraFast');
  const performanceLabel = model.name.includes('q5_1')
    ? t('whisper.q5_1')
    : model.name.includes('q5_0')
      ? t('whisper.q5_0')
      : t('whisper.quantized');

  const isAvailable = model.status === 'Available';
  const isMissing = model.status === 'Missing';
  const isError = typeof model.status === 'object' && 'Error' in model.status;
  const isCorrupted = typeof model.status === 'object' && 'Corrupted' in model.status;
  const downloadProgress =
    typeof model.status === 'object' && 'Downloading' in model.status
      ? model.status.Downloading
      : null;

  return (
    <motion.div
      initial={{ opacity: 0, y: 5 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2 }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      className={`
        relative rounded-lg border-2 transition-all cursor-pointer
        ${isSelected && isAvailable
          ? 'border-blue-500 bg-blue-50'
          : isAvailable
            ? 'border-gray-200 hover:border-gray-300 bg-white'
            : 'border-gray-200 bg-gray-50'
        }
        ${isAvailable ? '' : 'cursor-default'}
      `}
      onClick={() => {
        if (isAvailable) onSelect();
      }}
    >
      {/* Recommended Badge */}
      {isRecommended && (
        <div className="absolute -top-2 -right-2 bg-blue-600 text-white text-xs px-2 py-0.5 rounded-full font-medium">
          {t('status.recommended')}
        </div>
      )}

      <div className="p-3">
        <div className="flex items-start justify-between mb-2">
          <div className="flex-1">
            {/* Model Name and Tagline */}
            <div className="flex items-center gap-2 flex-wrap mb-2">
              <span className="text-2xl">{getModelIcon(model.accuracy)}</span>
              <h3 className="font-semibold text-gray-900">{displayName}</h3>
              <span className="text-sm text-gray-500">•</span>
               <span className="text-sm text-gray-500">{t('whisper.attribute', { speed: localizedSpeed, accuracy: localizedAccuracy })}</span>
              {isSelected && isAvailable && (
                <motion.span
                  initial={{ scale: 0 }}
                  animate={{ scale: 1 }}
                  className="bg-blue-600 text-white px-2 py-0.5 rounded-full text-xs font-medium flex items-center gap-1"
                >
                  ✓
                </motion.span>
              )}
              {isQuantizedModel(model.name) && (
                <span className={`px-2 py-0.5 rounded-full text-xs ${getModelPerformanceBadge(model.name).color === 'green'
                  ? 'bg-green-100 text-green-700'
                  : getModelPerformanceBadge(model.name).color === 'orange'
                    ? 'bg-orange-100 text-orange-700'
                    : 'bg-gray-100 text-gray-700'
                  }`}>
                  {performanceLabel}
                </span>
              )}
            </div>

            {/* Model Specs */}
            <div className="flex items-center space-x-4 text-sm text-gray-600 ml-9 mt-1.5">
              <span className="flex items-center space-x-1">
                <span>📦</span>
                <span>{formatFileSize(model.size_mb)}</span>
              </span>
              <span className="flex items-center space-x-1">
                <span>🎯</span>
                <span>{t('whisper.accuracySpec', { accuracy: localizedAccuracy })}</span>
              </span>
              <span className="flex items-center space-x-1">
                <span>⚡</span>
                <span>{t('whisper.speedSpec', { speed: localizedSpeed })}</span>
              </span>
            </div>
          </div>

          {/* Status/Action */}
          <div className="ml-4 flex items-center gap-2">
            {isAvailable && (
              <>
                <div className="flex items-center gap-1.5 text-green-600">
                  <div className="w-2 h-2 bg-green-500 rounded-full"></div>
                  <span className="text-xs font-medium">{t('transcription:setup.fileReady')}</span>
                </div>
                <AnimatePresence>
                  {isHovered && (
                    <motion.button
                      initial={{ opacity: 0, scale: 0.8 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.8 }}
                      transition={{ duration: 0.15 }}
                      onClick={(e) => {
                        e.stopPropagation();
                        onDelete();
                      }}
                      className="text-gray-400 hover:text-red-600 transition-colors p-1"
                      title={t('actions.deleteToFreeSpace')}
                    >
                      <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                      </svg>
                    </motion.button>
                  )}
                </AnimatePresence>
              </>
            )}

            {isMissing && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onDownload();
                }}
                className="bg-blue-600 text-white px-3 py-1.5 rounded-md text-sm font-medium hover:bg-blue-700 transition-colors"
              >
                {t('actions.download')}
              </button>
            )}

            {downloadProgress === null && isError && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onDownload();
                }}
                className="bg-red-600 text-white px-3 py-1.5 rounded-md text-sm font-medium hover:bg-red-700 transition-colors"
              >
                {t('actions.retry')}
              </button>
            )}

            {isCorrupted && (
              <div className="flex gap-2">
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onDelete();
                  }}
                  className="bg-orange-600 text-white px-3 py-1.5 rounded-md text-sm font-medium hover:bg-orange-700 transition-colors"
                >
                  {t('actions.delete')}
                </button>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    onDownload();
                  }}
                  className="bg-blue-600 text-white px-3 py-1.5 rounded-md text-sm font-medium hover:bg-blue-700 transition-colors"
                >
                  {t('actions.redownload')}
                </button>
              </div>
            )}
          </div>
        </div>

        {/* Full-width Download Progress Bar - PROMINENT */}
        {downloadProgress !== null && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            className="mt-3 pt-3 border-t border-gray-200"
          >
            <div className="flex items-center justify-between mb-2">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-blue-600">{t('status.downloading')}</span>
                <span className="text-sm font-semibold text-blue-600">{Math.round(downloadProgress)}%</span>
              </div>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  onCancel();
                }}
                className="text-xs text-gray-600 hover:text-red-600 font-medium transition-colors px-2 py-1 rounded hover:bg-red-50"
                title={t('actions.cancelDownload')}
              >
                {t('actions.cancel')}
              </button>
            </div>
            <div className="w-full h-2 bg-gray-200 rounded-full overflow-hidden">
              <motion.div
                className="h-full bg-gradient-to-r from-blue-500 to-blue-600 rounded-full"
                initial={{ width: 0 }}
                animate={{ width: `${downloadProgress}%` }}
                transition={{ duration: 0.3, ease: 'easeOut' }}
              />
            </div>
            <p className="text-xs text-gray-500 mt-1">
              {model.size_mb ? (
                <>
                  {formatFileSize(model.size_mb * downloadProgress / 100)} / {formatFileSize(model.size_mb)}
                </>
              ) : (
                t('status.downloading')
              )}
            </p>
          </motion.div>
        )}
      </div>
    </motion.div>
  );
}
