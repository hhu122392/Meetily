'use client';

import React, { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useRecordingStop } from '@/hooks/useRecordingStop';

/**
 * RecordingPostProcessingProvider
 *
 * This provider handles post-processing when recording stops from any source:
 * - Tray menu stop
 * - Global keyboard shortcut
 * - Overlay stop button
 * - Main UI stop button
 *
 * It listens for the 'recording-stop-complete' event from Rust backend
 * and triggers the full post-processing flow (save to database, navigate, analytics)
 * regardless of which page the user is currently on.
 */
export function RecordingPostProcessingProvider({ children }: { children: React.ReactNode }) {
  // No-op functions since the global RecordingStateContext already handles state updates
  // These are only needed for the hook's local component state management
  const setIsRecording = () => { };
  const setIsRecordingDisabled = () => { };

  const {
    handleRecordingStop,
  } = useRecordingStop(setIsRecording, setIsRecordingDisabled);
  const { setStatus } = useRecordingState();
  const callbacks = useRef({ handleRecordingStop, setStatus });
  callbacks.current = { handleRecordingStop, setStatus };

  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    let unlistenNotice: (() => void) | undefined;
    let disposed = false;

    const setupListener = async () => {
      try {
        // Listen for recording-stop-complete event from Rust
        unlistenFn = await listen<boolean>('recording-stop-complete', (event) => {
          console.log('[RecordingPostProcessing] Received recording-stop-complete event:', event.payload);

          // Call the post-processing handler
          // event.payload is the callApi boolean (true for normal stops)
          void callbacks.current.handleRecordingStop(event.payload);
        });
        if (disposed) { unlistenFn(); return; }
        unlistenNotice = await listen<{ message: string; error: boolean }>('lark-auto-recording', ({ payload }) => {
          if (payload.error) {
            toast.error(payload.message, { duration: 10000 });
            void invoke<boolean>('is_recording').then(active => {
              if (!active && !disposed) callbacks.current.setStatus(RecordingStatus.ERROR, payload.message);
            }).catch(() => {});
          } else { toast.info(payload.message, { duration: 6000 }); }
        });
        if (disposed) { unlistenNotice(); return; }
        await invoke('lark_auto_recording_ready', { ready: true });
        if (disposed) await invoke('lark_auto_recording_ready', { ready: false });

        console.log('[RecordingPostProcessing] Event listener set up successfully');
      } catch (error) {
        console.error('[RecordingPostProcessing] Failed to set up event listener:', error);
      }
    };

    setupListener();

    return () => {
      disposed = true;
      unlistenNotice?.();
      void invoke('lark_auto_recording_ready', { ready: false }).catch(() => {});
      if (unlistenFn) {
        console.log('[RecordingPostProcessing] Cleaning up event listener');
        unlistenFn();
      }
    };
  }, []);

  return <>{children}</>;
}
