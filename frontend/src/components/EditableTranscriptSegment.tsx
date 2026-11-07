'use client';
import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { Check, LoaderCircle, Pencil, X } from 'lucide-react';
import { ConfidenceIndicator } from './ConfidenceIndicator';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/tooltip';
import { t, useUiLanguage } from '@/i18n';
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined) return '[--:--]';
    const total = Math.floor(seconds);
    return '[' + Math.floor(total / 60).toString().padStart(2, '0') + ':' + (total % 60).toString().padStart(2, '0') + ']';
}

export const EditableTranscriptSegment = memo(function TranscriptSegment({
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
    onUpdateSegment?: (segmentId: string, text: string, expectedText: string) => Promise<void>;
}) {
    useUiLanguage();
    const displayText = text.trim() === '' ? t('[Silence]') : text;
    const [isEditing, setIsEditing] = useState(false);
    const [draftText, setDraftText] = useState(text);
    const [isSaving, setIsSaving] = useState(false);
    const [sourceText, setSourceText] = useState(text);
    const [error, setError] = useState('');
    const saving = useRef(false);

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
        if (saving.current) return;
        if (!onUpdateSegment || !normalized || normalized === sourceText) {
            if (normalized === sourceText) setIsEditing(false);
            return;
        }
        saving.current = true;
        setError('');
        setIsSaving(true);
        try {
            await onUpdateSegment(id, normalized, sourceText);
            setDraftText(normalized);
            setIsEditing(false);
         } catch (error) {
            setError(String(error).includes('proofread_source_changed') ? t('转写已变化，请取消编辑后重新打开') : t('保存失败，修改内容已保留，请重试'));
        } finally {
            saving.current = false;
            setIsSaving(false);
        }
    }, [draftText, id, onUpdateSegment, sourceText]);

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
                                    if (event.nativeEvent.isComposing) return;
                                    if (event.key === 'Escape') {
                                        event.preventDefault();
                                        cancelEditing();
                                    } else if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
                                        event.preventDefault();
                                        void saveEditing();
                                    }
                                }}
                                onClick={(event) => event.stopPropagation()}
                                aria-label={t('编辑转写内容')}
                                className="min-h-24 w-full resize-y rounded-md border border-blue-200 bg-white px-3 py-2 text-base leading-relaxed text-gray-800 focus:outline-none focus:ring-2 focus:ring-blue-500"
                                disabled={isSaving}
                                autoFocus
                            />
                            {error && <p role="alert" className="mt-2 text-xs text-red-600">{error}</p>}
                            <div className="mt-2 flex justify-end gap-2">
                                <button
                                    type="button"
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        cancelEditing();
                                    }}
                                    aria-label={t('取消编辑转写')}
                                    className="inline-flex items-center gap-1 rounded-md border border-gray-300 bg-white px-3 py-1.5 text-sm hover:bg-gray-50 disabled:opacity-50"
                                    disabled={isSaving}
                                >
                                    <X size={16} />
                                    {t('Cancel')}
                                </button>
                                <button
                                    type="button"
                                    onClick={(event) => {
                                        event.stopPropagation();
                                        void saveEditing();
                                    }}
                                    aria-label={t('保存转写')}
                                    className="inline-flex items-center gap-1 rounded-md bg-blue-600 px-3 py-1.5 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
                                    disabled={isSaving || !draftText.trim() || draftText.trim() === sourceText}
                                >
                                    {isSaving ? <LoaderCircle className="animate-spin" size={16} /> : <Check size={16} />}
                                    {t('保存转写')}
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
                                        setSourceText(text);
                                        setError('');
                                        setIsEditing(true);
                                    }}
                                    aria-label={t('编辑这段转写')}
                                    title={t('编辑这段转写')}
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
