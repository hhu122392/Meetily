'use client';
import { useCallback, useEffect, useRef, useState } from 'react';
import { downloadSenseVoice, readSenseVoiceState, SenseVoiceState, SENSEVOICE_BYTES } from '@/lib/sensevoice';

export function useSenseVoiceModel() {
  const [state, setState] = useState<SenseVoiceState>({ status: 'checking', downloaded_bytes: 0, total_bytes: SENSEVOICE_BYTES });
  const mounted = useRef(false);
  const busy = useRef(false);
  const refresh = useCallback(async () => {
    try {
      const next = await readSenseVoiceState();
      if (mounted.current) setState(next);
      return next;
    } catch (error) {
      console.warn('SenseVoice status failed', error);
      if (mounted.current) setState(prev => ({ ...prev, status: 'error' }));
      return null;
    }
  }, []);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; };
  }, [refresh]);
  useEffect(() => {
    if (state.status !== 'downloading') return;
    const timer = setInterval(() => { void refresh(); }, 1500);
    return () => clearInterval(timer);
  }, [state.status, refresh]);
  const download = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setState(prev => ({ ...prev, status: 'downloading' }));
    try {
      const next = await downloadSenseVoice();
      if (mounted.current) setState(next);
    } catch (error) {
      console.warn('SenseVoice download failed', error);
      // A second view may have started the download; retain its live status.
      const next = await refresh();
      if (mounted.current && next?.status !== 'downloading') setState(prev => ({ ...prev, status: 'error' }));
    } finally { busy.current = false; }
  }, [refresh]);
  return { state, download, refresh };
}
