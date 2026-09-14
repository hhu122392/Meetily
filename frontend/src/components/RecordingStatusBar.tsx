'use client';

import { motion } from 'framer-motion';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { deriveRecordingInputFeedback } from '@/lib/recording-input-feedback';
import { RecordingInputLevel } from './RecordingInputMonitor';

interface RecordingStatusBarProps {
  isPaused?: boolean;
}

export const RecordingStatusBar: React.FC<RecordingStatusBarProps> = ({ isPaused = false }) => {
  const { t } = useTranslation('recording');
  // Get recording duration from backend-synced context (in seconds)
  // Backend polls every 500ms, providing smooth updates
  const { activeDuration, recordingMode, microphoneRoute, systemRoute, microphoneDataAdvanced,
    systemDataAdvanced, isPaused: backendPaused, lastError, isReconnecting } = useRecordingState();
  const paused = isPaused || backendPaused;
  const microphoneFeedback = deriveRecordingInputFeedback(microphoneRoute, activeDuration ?? 0, paused, microphoneDataAdvanced);
  const systemFeedback = deriveRecordingInputFeedback(systemRoute, activeDuration ?? 0, paused, systemDataAdvanced);

  // Display state synced from backend
  const [displaySeconds, setDisplaySeconds] = useState(0);

  // Sync with backend duration when it changes (handles refresh/navigation)
  useEffect(() => {
    if (activeDuration !== null) {
      // Round to nearest second to avoid decimal issues
      setDisplaySeconds(Math.floor(activeDuration));
    }
  }, [activeDuration]);

  const formatDuration = (seconds: number): string => {
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  };

  const systemAudioWaiting = systemRoute.active
    && systemRoute.callback_count === 0
    && !systemRoute.no_signal
    && !systemRoute.silent
    && !systemRoute.failed;

  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
      className="flex items-center gap-2 px-3 py-2 bg-gray-50 rounded-lg mb-2"
      role="status"
      aria-live="polite"
    >
      <div className={`w-2 h-2 rounded-full ${paused ? 'bg-orange-500' : 'bg-red-500 animate-pulse'}`} />
      <span className={`text-sm ${paused ? 'text-orange-700' : 'text-gray-700'}`}>
        {paused ? t('status.paused') : t('status.recording')} • {formatDuration(displaySeconds)}
      </span>
      <span className="ml-auto flex items-center gap-2 text-xs text-gray-600">
        <span>{t(`status.mode.${recordingMode}`)}</span>
        {microphoneRoute.active && (
          <span
            className={`inline-flex items-center gap-1 ${microphoneRoute.no_signal || microphoneRoute.failed ? 'text-red-700' : ''}`}
            title={[
              microphoneRoute.device_name,
              microphoneRoute.last_capture_qpc_ns
                ? `QPC ${microphoneRoute.last_capture_qpc_ns}`
                : null,
              microphoneRoute.no_signal
                ? `no callback for ${microphoneRoute.max_callback_gap_ns ?? 0} ns`
                : null,
              `RMS ${((microphoneRoute.rms_level ?? 0) * 100).toFixed(1)}%`,
              `Peak ${((microphoneRoute.peak_level ?? 0) * 100).toFixed(1)}%`,
            ].filter(Boolean).join(' · ') || undefined}
          >
            {t('status.microphoneRoute')} {microphoneRoute.no_signal || microphoneRoute.failed ? '!' : microphoneRoute.callback_count > 0 ? '●' : '○'}{' '}
            <RecordingInputLevel label={t('monitor.levelLabel', { route: t('monitor.route.microphone') })}
              levelPercent={microphoneFeedback.levelPercent} className="w-12 shrink-0" />
          </span>
        )}
        {systemRoute.active && (
          <span
            className={`inline-flex items-center gap-1 ${systemRoute.no_signal || systemRoute.silent || systemRoute.failed
              ? 'text-red-700'
              : systemAudioWaiting
                ? 'text-amber-700'
                : ''}`}
            title={[
              systemRoute.device_name,
              systemRoute.format
                ? `${systemRoute.format.sample_rate} Hz / ${systemRoute.format.channels} ch / ${systemRoute.format.sample_format}`
                : null,
              systemRoute.last_capture_qpc_ns
                ? `QPC ${systemRoute.last_capture_qpc_ns}`
                : null,
              systemRoute.no_signal
                ? `no callback for ${systemRoute.max_callback_gap_ns ?? 0} ns`
                : null,
              systemRoute.driver_mute_behavior
                ? `mute ${systemRoute.driver_mute_behavior}`
                : null,
              `RMS ${((systemRoute.rms_level ?? 0) * 100).toFixed(1)}%`,
              `Peak ${((systemRoute.peak_level ?? 0) * 100).toFixed(1)}%`,
            ].filter(Boolean).join(' · ') || undefined}
          >
            {t('status.systemRoute')}{' '}
            {systemAudioWaiting
              ? `○ ${t('status.waitingForSystemAudio')}`
              : systemRoute.no_signal || systemRoute.silent || systemRoute.failed ? '!' : '●'}
            <RecordingInputLevel label={t('monitor.levelLabel', { route: t('monitor.route.system') })}
              levelPercent={systemFeedback.levelPercent} className="w-12 shrink-0" />
          </span>
        )}
        {(isReconnecting || microphoneRoute.no_signal || microphoneRoute.failed || systemRoute.no_signal || systemRoute.silent || systemRoute.failed) && (
          <span className="text-red-700" title={lastError ?? 'Audio route is reconnecting'}>
            {t('status.audioError')}{isReconnecting ? '…' : ''}
          </span>
        )}
        {(microphoneRoute.no_signal || microphoneRoute.failed || systemRoute.no_signal || systemRoute.silent || systemRoute.failed) && (
          <span className="text-red-700">{t('status.importExistingAudio')}</span>
        )}
      </span>
    </motion.div>
  );
};
