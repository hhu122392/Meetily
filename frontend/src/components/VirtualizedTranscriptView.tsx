'use client';

import { useCallback, useRef, useReducer, startTransition, useEffect, useState, memo } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { RecordingInputMonitor } from "./RecordingInputMonitor";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { useTranslation } from "react-i18next";
import { Check, LoaderCircle, Pencil, X } from "lucide-react";

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;
    /** P0-2: 无转写时的空态文案。'noTranscript' 用于已选中会议的详情页 */
    emptyStateVariant?: 'welcome' | 'noTranscript';

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;
    /** Persist a manual correction for a finalized segment. */
    onUpdateSegment?: (segmentId: string, text: string) => Promise<void>;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

// Memoized transcript segment component
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    text,
    confidence,
    isStreaming,
    isPartial = false,
    showConfidence,
    onUpdateSegment,
}: {
    id: string;
    timestamp: number;
    text: string;
    confidence?: number;
    isStreaming: boolean;
    isPartial?: boolean;
    showConfidence: boolean;
    onUpdateSegment?: (segmentId: string, text: string) => Promise<void>;
}) {
    const { t } = useTranslation('transcription');
    const displayText = text.trim() === '' ? t('labels.silence') : text;
    const [isEditing, setIsEditing] = useState(false);
    const [draftText, setDraftText] = useState(text);
    const [isSaving, setIsSaving] = useState(false);

    useEffect(() => {
        if (!isEditing) setDraftText(text);
    }, [text, isEditing]);

    const cancelEditing = useCallback(() => {
        if (isSaving) return;
        setDraftText(text);
        setIsEditing(false);
    }, [isSaving, text]);

    const saveEditing = useCallback(async () => {
        const normalized = draftText.trim();
        if (!onUpdateSegment || !normalized || normalized === text) {
            if (normalized === text) setIsEditing(false);
            return;
        }
        setIsSaving(true);
        try {
            await onUpdateSegment(id, normalized);
            setDraftText(normalized);
            setIsEditing(false);
        } finally {
            setIsSaving(false);
        }
    }, [draftText, id, onUpdateSegment, text]);

    return (
        <div id={`segment-${id}`} data-transcript-id={id} data-partial={isPartial} aria-busy={isPartial || undefined} className="mb-3 group/segment">
            <div className="flex items-start gap-2">
                <Tooltip>
                    <TooltipTrigger>
                        <span className="text-xs text-gray-400 mt-1 flex-shrink-0 min-w-[50px]">
                            {formatRecordingTime(timestamp)}
                        </span>
                    </TooltipTrigger>
                    <TooltipContent>
                        {confidence !== undefined && showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                <div className="flex-1">
                    {isEditing ? (
                        <div className="rounded-lg border border-blue-300 bg-blue-50 p-2">
                            <textarea
                                value={draftText}
                                onChange={(event) => setDraftText(event.target.value)}
                                onKeyDown={(event) => {
                                    event.stopPropagation();
                                    if (event.key === 'Escape') {
                                        event.preventDefault();
                                        cancelEditing();
                                    } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                                        event.preventDefault();
                                        void saveEditing();
                                    }
                                }}
                                onClick={(event) => event.stopPropagation()}
                                aria-label={t('accessibility.editTranscriptSegment')}
                                className="min-h-24 w-full resize-y rounded-md border border-blue-200 bg-white px-3 py-2 text-base leading-relaxed text-gray-800 focus:outline-none focus:ring-2 focus:ring-blue-500"
                                disabled={isSaving}
                                autoFocus
                            />
                            <div className="mt-2 flex justify-end gap-2">
                                <button
                                    type="button"
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        cancelEditing();
                                    }}
                                    aria-label={t('actions.cancelTranscriptEdit')}
                                    className="inline-flex items-center gap-1 rounded-md border border-gray-300 bg-white px-3 py-1.5 text-sm hover:bg-gray-50 disabled:opacity-50"
                                    disabled={isSaving}
                                >
                                    <X size={16} />
                                    {t('actions.cancel')}
                                </button>
                                <button
                                    type="button"
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        void saveEditing();
                                    }}
                                    aria-label={t('actions.saveTranscriptEdit')}
                                    className="inline-flex items-center gap-1 rounded-md bg-blue-600 px-3 py-1.5 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
                                    disabled={isSaving || !draftText.trim() || draftText.trim() === text}
                                >
                                    {isSaving ? <LoaderCircle className="animate-spin" size={16} /> : <Check size={16} />}
                                    {t('actions.saveTranscriptEdit')}
                                </button>
                            </div>
                        </div>
                    ) : isStreaming ? (
                        <div className="bg-gray-100 border border-gray-200 rounded-lg px-3 py-2">
                            <p className="text-base text-gray-800 leading-relaxed">{displayText}</p>
                        </div>
                    ) : (
                        <div className="flex items-start gap-1">
                            <p className={`flex-1 text-base leading-relaxed whitespace-pre-wrap ${isPartial ? 'text-gray-500' : 'text-gray-800'}`}>{displayText}</p>
                            {onUpdateSegment && !isPartial && (
                                <button
                                    type="button"
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        setDraftText(text);
                                        setIsEditing(true);
                                    }}
                                    aria-label={t('actions.editTranscriptSegment')}
                                    title={t('actions.editTranscriptSegment')}
                                    className="rounded p-1 text-gray-500 opacity-0 transition-opacity hover:bg-gray-100 hover:text-gray-800 focus:opacity-100 group-hover/segment:opacity-100"
                                >
                                    <Pencil size={15} />
                                </button>
                            )}
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
});

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    emptyStateVariant = 'welcome',
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    onUpdateSegment,
}) => {
    const { t } = useTranslation('transcription');
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 60, // Estimated height per segment
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // Custom hook for auto-scrolling (supports both virtualized and non-virtualized)
    useAutoScroll({
        scrollRef,
        segments,
        isRecording,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
    });

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;

    return (
        <div ref={scrollRef} className="flex flex-col h-full overflow-y-auto px-4 py-2">
            {/* Recording Status Bar - Sticky at top, always visible when recording */}
            <AnimatePresence>
                {isRecording && (
                    <div className="sticky top-0 z-10 shrink-0 bg-white pb-2">
                        <RecordingStatusBar isPaused={isPaused} />
                    </div>
                )}
            </AnimatePresence>

            {/* Content - add padding when recording to prevent overlap */}
            <div className={`shrink-0 ${isRecording ? 'pt-2' : ''}`}>
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-gray-500 mt-8"
                    role="status"
                    aria-live="polite"
                >
                    {isRecording ? (
                        <RecordingInputMonitor isPaused={isPaused} />
                    ) : (
                        emptyStateVariant === 'noTranscript' ? (
                            <>
                                <p className="text-lg font-semibold">{t('labels.noTranscriptForThisMeeting')}</p>
                                <p className="text-xs mt-1">{t('labels.noTranscriptHint')}</p>
                            </>
                        ) : (
                            <>
                                <p className="text-lg font-semibold">{t('labels.welcomeToMeetily')}</p>
                                <p className="text-xs mt-1">{t('actions.startRecordingToSeeLiveTranscription')}</p>
                            </>
                        )
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={(segment.revision ?? 0) > 0 ? undefined : segment.confidence}
                                        isStreaming={isStreaming}
                                        isPartial={segment.is_partial}
                                        showConfidence={showConfidence}
                                        onUpdateSegment={onUpdateSegment}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{t('status.loadingMore')}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {t('status.showingSegments', { visible: loadedCount, total: totalCount })}
                                </span>
                            ) : null}
                        </div>
                    )}

                  {/* Listening indicator when recording */}
                  {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                      <div className="mt-4">
                          <RecordingInputMonitor compact transcriptCount={segments.length} />
                      </div>
                  )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div className="space-y-1">
                        {segments.map((segment) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={(segment.revision ?? 0) > 0 ? undefined : segment.confidence}
                                        isStreaming={isStreaming}
                                        isPartial={segment.is_partial}
                                        showConfidence={showConfidence}
                                        onUpdateSegment={onUpdateSegment}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-gray-500">
                                    <div className="w-4 h-4 border-2 border-gray-300 border-t-gray-600 rounded-full animate-spin" />
                                    <span className="text-sm">{t('status.loadingMore')}</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-gray-400">
                                    {t('status.showingSegments', { visible: loadedCount, total: totalCount })}
                                </span>
                            ) : null}
                        </div>
                    )}

                  {/* Listening indicator when recording */}
                  {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                      <div className="mt-4">
                          <RecordingInputMonitor compact transcriptCount={segments.length} />
                      </div>
                  )}
                </>
            )}
            </div>
        </div>
    );
};
