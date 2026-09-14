'use client';

import { Transcript } from '@/types';
import { useEffect, useRef, useState } from 'react';
import { ConfidenceIndicator } from './ConfidenceIndicator';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/tooltip';
import { RecordingStatusBar } from './RecordingStatusBar';
import { motion, AnimatePresence } from 'framer-motion';
import { useTranslation } from 'react-i18next';

interface TranscriptViewProps {
  transcripts: Transcript[];
  isRecording?: boolean;
  isPaused?: boolean; // Is recording paused (affects UI indicators)
  isProcessing?: boolean; // Is processing/finalizing transcription (hides "Listening..." indicator)
  isStopping?: boolean; // Is recording being stopped (provides immediate UI feedback)
  enableStreaming?: boolean; // Enable streaming effect for live transcription UX
}

interface SpeechDetectedEvent {
  message: string;
}

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
  if (seconds === undefined) return '[--:--]';

  const totalSeconds = Math.floor(seconds);
  const minutes = Math.floor(totalSeconds / 60);
  const secs = totalSeconds % 60;

  return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

export const TranscriptView: React.FC<TranscriptViewProps> = ({ transcripts, isRecording = false, isPaused = false, isProcessing = false, isStopping = false, enableStreaming = false }) => {
  // P0-1: 实时字幕按 20/25 秒分片输出，用户在前 20 秒看不到任何进展。
  // 这里用一个本地计时器把"等待时长"显式展示出来（不依赖录制页/父组件状态）。
  const [recordingElapsedSeconds, setRecordingElapsedSeconds] = useState(0);

  useEffect(() => {
    if (!isRecording) {
      setRecordingElapsedSeconds(0);
      return;
    }
    if (isPaused) return;
    const timer = window.setInterval(
      () => setRecordingElapsedSeconds((prev) => prev + 1),
      1000,
    );
    return () => window.clearInterval(timer);
  }, [isRecording, isPaused]);
  const { t } = useTranslation('transcription');
  const [speechDetected, setSpeechDetected] = useState(false);

  // Debug: Log the props to understand what's happening
  console.log('TranscriptView render:', {
    isRecording,
    isPaused,
    isProcessing,
    isStopping,
    transcriptCount: transcripts.length,
    shouldShowListening: !isStopping && isRecording && !isPaused && !isProcessing && transcripts.length > 0
  });

  // Streaming effect state
  const [streamingTranscript, setStreamingTranscript] = useState<{
    id: string;
    visibleText: string;
    fullText: string;
  } | null>(null);
  const streamingIntervalRef = useRef<NodeJS.Timeout | null>(null);
  const lastStreamedIdRef = useRef<string | null>(null); // Track which transcript we've streamed

  // Load preference for showing confidence indicator
  const [showConfidence, setShowConfidence] = useState<boolean>(() => {
    if (typeof window !== 'undefined') {
      const saved = localStorage.getItem('showConfidenceIndicator');
      return saved !== null ? saved === 'true' : true; // Default to true
    }
    return true;
  });

  // Listen for preference changes from settings
  useEffect(() => {
    const handleConfidenceChange = (e: Event) => {
      const customEvent = e as CustomEvent<boolean>;
      setShowConfidence(customEvent.detail);
    };

    window.addEventListener('confidenceIndicatorChanged', handleConfidenceChange);
    return () => window.removeEventListener('confidenceIndicatorChanged', handleConfidenceChange);
  }, []);

  // Listen for speech-detected event
  useEffect(() => {
    let unsubscribe: (() => void) | undefined;

    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      unsubscribe = await listen<SpeechDetectedEvent>('speech-detected', () => {
        setSpeechDetected(true);
      });
    };

    if (isRecording) {
      setupListener();
    } else {
      // Reset when not recording
      setSpeechDetected(false);
    }

    return () => {
      if (unsubscribe) {
        unsubscribe();
      }
    };
  }, [isRecording]);

  // Streaming effect: animate new transcripts character-by-character
  useEffect(() => {
    if (!enableStreaming || !isRecording) {
      // Clean up if streaming is disabled
      if (streamingIntervalRef.current) {
        clearInterval(streamingIntervalRef.current);
        streamingIntervalRef.current = null;
      }
      setStreamingTranscript(null);
      lastStreamedIdRef.current = null;
      return;
    }

    // Find the latest non-partial transcript
    const latestTranscript = transcripts
      .slice(-1)[0];

    if (!latestTranscript) return;

    if ((latestTranscript.revision ?? 0) > 0) {
      if (streamingIntervalRef.current) clearInterval(streamingIntervalRef.current);
      streamingIntervalRef.current = null;
      setStreamingTranscript(null);
      lastStreamedIdRef.current = latestTranscript.id;
      return;
    }

    // Check if this is a new transcript we haven't streamed yet (using ref to avoid dependency issues)
    if (lastStreamedIdRef.current !== latestTranscript.id) {
      // Clear any existing streaming interval
      if (streamingIntervalRef.current) {
        clearInterval(streamingIntervalRef.current);
        streamingIntervalRef.current = null;
      }

      // Mark this transcript as being streamed
      lastStreamedIdRef.current = latestTranscript.id;

      const fullText = latestTranscript.text;

      // Fast typewriter effect - complete in 0.8 seconds for snappy feel
      const TOTAL_DURATION_MS = 800; // 0.8 seconds total - fast and snappy!
      const INTERVAL_MS = 15; // Update every 15ms for smooth animation
      const totalTicks = TOTAL_DURATION_MS / INTERVAL_MS; // ~53 ticks
      const charsPerTick = Math.max(2, Math.ceil(fullText.length / totalTicks)); // At least 2 chars per tick for speed
      const INITIAL_CHARS = Math.min(5, fullText.length); // Start with first 5 chars visible
      let charIndex = INITIAL_CHARS;

      setStreamingTranscript({
        id: latestTranscript.id,
        visibleText: fullText.substring(0, INITIAL_CHARS),
        fullText: fullText
      });

      streamingIntervalRef.current = setInterval(() => {
        charIndex += charsPerTick;

        if (charIndex >= fullText.length) {
          // Streaming complete
          clearInterval(streamingIntervalRef.current!);
          streamingIntervalRef.current = null;
          setStreamingTranscript(null);
        } else {
          setStreamingTranscript(prev => {
            if (!prev) return null;
            return {
              ...prev,
              visibleText: fullText.substring(0, charIndex)
            };
          });
        }
      }, INTERVAL_MS);
    }
  }, [transcripts, enableStreaming, isRecording]);

  // Cleanup streaming interval on unmount
  useEffect(() => {
    return () => {
      if (streamingIntervalRef.current) {
        clearInterval(streamingIntervalRef.current);
        streamingIntervalRef.current = null;
      }
      lastStreamedIdRef.current = null;
    };
  }, []);

  return (
    <div className="px-4 py-2">
      {/* Recording Status Bar - Sticky at top, always visible when recording */}
      <AnimatePresence>
        {isRecording && (
          <div className="sticky top-4 z-10 bg-white pb-2">
            <RecordingStatusBar isPaused={isPaused} />
          </div>
        )}
      </AnimatePresence>

      {transcripts?.map((transcript, index) => {
        const isStreaming = (transcript.revision ?? 0) === 0 && streamingTranscript?.id === transcript.id
          && streamingTranscript.fullText === transcript.text;
        const textToShow = isStreaming ? streamingTranscript.visibleText : transcript.text;
        const filteredText = textToShow;
        // Show [Silence] ONLY if the ORIGINAL transcript was empty (not just after filtering)
        const originalWasEmpty = transcript.text.trim() === '';
        const displayText = originalWasEmpty && !isStreaming ? t('labels.silence') : filteredText;

        // Sizer text: use cleaned version for proper sizing, fallback to [Silence] only if original was empty
        const sizerText = (isStreaming ? streamingTranscript.fullText : transcript.text)
          || (originalWasEmpty && !isStreaming ? t('labels.silence') : '');

        return (
          <motion.div
            key={transcript.id ? `${transcript.id}-${index}` : `transcript-${index}`}
            initial={{ opacity: 0, y: 5 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.15 }}
            className="mb-3"
            data-transcript-id={transcript.sequence_id}
            data-revision={transcript.revision ?? 0}
            aria-busy={transcript.is_partial || undefined}
          >
            <div className="flex items-start gap-2">
              <Tooltip>
                <TooltipTrigger>
                  <span className="text-xs text-gray-400 mt-1 flex-shrink-0 min-w-[50px]">
                    {transcript.audio_start_time !== undefined
                      ? formatRecordingTime(transcript.audio_start_time)
                      : transcript.timestamp}
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  {transcript.duration !== undefined && (
                    <span className="text-xs text-gray-400">
                      {t('labels.durationSeconds', { duration: transcript.duration.toFixed(1) })}
                      {transcript.confidence !== undefined && (transcript.revision ?? 0) === 0 && (
                        <ConfidenceIndicator
                          confidence={transcript.confidence}
                          showIndicator={showConfidence}
                        />
                      )}
                    </span>
                  )}
                </TooltipContent>
              </Tooltip>
              <div className="flex-1">
                {isStreaming ? (
                  // Streaming transcript - show in bubble (full width)
                  <div className="bg-gray-100 border border-gray-200 rounded-lg px-3 py-2">
                    <div className="relative">
                      <p className="text-base text-gray-800 leading-relaxed" style={{ visibility: 'hidden' }}>
                        {sizerText}
                      </p>
                      <p className="text-base text-gray-800 leading-relaxed absolute top-0 left-0">
                        {displayText}
                      </p>
                    </div>
                  </div>
                ) : (
                  // Regular transcript - simple text
                  <div className="relative">
                    <p className="text-base text-gray-800 leading-relaxed" style={{ visibility: 'hidden' }}>
                      {sizerText}
                    </p>
                    <p className={`text-base leading-relaxed absolute top-0 left-0 ${transcript.is_partial ? 'text-gray-500' : 'text-gray-800'}`}>
                      {displayText}
                    </p>
                  </div>
                )}
              </div>
            </div>
          </motion.div>
        );
      })}

      {/* Show listening indicator when recording and has transcripts */}
      {!isStopping && isRecording && !isPaused && !isProcessing && transcripts.length > 0 && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="flex items-center gap-2 mt-4 text-gray-500"
        >
          <div className="w-2 h-2 bg-blue-500 rounded-full animate-pulse"></div>
          <span className="text-sm">
            {t('status.transcribedSegmentsNext', { count: transcripts.length })}
          </span>
        </motion.div>
      )}

      {/* Empty state when no transcripts */}
      {transcripts.length === 0 && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          className="text-center text-gray-500 mt-8"
          role="status"
          aria-live="polite"
        >
          {isRecording ? (
            <>
              <div className="flex items-center justify-center mb-3">
                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-orange-500' : 'bg-blue-500 animate-pulse'}`}></div>
              </div>
              <p className="text-sm text-gray-600">
                {isPaused ? t('status.recordingPaused') : t('status.listeningForSpeech')}
              </p>
              <p className="text-xs mt-1 text-gray-400">
                {isPaused
                  ? t('labels.clickResumeToContinueRecording')
                  : t('labels.speakToSeeLiveTranscription')}
              </p>
              {!isPaused && (
                <>
                  <p className="text-xs mt-2 text-gray-400" role="status" aria-live="polite">
                    {t('labels.waitingElapsed', {
                      time: formatRecordingTime(recordingElapsedSeconds).replace(/^\[|\]$/g, ''),
                    })}
                  </p>
                  <p className="text-xs mt-1 text-gray-400 max-w-xs mx-auto">
                    {t('labels.firstCaptionLatencyHint')}
                  </p>
                </>
              )}
            </>
          ) : (
            <>
              <p className="text-lg font-semibold">{t('labels.welcomeToMeetily')}</p>
              <p className="text-xs mt-1">{t('actions.startRecordingToSeeLiveTranscription')}</p>
            </>
          )}
        </motion.div>
      )}
    </div>
  );
};
