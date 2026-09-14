'use client';
import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { HelpHint } from '@/components/ui/help-hint';
import { Button } from '@/components/ui/button';

export function TranscriptFileSyncNotice({ meetingId, revision }: { meetingId: string; revision?: unknown }) {
  const { t } = useTranslation('transcription');
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    try {
      setPending(await invoke<boolean>('api_transcript_file_sync_pending', { meetingId }));
      setFailed(false);
    } catch (error) { console.warn('Could not check transcript file synchronization', error); setFailed(true); }
  }, [meetingId]);
  useEffect(() => {
    void refresh();
    const listener = () => { void refresh(); };
    window.addEventListener('transcript-files-changed', listener);
    return () => window.removeEventListener('transcript-files-changed', listener);
  }, [refresh, revision]);
  if (!pending && !failed) return null;
  return <div className="flex flex-wrap items-center gap-2 py-1 text-xs text-amber-800" role="status">
    <span>{t(pending ? 'fileSync.pending' : 'fileSync.checkFailed')}</span>
    <HelpHint text={t('fileSync.help')} />
    <Button type="button" size="sm" variant="outline" disabled={busy} onClick={async () => {
      setBusy(true);
      try {
        if (pending) await invoke('api_retry_transcript_file_sync', { meetingId });
        await refresh();
        window.dispatchEvent(new Event('transcript-files-changed'));
      } catch (error) { console.warn('Transcript file synchronization retry failed', error); setFailed(true); }
      finally { setBusy(false); }
    }}>{t(busy ? 'fileSync.retrying' : 'fileSync.retry')}</Button>
  </div>;
}
