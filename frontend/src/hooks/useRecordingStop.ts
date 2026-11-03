import { useState, useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
const notified = new Set<string>();

// Audio, meeting identity and queue insertion are owned by Rust. A background
// completion must never reset the live session, save its text, or navigate it.
export function useRecordingStop(setIsRecording: (value: boolean) => void, setIsRecordingDisabled: (value: boolean) => void) {
  const state = useRecordingState();
  const { clearTranscripts } = useTranscripts();
  const { refetchMeetings, setIsMeetingActive } = useSidebar();
  const [summaryStatus] = useState<SummaryStatus>('idle');
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<{ meeting_id?: string; background_queued?: boolean; save_error?: string }>('recording-stopped', ({ payload }) => {
      if (disposed || !payload.background_queued) return;
      // Synchronous event boundary: no late async callback can clear a new meeting.
      clearTranscripts();
      setIsMeetingActive(false);
      void refetchMeetings();
      if (payload.meeting_id) void applyPinnedSummaryLanguageToMeeting(payload.meeting_id).catch(() => {});
      if (payload.meeting_id && !notified.has(payload.meeting_id)) {
        notified.add(payload.meeting_id);
        if (notified.size > 100) notified.delete(notified.values().next().value!);
        if (payload.save_error) toast.error(payload.save_error, { duration: 15000 });
        else toast.success('音频已保存并加入后台队列，可以开始下一场录音。');
      }
    }).then(fn => { if (disposed) fn(); else unlisten = fn; });
    return () => { disposed = true; unlisten?.(); };
  }, [clearTranscripts, refetchMeetings, setIsMeetingActive]);
  const handleRecordingStop = useCallback(async (_callApi: boolean) => {
    // The stop command has already persisted the job; no transcript polling here.
    const active = await invoke<boolean>('is_recording');
    if (!active) { setIsRecording(false); setIsRecordingDisabled(false); }
    await refetchMeetings();
  }, [setIsRecording, setIsRecordingDisabled, refetchMeetings]);
  return { handleRecordingStop, isStopping: state.isStopping, isProcessingTranscript: state.isProcessing,
    isSavingTranscript: state.isSaving, summaryStatus,
    setIsStopping: (value: boolean) => { if (value) state.setStatus(RecordingStatus.STOPPING); }
  };
}
