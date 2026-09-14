"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptFileSyncNotice } from './TranscriptFileSyncNotice';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;
  /** PRO 版式：转写单独成页时占满整个宽度 */
  fullWidth?: boolean;
  /** PRO 版式：两个视图靠 CSS 隐藏切换，不走卸载 */
  hidden?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  fullWidth = false,
  hidden = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptPanelProps) {
  const { t } = useTranslation('meetings');
  const { t: tTranscription } = useTranslation('transcription');
  const handleUpdateSegment = useCallback(async (segmentId: string, text: string) => {
    if (!meetingId) {
      throw new Error('Meeting ID is required to update a transcript');
    }
    try {
      await invoke('api_update_transcript_segment', {
        meetingId,
        transcriptId: segmentId,
        text,
      });
      await onRefetchTranscripts?.();
      toast.success(tTranscription('messages.transcriptSegmentUpdated'));
    } catch (error) {
      console.error('Failed to update transcript segment:', error);
      toast.error(tTranscription('errors.failedToUpdateTranscriptSegment'));
      throw error;
    }
  }, [meetingId, onRefetchTranscripts, tTranscription]);
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      confidence: t.confidence,
    }));
  }, [transcripts, usePagination, segments]);

  return (
    <div
      className={
        hidden
          ? 'hidden'
          : fullWidth
            ? 'flex w-full min-w-0 bg-white flex-col relative'
            : 'hidden md:flex md:w-1/4 lg:w-1/3 min-w-0 border-r border-gray-200 bg-white flex-col relative shrink-0'
      }
    >
      {/* Title area */}
      <div
        className={
          fullWidth
            ? 'border-b border-gray-200 px-4 pb-3 pt-16'
            : 'p-4 border-b border-gray-200'
        }
      >
        <div className={fullWidth ? 'mx-auto w-full max-w-3xl' : ''}>
          <TranscriptButtonGroup
            transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
            onCopyTranscript={onCopyTranscript}
            onOpenMeetingFolder={onOpenMeetingFolder}
            meetingId={meetingId}
            meetingFolderPath={meetingFolderPath}
            onRefetchTranscripts={onRefetchTranscripts}
            align={fullWidth ? 'end' : 'center'}
          />
        </div>
      </div>

      {meetingId && <TranscriptFileSyncNotice meetingId={meetingId} revision={convertedSegments} />}

      {/* Transcript content - use virtualized view for better performance */}
      <div
        className={
          fullWidth
            ? 'mx-auto w-full max-w-3xl flex-1 overflow-hidden pb-4'
            : 'flex-1 overflow-hidden pb-4'
        }
      >
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          emptyStateVariant="noTranscript"
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          onUpdateSegment={meetingId ? handleUpdateSegment : undefined}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className={fullWidth ? 'mx-auto w-full max-w-3xl border-t border-gray-200 p-1' : 'p-1 border-t border-gray-200'}>
          <textarea
            placeholder={t('descriptions.addSummaryContext')}
            aria-label={t('descriptions.addSummaryContext')}
            className="w-full px-3 py-2 border border-gray-200 rounded-md text-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500 bg-white shadow-sm min-h-[80px] resize-y"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
    </div>
  );
}
