import { useEffect, type Dispatch, type SetStateAction } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import type { Transcript } from '@/types';

export function useRealtimeTranscripts(
  setTranscripts: Dispatch<SetStateAction<Transcript[]>>,
) {
  useEffect(() => {
    let active = true;
    let folder: string | null = null;
    let revision = 0;
    const cleanups: (() => void)[] = [];
    const register = async <T,>(name: string, handler: (payload: T) => void) => {
      const unlisten = await listen<T>(name, event => { if (active) handler(event.payload); });
      if (active) cleanups.push(unlisten); else unlisten();
    };
    void (async () => {
      await register<{ folder_path?: string }>('recording-started', payload => {
        revision++;
        folder = payload.folder_path || null;
        setTranscripts([]);
      });
      await register('recording-stopped', () => { revision++; folder = null; });
      await register<Transcript & { folder_path: string }>('realtime-transcript', payload => {
        if (payload.folder_path !== folder) return;
        const id = `live-${payload.folder_path}-${payload.sequence_id}`;
        setTranscripts(current => current.some(row => row.id === id) ? current : [
          ...current, { ...payload, id, chunk_start_time: payload.audio_start_time },
        ]);
      });
      await register<{ folder_path: string; message: string }>('realtime-transcription-status', payload => {
        if (payload.folder_path === folder) toast.warning(payload.message);
      });
      const requestedRevision = revision;
      const currentFolder = await invoke<string | null>('get_meeting_folder_path');
      if (active && revision === requestedRevision) folder = currentFolder;
    })().catch(error => { if (active) console.error('实时转写监听失败', error); });
    return () => { active = false; cleanups.forEach(cleanup => cleanup()); };
  }, [setTranscripts]);
}
