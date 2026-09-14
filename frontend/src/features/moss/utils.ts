import type { MossRunSummary } from './types';

export function isMossRunActive(run: MossRunSummary): boolean {
  return run.state === 'preparing' || run.state === 'running' || run.state === 'cancel_requested';
}

export function mossRunFailureI18nKey(errorCode: string | null): 'errors.audioTooLong' | 'errors.operationFailed' {
  return errorCode === 'MOSS_AUDIO_TOO_LONG' ? 'errors.audioTooLong' : 'errors.operationFailed';
}

export function shortMossHash(value: string | null): string {
  return value ? `${value.slice(0, 8)}…${value.slice(-8)}` : '—';
}

export function formatMossBytes(value: number | null, locale: string): string {
  if (value === null) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = value;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: unit === 0 ? 0 : 1 }).format(size)} ${units[unit]}`;
}

export function formatMossTimestamp(milliseconds: number): string {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  return hours > 0
    ? `${hours}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`
    : `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

export class MossOperationGate {
  private readonly active = new Set<string>();

  async run<T>(key: string, operation: () => Promise<T>): Promise<T | null> {
    if (this.active.has(key)) return null;
    this.active.add(key);
    try {
      return await operation();
    } finally {
      this.active.delete(key);
    }
  }

  has(key: string): boolean {
    return this.active.has(key);
  }
}
