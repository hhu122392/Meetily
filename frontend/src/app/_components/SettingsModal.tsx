import { ModelConfig } from "@/components/ModelSettingsModal";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { DeviceSelection } from "@/components/DeviceSelection";
import { LanguageSelection } from "@/components/LanguageSelection";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { toast } from "sonner";
import { useConfig } from "@/contexts/ConfigContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { useTranslation } from "react-i18next";
import { useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { shouldShowWhisperPreparation, prepareWhisperModel, type ModelPreparationStep } from "@/lib/transcription-setup";

type modalType = "modelSettings" | "deviceSettings" | "languageSettings" | "modelSelector" | "errorAlert" | "chunkDropWarning";

/**
 * SettingsModals Component
 *
 * All settings modals consolidated into a single component.
 * Uses ConfigContext and RecordingStateContext internally - no prop drilling needed!
 */

interface SettingsModalsProps {
  modals: {
    modelSettings: boolean;
    deviceSettings: boolean;
    languageSettings: boolean;
    modelSelector: boolean;
    errorAlert: boolean;
    chunkDropWarning: boolean;
  };
  messages: {
    errorAlert: string;
    chunkDropWarning: string;
    modelSelector: string;
  };
  onClose: (name: modalType) => void;
}

export function SettingsModals({
  modals,
  messages,
  onClose,
}: SettingsModalsProps) {
  const { t } = useTranslation(['recording', 'transcription', 'settings', 'models']);
  // Contexts
  const {
    modelConfig,
    setModelConfig,
    models,
    modelOptions,
    error,
    selectedDevices,
    setSelectedDevices,
    selectedLanguage,
    setSelectedLanguage,
    transcriptModelConfig,
    setTranscriptModelConfig,
    showConfidenceIndicator,
    toggleConfidenceIndicator,
  } = useConfig();

  const { isRecording, isStopping, isProcessing, isSaving } = useRecordingState();
  const preparingRef = useRef(false);
  const [preparing, setPreparing] = useState(false);
  const [preparationStep, setPreparationStep] = useState<ModelPreparationStep | 'ready' | null>(null);
  const [preparationError, setPreparationError] = useState('');
  const [downloadProgress, setDownloadProgress] = useState<number | null>(null);
  const settingsLocked = isRecording || isStopping || isProcessing || isSaving || preparing;
  const showWhisperPreparation = shouldShowWhisperPreparation(transcriptModelConfig.provider, transcriptModelConfig.model);
  const preparationModel = showWhisperPreparation
    ? 'small' : transcriptModelConfig.model;

  const prepareModel = async () => {
    if (settingsLocked || preparingRef.current) return;
    preparingRef.current = true;
    setPreparing(true);
    setPreparationError('');
    setDownloadProgress(null);
    setPreparationStep('checking');
    let step: ModelPreparationStep = 'checking';
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await listen<{ modelName: string; progress: number }>('model-download-progress', ({ payload }) => {
        if (payload.modelName === preparationModel) setDownloadProgress(payload.progress);
      });
      await setSelectedLanguage(selectedLanguage);
      await prepareWhisperModel(preparationModel, true, nextStep => {
        step = nextStep;
        setPreparationStep(nextStep);
      });
      setTranscriptModelConfig({ provider: 'localWhisper', model: preparationModel, apiKey: null });
      setPreparationStep('ready');
    } catch (error) {
      setPreparationStep(null);
      console.error('Transcription setup failed:', error);
      setPreparationError(t(`transcription:setup.failureSteps.${step}`));
    } finally {
      unlisten?.();
      preparingRef.current = false;
      setPreparing(false);
    }
  };

  const preparationControls = (
    <div className="my-4 rounded-lg border border-blue-200 bg-blue-50 p-4 space-y-2">
      <p className="text-sm text-slate-700">{t('transcription:setup.explanation')}</p>
      <button type="button" onClick={prepareModel} disabled={settingsLocked}
        className="rounded-md bg-blue-600 px-4 py-2 text-sm text-white disabled:opacity-50">
        {t('transcription:setup.prepare', { model: preparationModel })}
      </button>
      {preparationStep && (preparationStep !== 'ready' || !showWhisperPreparation) && <p role="status" className="text-sm">
        {t(`transcription:setup.${preparationStep}`)}
        {preparationStep === 'downloading' && downloadProgress !== null && ` ${downloadProgress}%`}
      </p>}
      {preparationError && <p role="alert" className="text-sm text-red-700">{t('transcription:setup.failed')} {preparationError}</p>}
    </div>
  );

  return <>
    {/* Legacy Settings Modal */}
    {modals.modelSettings && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50 p-4">
        <div className="bg-white rounded-lg shadow-xl max-w-4xl w-full max-h-[90vh] overflow-hidden flex flex-col">
          {/* Header */}
          <div className="flex justify-between items-center p-6 border-b">
            <h3 className="text-xl font-semibold text-gray-900">{t('settings:preferences.title')}</h3>
            <button
              onClick={() => onClose("modelSettings")
              }
              aria-label={t('settings:accessibility.closePreferences')}
              className="text-gray-500 hover:text-gray-700"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          {/* Content - Scrollable */}
          <div className="flex-1 overflow-y-auto p-6 space-y-8">
            {/* General Preferences Section */}
            <PreferenceSettings />

            {/* Divider */}
            <div className="border-t pt-8">
              <h4 className="text-lg font-semibold text-gray-900 mb-4">{t('settings:summary.modelConfiguration')}</h4>
              <div className="space-y-4">
                <div>
                  <label className="block text-sm font-medium text-gray-700 mb-1">
                    {t('models:config.summaryModel')}
                  </label>
                  <div className="flex space-x-2">
                    <select
                      className="px-3 py-2 text-sm bg-white border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                      value={modelConfig.provider}
                      onChange={(e) => {
                        const provider = e.target.value as ModelConfig['provider'];
                        setModelConfig({
                          ...modelConfig,
                          provider,
                          model: modelOptions[provider][0]
                        });
                      }}
                    >
                      <option value="builtin-ai">{t('models:providers.builtInShort')}</option>
                      <option value="claude">{t('models:providers.claude')}</option>
                      <option value="groq">{t('models:providers.groq')}</option>
                      <option value="ollama">{t('models:providers.ollama')}</option>
                      <option value="openrouter">{t('models:providers.openRouter')}</option>
                      <option value="openai">{t('models:providers.openAI')}</option>
                    </select>

                    <select
                      className="flex-1 px-3 py-2 text-sm bg-white border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                      value={modelConfig.model}
                      onChange={(e) => setModelConfig((prev: ModelConfig) => ({ ...prev, model: e.target.value }))}
                    >
                      {modelOptions[modelConfig.provider].map((model: string) => (
                        <option key={model} value={model}>
                          {model}
                        </option>
                      ))}
                    </select>
                  </div>
                </div>
                {modelConfig.provider === 'ollama' && (
                  <div>
                    <h4 className="text-lg font-bold mb-4">{t('models:config.availableOllama')}</h4>
                    {error && (
                      <div className="bg-red-100 border border-red-400 text-red-700 px-4 py-3 rounded mb-4">
                        {t('models:errors.loadModels')}
                      </div>
                    )}
                    <div className="grid gap-4 max-h-[400px] overflow-y-auto pr-2">
                      {models.map((model) => (
                        <div
                          key={model.id}
                          className={`bg-white p-4 rounded-lg shadow cursor-pointer transition-colors ${modelConfig.model === model.name ? 'ring-2 ring-blue-500 bg-blue-50' : 'hover:bg-gray-50'
                            }`}
                          onClick={() => setModelConfig((prev: ModelConfig) => ({ ...prev, model: model.name }))}
                        >
                          <h3 className="font-bold">{model.name}</h3>
                          <p className="text-gray-600">{t('models:config.modelSize', { size: model.size })}</p>
                          <p className="text-gray-600">{t('models:config.modelModified', { modified: model.modified })}</p>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          </div>

          {/* Footer */}
          <div className="border-t p-6 flex justify-end">
            <button
              onClick={() => onClose('modelSettings')}
              className="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
            >
              {t('settings:common.done')}
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Device Settings Modal */}
    {modals.deviceSettings && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
        <div className="bg-white rounded-lg p-6 max-w-md w-full mx-4 shadow-xl">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-gray-900">{t('settings:devices.settingsTitle')}</h3>
            <button
              onClick={() => onClose('deviceSettings')}
              aria-label={t('settings:accessibility.closeDeviceSettings')}
              className="text-gray-500 hover:text-gray-700"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <DeviceSelection
            selectedDevices={selectedDevices}
            onDeviceChange={setSelectedDevices}
            disabled={isRecording}
          />

          <div className="mt-6 flex justify-end">
            <button
              onClick={() => {
                const micDevice = selectedDevices.micDevice || t('settings:common.default');
                const systemDevice = selectedDevices.systemDevice || t('settings:common.default');
                toast.success(t('settings:devices.selected'), {
                  description: t('settings:devices.selectionDescription', { micDevice, systemDevice })
                });
                onClose('deviceSettings');
              }}
              className="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
            >
              {t('settings:common.done')}
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Language Settings Modal */}
    {modals.languageSettings && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
        <div className="bg-white rounded-lg p-6 max-w-md w-full mx-4 shadow-xl">
          <div className="flex justify-between items-center mb-4">
            <h3 className="text-lg font-semibold text-gray-900">{t('settings:language.settingsTitle')}</h3>
            <button
              onClick={() => onClose('languageSettings')}
              aria-label={t('settings:accessibility.closeLanguageSettings')}
              className="text-gray-500 hover:text-gray-700"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          <LanguageSelection
            selectedLanguage={selectedLanguage}
            onLanguageChange={setSelectedLanguage}
            disabled={settingsLocked}
            provider={transcriptModelConfig.provider}
          />
          {showWhisperPreparation && preparationControls}

          <div className="mt-6 flex justify-end">
            <button
              onClick={() => onClose('languageSettings')}
              className="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500"
            >
              {t('settings:common.done')}
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Model Selection Modal */}
    {modals.modelSelector && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
        <div className="bg-white rounded-lg max-w-4xl w-full mx-4 shadow-xl max-h-[90vh] flex flex-col">
          {/* Fixed Header */}
          <div className="flex justify-between items-center p-6 pb-4 border-b border-gray-200">
            <h3 className="text-lg font-semibold text-gray-900">
              {messages.modelSelector ? t('settings:transcription.setupRequired') : t('settings:transcription.modelSettings')}
            </h3>
            <button
              onClick={() => onClose('modelSelector')}
              aria-label={t('settings:accessibility.closeModelSettings')}
              className="text-gray-500 hover:text-gray-700"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="h-6 w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          </div>

          {/* Scrollable Content */}
          <div className="flex-1 overflow-y-auto p-6 pt-4">
            {messages.modelSelector && <p role="alert" className="mb-4 text-amber-800">{messages.modelSelector}</p>}
            <LanguageSelection selectedLanguage={selectedLanguage} onLanguageChange={setSelectedLanguage}
              disabled={settingsLocked} provider={transcriptModelConfig.provider} />
            {showWhisperPreparation && preparationControls}
            {!settingsLocked && <details><summary className="cursor-pointer text-sm">{t('transcription:labels.advancedModels')}</summary>
            <TranscriptSettings
              transcriptModelConfig={transcriptModelConfig}
              setTranscriptModelConfig={setTranscriptModelConfig}
              onModelSelect={() => onClose('modelSelector')}
            />
            </details>}
          </div>

          {/* Fixed Footer */}
          <div className="p-6 pt-4 border-t border-gray-200 flex items-center justify-between">
            {/* Confidence Indicator Toggle */}
            <div className="flex items-center gap-3">
              <label className="relative inline-flex items-center cursor-pointer">
                <input
                  type="checkbox"
                  checked={showConfidenceIndicator}
                  onChange={(e) => toggleConfidenceIndicator(e.target.checked)}
                  className="sr-only peer"
                />
                <div className="w-11 h-6 bg-gray-200 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-blue-300 rounded-full peer peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:start-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-blue-600"></div>
              </label>
              <div>
                <p className="text-sm font-medium text-gray-700">{t('settings:transcription.showConfidence')}</p>
                <p className="text-xs text-gray-500">{t('settings:transcription.showConfidenceDescription')}</p>
              </div>
            </div>

            <button
              onClick={() => onClose('modelSelector')}
              className="px-4 py-2 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-gray-500"
            >
              {messages.modelSelector ? t('settings:common.cancel') : t('settings:common.done')}
            </button>
          </div>
        </div>
      </div>
    )}

    {/* Error Alert Modal */}
    {modals.errorAlert && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
        <Alert className="max-w-md mx-4 border-red-200 bg-white shadow-xl">
          <AlertTitle className="text-red-800">{t('recording:status.recordingStopped')}</AlertTitle>
          <AlertDescription className="text-red-700">
            {messages.errorAlert}
            <button
              onClick={() => onClose('errorAlert')}
              className="ml-2 text-red-600 hover:text-red-800 underline"
            >
              {t('transcription:actions.dismiss')}
            </button>
          </AlertDescription>
        </Alert>
      </div>
    )}

    {/* Chunk Drop Warning Modal */}
    {modals.chunkDropWarning && (
      <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
        <Alert className="max-w-lg mx-4 border-yellow-200 bg-white shadow-xl">
          <AlertTitle className="text-yellow-800">{t('transcription:labels.performanceWarning')}</AlertTitle>
          <AlertDescription className="text-yellow-700">
            {messages.chunkDropWarning}
            <button
              onClick={() => onClose('chunkDropWarning')}
              className="ml-2 text-yellow-600 hover:text-yellow-800 underline"
            >
              {t('transcription:actions.dismiss')}
            </button>
          </AlertDescription>
        </Alert>
      </div>
    )}
  </>
}
