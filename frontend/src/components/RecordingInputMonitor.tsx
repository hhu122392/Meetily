'use client';

import { AudioLines, Mic, MonitorSpeaker, Settings2 } from 'lucide-react';
import { motion } from 'framer-motion';
import { useRouter } from 'next/navigation';
import { useTranslation } from 'react-i18next';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import type { RecordingRouteState } from '@/services/recordingService';
import {
  deriveRecordingInputFeedback,
  type RecordingInputFeedback,
} from '@/lib/recording-input-feedback';

interface RecordingInputMonitorProps {
  isPaused?: boolean;
  compact?: boolean;
  /** P0-1: 已收到的转写片段数，用于把"还在识别"这件事显式展示出来 */
  transcriptCount?: number;
}

function formatElapsed(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const mm = String(Math.floor(total / 60)).padStart(2, '0');
  const ss = String(total % 60).padStart(2, '0');
  return `${mm}:${ss}`;
}

const feedbackStyles: Record<RecordingInputFeedback['state'], {
  badge: string;
  bar: string;
  card: string;
}> = {
  waiting: {
    badge: 'bg-blue-50 text-blue-700',
    bar: 'bg-blue-500',
    card: 'border-blue-100 bg-blue-50/30',
  },
  missing: {
    badge: 'bg-amber-100 text-amber-800',
    bar: 'bg-amber-500',
    card: 'border-amber-200 bg-amber-50',
  },
  receiving: {
    badge: 'bg-emerald-100 text-emerald-800',
    bar: 'bg-emerald-500',
    card: 'border-emerald-200 bg-emerald-50/60',
  },
  quiet: {
    badge: 'bg-slate-100 text-slate-700',
    bar: 'bg-slate-400',
    card: 'border-slate-200 bg-slate-50',
  },
  problem: {
    badge: 'bg-red-100 text-red-800',
    bar: 'bg-red-500',
    card: 'border-red-200 bg-red-50',
  },
  paused: {
    badge: 'bg-orange-100 text-orange-800',
    bar: 'bg-orange-400',
    card: 'border-orange-200 bg-orange-50',
  },
};

export function RecordingInputLevel({ label, levelPercent, barClassName = 'bg-emerald-500', className = 'flex-1' }: {
  label: string;
  levelPercent: number;
  barClassName?: string;
  className?: string;
}) {
  return (
    <span className={`inline-block h-2 overflow-hidden rounded-full bg-slate-200 ${className}`}
      role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={levelPercent}>
      <motion.span className={`block h-full rounded-full ${barClassName}`}
        animate={{ width: `${levelPercent}%` }} transition={{ duration: 0.2, ease: 'easeOut' }} />
    </span>
  );
}

export function RecordingInputMonitor({
  isPaused = false,
  compact = false,
  transcriptCount = 0,
}: RecordingInputMonitorProps) {
  const { t } = useTranslation('recording');
  const router = useRouter();
  const {
    activeDuration,
    microphoneRoute,
    systemRoute,
    microphoneDataAdvanced,
    systemDataAdvanced,
    isPaused: backendPaused,
  } = useRecordingState();

  const paused = isPaused || backendPaused;
  const elapsed = activeDuration ?? 0;

  const activeRoutes: Array<{
    id: 'microphone' | 'system';
    route: RecordingRouteState;
    feedback: RecordingInputFeedback;
  }> = [];

  if (microphoneRoute.active) {
    activeRoutes.push({
      id: 'microphone',
      route: microphoneRoute,
      feedback: deriveRecordingInputFeedback(
        microphoneRoute,
        elapsed,
        paused,
        microphoneDataAdvanced,
      ),
    });
  }
  if (systemRoute.active) {
    activeRoutes.push({
      id: 'system',
      route: systemRoute,
      feedback: deriveRecordingInputFeedback(
        systemRoute,
        elapsed,
        paused,
        systemDataAdvanced,
      ),
    });
  }

  const showSettings = activeRoutes.some(({ feedback }) => feedback.showSettings);

  return (
    <div
      className={`mx-auto w-full ${compact ? 'max-w-lg' : 'max-w-md'} rounded-xl border border-slate-200 bg-white p-4 text-left shadow-sm`}
      aria-live="polite"
    >
      <div className="mb-3 flex items-center gap-2 text-sm font-semibold text-slate-800">
        <AudioLines className="h-4 w-4" aria-hidden="true" />
        <span>{t('monitor.title')}</span>
        <span className="ml-auto text-xs font-normal text-slate-500">
          {t('monitor.realData')}
        </span>
      </div>

      {!paused && (
        <div className="mb-3 flex flex-col gap-0.5 text-xs text-slate-600" role="status" aria-live="polite">
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-blue-500 animate-pulse" aria-hidden="true" />
            <span className="font-medium">
              {transcriptCount > 0
                ? t('monitor.recognizingNext', { count: transcriptCount })
                : t('monitor.recognizingFirst', { time: formatElapsed(elapsed) })}
            </span>
          </div>
          {transcriptCount === 0 && (
            <span className="pl-4 text-slate-500">{t('monitor.firstCaptionHint')}</span>
          )}
        </div>
      )}

      <div className="space-y-3">
        {activeRoutes.map(({ id, route, feedback }) => {
          const styles = feedbackStyles[feedback.state];
          const Icon = id === 'system' ? MonitorSpeaker : Mic;

          return (
            <div key={id} className={`rounded-lg border p-3 ${styles.card}`}>
              <div className="flex items-center gap-2">
                <Icon className="h-4 w-4 text-slate-600" aria-hidden="true" />
                <span className="text-sm font-medium text-slate-800">
                  {t(`monitor.route.${id}`)}
                </span>
                <span className={`ml-auto rounded-full px-2 py-0.5 text-xs font-medium ${styles.badge}`}>
                  {t(`monitor.state.${feedback.state}`)}
                </span>
              </div>

              <div className="mt-2 flex items-center gap-2">
                <RecordingInputLevel
                  label={t('monitor.levelLabel', { route: t(`monitor.route.${id}`) })}
                  levelPercent={feedback.levelPercent}
                  barClassName={styles.bar}
                />
              </div>

              <p className="mt-2 text-xs text-slate-600">
                {t(`monitor.hint.${feedback.state}`, {
                  device: route.device_name ?? t(`monitor.route.${id}`),
                })}
              </p>
            </div>
          );
        })}
      </div>

      {showSettings && (
        <button
          type="button"
          onClick={() => router.push('/settings?tab=recording')}
          className="mt-3 inline-flex w-full items-center justify-center gap-2 rounded-lg bg-slate-900 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-slate-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-slate-500 focus-visible:ring-offset-2"
        >
          <Settings2 className="h-4 w-4" aria-hidden="true" />
          {t('monitor.openSettings')}
        </button>
      )}
    </div>
  );
}
