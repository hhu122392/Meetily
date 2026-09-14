'use client';

import { useMemo, useState } from 'react';
import { ChevronDown, Loader2, RotateCcw, Check } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '../ui/button';
import type {
  TextSpanDifference,
  TranscriptDiffItem,
  TranscriptDiffKind,
  TranscriptDiffResponse,
} from '@/lib/transcript-revision';

interface TranscriptRevisionPanelProps {
  diff: TranscriptDiffResponse;
  isRestoring: boolean;
  onRestore: () => void;
  onKeep: () => void;
  /** 没有可回退的备份（例如"按术语核对"没改动、没产生备份）时把恢复按钮禁掉 */
  canRestore?: boolean;
}

const KIND_STYLES: Record<TranscriptDiffKind, string> = {
  changed: 'bg-amber-100 text-amber-900 border-amber-200',
  added: 'bg-blue-100 text-blue-900 border-blue-200',
  removed: 'bg-red-100 text-red-900 border-red-200',
  unchanged: 'bg-gray-100 text-gray-600 border-gray-200',
};

const KIND_LABEL_KEYS: Record<
  TranscriptDiffKind,
  | 'enhancementReview.kindChanged'
  | 'enhancementReview.kindAdded'
  | 'enhancementReview.kindRemoved'
  | 'enhancementReview.kindUnchanged'
> = {
  changed: 'enhancementReview.kindChanged',
  added: 'enhancementReview.kindAdded',
  removed: 'enhancementReview.kindRemoved',
  unchanged: 'enhancementReview.kindUnchanged',
};

/**
 * 把后端给的"字符区间"渲染成高亮文本。
 * 注意：Rust 侧的偏移按 Unicode 标量（字符）算，这里也用 Array.from 切，避免中文/emoji 错位。
 */
function HighlightedText({
  text,
  spans,
  side,
}: {
  text: string;
  spans: TextSpanDifference[];
  side: 'before' | 'after';
}) {
  const characters = useMemo(() => Array.from(text), [text]);
  const ranges = useMemo(
    () =>
      spans
        .map((span) =>
          side === 'before'
            ? { start: span.before_start, length: span.before_length }
            : { start: span.after_start, length: span.after_length },
        )
        .filter((range) => range.length > 0)
        .sort((a, b) => a.start - b.start),
    [spans, side],
  );

  if (ranges.length === 0) {
    return <span>{text}</span>;
  }

  const pieces: React.ReactNode[] = [];
  let cursor = 0;
  ranges.forEach((range, index) => {
    if (range.start > cursor) {
      pieces.push(
        <span key={`plain-${index}`}>{characters.slice(cursor, range.start).join('')}</span>,
      );
    }
    pieces.push(
      <mark
        key={`mark-${index}`}
        className={
          side === 'before'
            ? 'rounded bg-red-100 px-0.5 text-red-900'
            : 'rounded bg-green-100 px-0.5 text-green-900'
        }
      >
        {characters.slice(range.start, range.start + range.length).join('')}
      </mark>,
    );
    cursor = Math.max(cursor, range.start + range.length);
  });
  if (cursor < characters.length) {
    pieces.push(<span key="plain-tail">{characters.slice(cursor).join('')}</span>);
  }

  return <span>{pieces}</span>;
}

function formatSeconds(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value)) return '--:--';
  const total = Math.max(0, Math.floor(value));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`;
}

export function TranscriptRevisionPanel({
  diff,
  isRestoring,
  onRestore,
  onKeep,
  canRestore = true,
}: TranscriptRevisionPanelProps) {
  const { t } = useTranslation('transcription');
  const [showUnchanged, setShowUnchanged] = useState(false);

  const visibleItems = useMemo(
    () => diff.items.filter((item) => item.kind !== 'unchanged'),
    [diff.items],
  );
  const unchangedItems = useMemo(
    () => diff.items.filter((item) => item.kind === 'unchanged'),
    [diff.items],
  );

  const renderItem = (item: TranscriptDiffItem, index: number) => (
    <div
      key={`${item.kind}-${item.after_id ?? item.before_id ?? index}`}
      className="rounded-md border border-gray-200 p-3 text-sm"
    >
      <div className="mb-2 flex items-center gap-2 text-xs text-gray-500">
        <span
          className={`rounded border px-1.5 py-0.5 text-[11px] font-medium ${KIND_STYLES[item.kind]}`}
        >
          {t(KIND_LABEL_KEYS[item.kind])}
        </span>
        <span>
          {formatSeconds(item.audio_start_time)} – {formatSeconds(item.audio_end_time)}
        </span>
      </div>
      {item.kind === 'added' ? (
        <p className="leading-relaxed text-gray-900">{item.after_text}</p>
      ) : item.kind === 'removed' ? (
        <p className="leading-relaxed text-gray-500 line-through">{item.before_text}</p>
      ) : (
        <div className="space-y-1.5">
          <p className="leading-relaxed text-gray-500">
            <HighlightedText
              text={item.before_text ?? ''}
              spans={item.differences}
              side="before"
            />
          </p>
          <p className="leading-relaxed text-gray-900">
            <HighlightedText
              text={item.after_text ?? ''}
              spans={item.differences}
              side="after"
            />
          </p>
        </div>
      )}
    </div>
  );

  return (
    <div className="space-y-4 py-2">
      <div className="max-h-[46vh] space-y-2 overflow-y-auto pr-1">
        {visibleItems.length === 0 ? (
          <p className="rounded-md border border-gray-200 bg-white p-3 text-sm text-gray-600">
            {t('enhancementReview.unchangedSummary', { count: diff.summary.unchanged })}
          </p>
        ) : (
          visibleItems.map(renderItem)
        )}

        {unchangedItems.length > 0 && (
          <div className="rounded-md border border-gray-200 bg-white">
            <button
              type="button"
              className="flex w-full items-center justify-between px-3 py-2 text-xs text-gray-600 hover:bg-gray-50"
              onClick={() => setShowUnchanged((value) => !value)}
              aria-expanded={showUnchanged}
            >
              <span>{t('enhancementReview.unchangedSummary', { count: unchangedItems.length })}</span>
              <ChevronDown
                className={`h-4 w-4 transition-transform ${showUnchanged ? 'rotate-180' : ''}`}
                aria-hidden="true"
              />
            </button>
            {showUnchanged && (
              <div className="space-y-2 border-t border-gray-100 p-3">
                {unchangedItems.map((item, index) => (
                  <div key={`unchanged-${item.after_id ?? index}`} className="text-xs text-gray-500">
                    <span className="mr-2">{formatSeconds(item.audio_start_time)}</span>
                    {item.after_text}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
        <Button
          type="button"
          variant="outline"
          onClick={onRestore}
          disabled={isRestoring || !canRestore}
        >
          {isRestoring ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden="true" />
          ) : (
            <RotateCcw className="mr-2 h-4 w-4" aria-hidden="true" />
          )}
          {isRestoring ? t('enhancementReview.restoring') : t('enhancementReview.restoreOriginal')}
        </Button>
        <Button type="button" onClick={onKeep} disabled={isRestoring}>
          <Check className="mr-2 h-4 w-4" aria-hidden="true" />
          {t('enhancementReview.keepNew')}
        </Button>
      </div>
    </div>
  );
}
