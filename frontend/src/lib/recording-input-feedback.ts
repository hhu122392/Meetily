export type RecordingInputFeedbackState =
  | 'waiting'
  | 'missing'
  | 'receiving'
  | 'quiet'
  | 'problem'
  | 'paused';

export interface RecordingInputRouteSnapshot {
  active: boolean;
  callback_count: number;
  sample_count: number;
  rms_level?: number;
  peak_level?: number;
  failed?: boolean;
  no_signal?: boolean;
  silent?: boolean;
}

export interface RecordingInputFeedback {
  state: RecordingInputFeedbackState;
  levelPercent: number;
  showSettings: boolean;
}

const NO_INPUT_WARNING_SECONDS = 5;
const AUDIBLE_LEVEL_FLOOR = 0.001;

function clampLevel(value: number | undefined): number {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.min(1, value ?? 0));
}

function displayLevelPercent(route: RecordingInputRouteSnapshot): number {
  const level = clampLevel(route.rms_level);
  if (level <= 0) return 0;
  return Math.round(Math.log10(level * 9 + 1) * 100);
}

export function deriveRecordingInputFeedback(
  route: RecordingInputRouteSnapshot,
  activeDurationSeconds: number,
  paused: boolean,
  dataAdvancedSinceLastPoll: boolean,
): RecordingInputFeedback {
  if (paused) {
    return { state: 'paused', levelPercent: 0, showSettings: false };
  }

  if (route.failed || route.no_signal) {
    return { state: 'problem', levelPercent: 0, showSettings: true };
  }

  if (route.callback_count === 0) {
    return activeDurationSeconds >= NO_INPUT_WARNING_SECONDS
      ? { state: 'missing', levelPercent: 0, showSettings: true }
      : { state: 'waiting', levelPercent: 0, showSettings: false };
  }

  const levelPercent = displayLevelPercent(route);
  const hasAudibleLevel = clampLevel(route.rms_level) >= AUDIBLE_LEVEL_FLOOR;

  if (route.silent || !dataAdvancedSinceLastPoll || !hasAudibleLevel) {
    return { state: 'quiet', levelPercent: 0, showSettings: false };
  }

  return { state: 'receiving', levelPercent, showSettings: false };
}
