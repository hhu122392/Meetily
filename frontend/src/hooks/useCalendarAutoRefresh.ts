'use client';
import { useCallback, useEffect, useState } from 'react';

const KEY = 'meetily.calendar.autoRefresh';
const EVENT = 'meetily-calendar-refresh-preference';
export const CALENDAR_REFRESH_INTERVAL = 5 * 60 * 1000;

function readPreference(): boolean {
  try { return window.localStorage.getItem(KEY) === 'true'; }
  catch { return false; }
}

export function useCalendarAutoRefresh() {
  const [enabled, setEnabled] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const reload = () => setEnabled(readPreference());
    reload();
    window.addEventListener(EVENT, reload);
    window.addEventListener('storage', reload);
    return () => {
      window.removeEventListener(EVENT, reload);
      window.removeEventListener('storage', reload);
    };
  }, []);
  const update = useCallback((value: boolean) => {
    try {
      window.localStorage.setItem(KEY, String(value));
      setEnabled(value);
      setError('');
      window.dispatchEvent(new Event(EVENT));
    } catch { setError('自动刷新设置保存失败，请重试。'); }
  }, []);
  return { enabled, update, error };
}
