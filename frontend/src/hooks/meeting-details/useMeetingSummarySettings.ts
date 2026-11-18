import { useEffect, useRef, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
export type AttendanceMetadata = { participantsManual: boolean; absentManual: boolean; evidence: {name:string;status:string;quote:string}[]; conflicts: string[] };
export type MeetingSummarySettings = { templateId: string; customPrompt: string; participants: string; absent: string; host: string; attendance?:AttendanceMetadata|null };
const defaults: MeetingSummarySettings = { templateId: 'standard_meeting', customPrompt: '', participants: '', absent: '', host: '' };
export function useMeetingSummarySettings(meetingId: string) {
  const [state, setState] = useState({ id: '', settings: defaults, saved: defaults });
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const writes = useRef<Promise<unknown>>(Promise.resolve());
  const pending = useRef<{ key: string; promise: Promise<MeetingSummarySettings> } | null>(null);
  const saveSequence = useRef(0);
  const currentId = useRef(meetingId); currentId.current = meetingId;
  const ready = state.id === meetingId;
  const settings = ready ? state.settings : defaults;
  useEffect(() => {
    let active = true; currentId.current = meetingId; setError(''); setSaving(false);
    invoke<MeetingSummarySettings>('meeting_summary_settings_read', { meetingId }).then(settings => {
      if (active) setState({ id: meetingId, settings, saved: settings });
    }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; if (currentId.current === meetingId) currentId.current = ''; };
  }, [meetingId]);
  const update = useCallback((patch: Partial<MeetingSummarySettings>) => {
    setState(previous => {
      if(previous.id!==meetingId)return previous;
      const settings={...previous.settings,...patch};
      if('participants' in patch||'absent' in patch){settings.attendance={participantsManual:false,absentManual:false,evidence:[],conflicts:[],...previous.settings.attendance,...('participants' in patch?{participantsManual:true}:{}),...('absent' in patch?{absentManual:true}:{})};}
      return {...previous,settings};
    });
  }, [meetingId]);
  const refresh=useCallback(async()=>{
    const loaded=await invoke<MeetingSummarySettings>('meeting_summary_settings_read',{meetingId});
    if(currentId.current!==meetingId)return;
    setState(previous=>{
      if(previous.id!==meetingId)return previous;
      const merged={...loaded};
      for(const key of ['templateId','customPrompt','participants','absent','host'] as const){if(previous.settings[key]!==previous.saved[key])merged[key]=previous.settings[key];}
      if(previous.settings.attendance?.participantsManual)merged.participants=previous.settings.participants;
      if(previous.settings.attendance?.absentManual)merged.absent=previous.settings.absent;
      if(previous.settings.attendance?.participantsManual||previous.settings.attendance?.absentManual){merged.attendance={evidence:[],conflicts:[],...loaded.attendance,participantsManual:!!(loaded.attendance?.participantsManual||previous.settings.attendance?.participantsManual),absentManual:!!(loaded.attendance?.absentManual||previous.settings.attendance?.absentManual)};}
      return {id:meetingId,settings:merged,saved:loaded};
    });
  },[meetingId]);
  const save = useCallback((): Promise<MeetingSummarySettings> => {
    if (!ready) return Promise.reject(Error('会议设置尚未加载，不能保存或生成纪要'));
    const snapshot = { ...settings };
    const key = JSON.stringify([meetingId, snapshot]);
    // Blur and a subsequent save/generate click can request the same snapshot.
    if (pending.current?.key === key) return pending.current.promise;
    const sequence = ++saveSequence.current;
    setSaving(true); setError('');
    // Keep writes ordered so a slow earlier save cannot overwrite a newer edit.
    const result: Promise<MeetingSummarySettings> = writes.current.then(async () => {
      try {
        await invoke('meeting_summary_settings_save', { meetingId, settings: snapshot });
        if (currentId.current !== meetingId) throw Error('会议已切换，未启动生成');
        setState(previous => previous.id === meetingId ? { ...previous, saved: snapshot } : previous);
        if (sequence === saveSequence.current) setError('');
        return snapshot;
      } catch (e) {
        if (currentId.current === meetingId && sequence === saveSequence.current) setError(String(e));
        throw e;
      } finally {
        if (pending.current?.promise === result) pending.current = null;
        if (currentId.current === meetingId && sequence === saveSequence.current) setSaving(false);
      }
    });
    writes.current = result.catch(() => {});
    pending.current = { key, promise: result };
    return result;
  }, [meetingId, ready, settings]);
  return { settings, ready, update, save, refresh, saving, error, dirty: ready && JSON.stringify(settings) !== JSON.stringify(state.saved) };
}
