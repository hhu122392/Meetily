'use client';

import React from 'react';
import { X, Info, Shield } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface AnalyticsDataModalProps {
  isOpen: boolean;
  onClose: () => void;
  onConfirmDisable: () => void;
}

export default function AnalyticsDataModal({ isOpen, onClose, onConfirmDisable }: AnalyticsDataModalProps) {
  const { t } = useTranslation('analytics');
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="analytics-transparency-title"
        className="bg-white rounded-lg shadow-xl max-w-2xl w-full mx-4 max-h-[90vh] overflow-y-auto"
      >
        {/* Header */}
        <div className="flex items-center justify-between p-6 border-b border-gray-200">
          <div className="flex items-center gap-3">
            <Shield className="w-6 h-6 text-blue-600" />
            <h2 id="analytics-transparency-title" className="text-xl font-semibold text-gray-900">{t('labels.whatAnalyticsCollects')}</h2>
          </div>
          <button
            onClick={onClose}
            type="button"
            aria-label={t('accessibility.closeTransparencyDialog')}
            className="text-gray-400 hover:text-gray-600 transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="p-6 space-y-6">
          {/* Privacy Notice */}
          <div className="bg-green-50 border border-green-200 rounded-lg p-4">
            <div className="flex items-start gap-3">
              <Info className="w-5 h-5 text-green-600 mt-0.5 flex-shrink-0" />
              <div className="text-sm text-green-800">
                <p className="font-semibold mb-1">{t('labels.yourPrivacyIsProtected')}</p>
                <p>{t('labels.analyticsIsOffByDefaultIfYouEnableItWe')} <strong>{t('labels.anonymousUsageDataOnly')}</strong>. {t('descriptions.anonymousOnlySuffix')}</p>
              </div>
            </div>
          </div>

          {/* Data Categories */}
          <div className="space-y-4">
            <h3 className="text-lg font-semibold text-gray-900">{t('labels.dataWeCollectWhenEnabled')}</h3>

            {/* Model Preferences */}
            <div className="border border-gray-200 rounded-lg p-4">
              <h4 className="font-semibold text-gray-900 mb-2">{t('labels.text1ModelPreferences')}</h4>
              <ul className="text-sm text-gray-700 space-y-1 ml-4">
                <li>{t('labels.transcriptionModelEGWhisperLargeV3Parakeet')}</li>
                <li>{t('labels.summaryModelEGLlama32ClaudeSonnet')}</li>
                <li>{t('labels.modelProviderEGLocalOllamaOpenRouter')}</li>
              </ul>
              <p className="text-xs text-gray-500 mt-2 italic">{t('labels.helpsUsUnderstandWhichModelsUsersPrefer')}</p>
            </div>

            {/* Meeting Metrics */}
            <div className="border border-gray-200 rounded-lg p-4">
              <h4 className="font-semibold text-gray-900 mb-2">{t('labels.text2AnonymousMeetingMetrics')}</h4>
              <ul className="text-sm text-gray-700 space-y-1 ml-4">
                <li>{t('labels.recordingDurationEG125Seconds')}</li>
                <li>{t('labels.pauseDurationEG5Seconds')}</li>
                <li>{t('labels.numberOfTranscriptSegments')}</li>
                <li>{t('labels.numberOfAudioChunksProcessed')}</li>
              </ul>
              <p className="text-xs text-gray-500 mt-2 italic">{t('labels.helpsUsOptimizePerformanceAndUnderstandUsagePatterns')}</p>
            </div>

            {/* Device Types */}
            <div className="border border-gray-200 rounded-lg p-4">
              <h4 className="font-semibold text-gray-900 mb-2">{t('labels.text3DeviceTypesNotNames')}</h4>
              <ul className="text-sm text-gray-700 space-y-1 ml-4">
                <li>{t('labels.microphoneTypeBluetoothOrWiredOrUnknown')}</li>
                <li>{t('labels.systemAudioTypeBluetoothOrWiredOrUnknown')}</li>
              </ul>
              <p className="text-xs text-gray-500 mt-2 italic">{t('labels.helpsUsImproveCompatibilityNOTTheActualDeviceNames')}</p>
            </div>

            {/* Usage Patterns */}
            <div className="border border-gray-200 rounded-lg p-4">
              <h4 className="font-semibold text-gray-900 mb-2">{t('labels.text4AppUsagePatterns')}</h4>
              <ul className="text-sm text-gray-700 space-y-1 ml-4">
                <li>{t('labels.appStartedStoppedEvents')}</li>
                <li>{t('labels.sessionDuration')}</li>
                <li>{t('labels.featureUsageEGSettingsChanged')}</li>
                <li>{t('labels.errorOccurrencesHelpsUsFixBugs')}</li>
              </ul>
              <p className="text-xs text-gray-500 mt-2 italic">{t('labels.helpsUsImproveUserExperience')}</p>
            </div>

            {/* Platform Info */}
            <div className="border border-gray-200 rounded-lg p-4">
              <h4 className="font-semibold text-gray-900 mb-2">{t('labels.text5PlatformInformation')}</h4>
              <ul className="text-sm text-gray-700 space-y-1 ml-4">
                <li>{t('labels.operatingSystemEGMacOSWindows')}</li>
                <li>{t('labels.appVersionAutomaticallyIncludedInAllEvents')}</li>
                <li>{t('labels.architectureEGX8664Aarch64')}</li>
              </ul>
              <p className="text-xs text-gray-500 mt-2 italic">{t('labels.helpsUsPrioritizePlatformSupport')}</p>
            </div>
          </div>

          {/* What We DON'T Collect */}
          <div className="bg-red-50 border border-red-200 rounded-lg p-4">
            <h4 className="font-semibold text-red-900 mb-2">{t('labels.whatWeDONTCollect')}</h4>
            <ul className="text-sm text-red-800 space-y-1 ml-4">
              <li>{t('labels.meetingNamesOrTitles')}</li>
              <li>{t('labels.fileNamesFilePathsOrMeetingFolders')}</li>
              <li>{t('labels.meetingTranscriptsOrContent')}</li>
              <li>{t('labels.audioRecordings')}</li>
              <li>{t('labels.deviceNamesOnlyTypesBluetoothWired')}</li>
              <li>{t('labels.personalInformation')}</li>
              <li>{t('labels.anyIdentifiableData')}</li>
            </ul>
          </div>

          {/* Example Event */}
          <div className="bg-gray-50 border border-gray-200 rounded-lg p-4">
            <h4 className="font-semibold text-gray-900 mb-2">{t('labels.exampleEvent')}</h4>
            <pre className="text-xs text-gray-700 overflow-x-auto">
              {`{
  "event": "meeting_ended",
  "app_version": "0.4.0",
  "transcription_provider": "parakeet",
  "transcription_model": "parakeet-tdt-0.6b-v3-int8",
  "summary_provider": "ollama",
  "summary_model": "llama3.2:latest",
  "total_duration_seconds": "125.5",
  "microphone_device_type": "Wired",
  "system_audio_device_type": "Bluetooth",
  "chunks_processed": "150",
  "had_fatal_error": "false"
}`}
            </pre>
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between gap-4 p-6 border-t border-gray-200 bg-gray-50">
          <button
            onClick={onClose}
            type="button"
            className="px-4 py-2 text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 transition-colors"
          >
            {t('actions.keepEnabled')}
          </button>
          <button
            onClick={onConfirmDisable}
            type="button"
            className="px-4 py-2 text-white bg-red-600 rounded-md hover:bg-red-700 transition-colors"
          >
            {t('actions.confirmDisable')}
          </button>
        </div>
      </div>
    </div>
  );
}
