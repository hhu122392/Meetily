import { useEffect, useRef, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
export type MeetingSummarySettings = { templateId: string; customPrompt: string; participants: string; absent: string; host: string };
const defaults: MeetingSummarySettings = { templateId: 'standard_meeting', customPrompt: '', participants: '', absent: '', host: '' };
export function useMeetingSummarySettings(meetingId: string) {
  const [state, setState] = useState({ id: '', settings: defaults, saved: defaults });
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const currentId = useRef(meetingId); currentId.current = meetingId;
  const ready = state.id === meetingId;
  const settings = ready ? state.settings : defaults;
  useEffect(() => {
    let active = true; setError(''); setSaving(false);
    invoke<MeetingSummarySettings>('meeting_summary_settings_read', { meetingId }).then(settings => {
      if (active) setState({ id: meetingId, settings, saved: settings });
    }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [meetingId]);
  const update = useCallback((patch: Partial<MeetingSummarySettings>) => {
    setState(previous => previous.id === meetingId ? { ...previous, settings: { ...previous.settings, ...patch } } : previous);
  }, [meetingId]);
  const save = useCallback(async () => {
    if (!ready) throw Error('会议设置尚未加载，不能生成纪要');
    const snapshot = { ...settings }; setSaving(true); setError('');
    try {
      await invoke('meeting_summary_settings_save', { meetingId, settings: snapshot });
      if (currentId.current !== meetingId) throw Error('会议已切换，未启动生成');
      setState(previous => previous.id === meetingId ? { ...previous, saved: snapshot } : previous);
      return snapshot;
    } catch (e) { if (currentId.current === meetingId) setError(String(e)); throw e; }
    finally { if (currentId.current === meetingId) setSaving(false); }
  }, [meetingId, ready, settings]);
  return { settings, ready, update, save, saving, error, dirty: ready && JSON.stringify(settings) !== JSON.stringify(state.saved) };
}
