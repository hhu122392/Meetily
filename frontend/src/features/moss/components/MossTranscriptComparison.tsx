'use client';

import React, { useEffect, useMemo, useState } from 'react';
import { Check, Pencil, Save } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';
import type { MossCandidateAlignment, MossCandidateReview, MossTranscriptSegment } from '../types';
import { formatMossTimestamp, shortMossHash } from '../utils';

function speakerName(
  segment: MossTranscriptSegment,
  participantNames: ReadonlyMap<string, string>,
  anonymousLabel: string,
): string {
  if (segment.resolvedPersonId) {
    return participantNames.get(segment.resolvedPersonId) ?? segment.speakerLabel ?? anonymousLabel;
  }
  return segment.speakerLabel ?? anonymousLabel;
}

function ReadonlySegment({
  segment,
  participantNames,
  anonymousLabel,
}: {
  segment: MossTranscriptSegment;
  participantNames: ReadonlyMap<string, string>;
  anonymousLabel: string;
}) {
  return (
    <li className="rounded-md border border-gray-200 bg-white p-3">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-gray-500">
        <span className="font-medium text-gray-800">
          {speakerName(segment, participantNames, anonymousLabel)}
        </span>
        <span>{formatMossTimestamp(segment.startMs)}–{formatMossTimestamp(segment.endMs)}</span>
      </div>
      <p className="mt-2 whitespace-pre-wrap text-sm leading-6 text-gray-900">{segment.text}</p>
    </li>
  );
}

function CandidateSegmentEditor({
  segment,
  alignment,
  participantNames,
  anonymousLabel,
  pending,
  locked,
  onSave,
}: {
  segment: MossTranscriptSegment;
  alignment: MossCandidateAlignment;
  participantNames: ReadonlyMap<string, string>;
  anonymousLabel: string;
  pending: boolean;
  locked: boolean;
  onSave: (text: string) => Promise<unknown>;
}) {
  const { t } = useTranslation('moss');
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(segment.text);
  const time = formatMossTimestamp(segment.startMs);

  useEffect(() => {
    setDraft(segment.text);
    setEditing(false);
  }, [locked, segment.text]);

  const normalizedDraft = draft.trim();
  const dirty = normalizedDraft !== segment.text.trim();
  const canSave = normalizedDraft.length > 0 && dirty && !pending && !locked;

  return (
    <li className="rounded-md border border-blue-200 bg-blue-50/40 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-gray-500">
        <span className="flex items-center gap-2 font-medium text-gray-800">
          {speakerName(segment, participantNames, anonymousLabel)}
          {segment.speakerResolution !== 'anonymous' && <Check className="h-3.5 w-3.5 text-emerald-600" aria-hidden="true" />}
        </span>
        <span>{time}–{formatMossTimestamp(segment.endMs)}</span>
      </div>
      <p
        className="mt-1 text-[11px] text-gray-500"
        title={`${alignment.audioTokenTrackSha256 ?? alignment.sourceTranscriptSha256} · ${alignment.rawTextSha256}`}
      >
        {alignment.alignmentMethod === 'whisper_audio_token'
          ? t('comparison.timeSourceAudioToken', {
            first: alignment.firstAudioTokenIndex,
            last: alignment.lastAudioTokenIndex,
            confidence: Math.round((alignment.confidence ?? 0) * 100),
            hash: shortMossHash(alignment.audioTokenTrackSha256 ?? ''),
          })
          : alignment.alignmentMethod === 'source_transcript_segment'
          ? t('comparison.timeSourceAligned', {
            count: alignment.sourceAnchorIds.length,
            confidence: Math.round((alignment.confidence ?? 0) * 100),
            hash: shortMossHash(alignment.sourceTranscriptSha256),
          })
          : t('comparison.timeSourceRaw', {
            hash: shortMossHash(alignment.sourceTranscriptSha256),
          })}
      </p>
      <p className="mt-0.5 text-[11px] font-medium text-gray-600">
        {t(`comparison.textLayers.${segment.textSourceLayer}`)}
      </p>

      {editing ? (
        <div className="mt-2 space-y-2">
          <Textarea
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            aria-label={t('comparison.editLabel', { time })}
            className="min-h-24 bg-white"
          />
          <div className="flex justify-end gap-2">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onClick={() => {
                setDraft(segment.text);
                setEditing(false);
              }}
              disabled={pending || locked}
            >
              {t('actions.keepReviewing')}
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={async () => {
                const result = await onSave(normalizedDraft);
                if (result) setEditing(false);
              }}
              disabled={!canSave}
              aria-label={t('comparison.saveLabel', { time })}
            >
              <Save className="h-4 w-4" aria-hidden="true" />
              {t('actions.saveEdit')}
            </Button>
          </div>
        </div>
      ) : (
        <div className="mt-2 flex items-start gap-2">
          <p className="min-w-0 flex-1 whitespace-pre-wrap text-sm leading-6 text-gray-900">{segment.text}</p>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            onClick={() => setEditing(true)}
            disabled={locked}
            aria-label={t('comparison.editLabel', { time })}
          >
            <Pencil className="h-4 w-4" aria-hidden="true" />
          </Button>
        </div>
      )}
    </li>
  );
}

export function MossTranscriptComparison({
  review,
  pendingKeys,
  onSaveSegment,
}: {
  review: MossCandidateReview;
  pendingKeys: ReadonlySet<string>;
  onSaveSegment: (segmentId: string, text: string) => Promise<unknown>;
}) {
  const { t } = useTranslation('moss');
  const participantNames = useMemo(
    () => new Map(review.participants.map((participant) => [participant.personId, participant.displayName])),
    [review.participants],
  );
  const sourceLabel = review.current.source === 'whisper'
    ? t('comparison.sourceWhisper')
    : review.current.source === 'moss'
      ? t('comparison.sourceMoss')
      : t('comparison.sourceManual');
  const alignments = useMemo(
    () => new Map(review.candidate.alignments.map((alignment) => [alignment.segmentId, alignment])),
    [review.candidate.alignments],
  );
  const diagnostics = review.candidate.diagnostics;
  const audioTokenAlignment = review.candidate.audioTokenAlignment;

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <section className="min-w-0 rounded-lg border border-gray-200 bg-gray-50 p-3" aria-labelledby="moss-current-heading">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 id="moss-current-heading" className="font-semibold text-gray-900">{t('comparison.current')}</h3>
            <p className="text-xs text-gray-500">{sourceLabel}</p>
          </div>
          <code className="text-xs text-gray-500">{shortMossHash(review.current.sha256)}</code>
        </div>
        {review.current.segments.length === 0 ? (
          <p className="mt-4 text-sm text-gray-600">{t('comparison.empty')}</p>
        ) : (
          <ol className="mt-3 max-h-[50vh] space-y-2 overflow-y-auto pr-1">
            {review.current.segments.map((segment) => (
              <ReadonlySegment
                key={segment.segmentId}
                segment={segment}
                participantNames={participantNames}
                anonymousLabel={t('speakers.anonymous')}
              />
            ))}
          </ol>
        )}
      </section>

      <section className="min-w-0 rounded-lg border border-blue-200 bg-blue-50/30 p-3" aria-labelledby="moss-candidate-heading">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <h3 id="moss-candidate-heading" className="font-semibold text-gray-900">{t('comparison.candidate')}</h3>
            {review.candidate.isActive && (
              <span className="rounded-full bg-emerald-100 px-2 py-0.5 text-xs font-medium text-emerald-800">
                {t('comparison.activeBadge')}
              </span>
            )}
          </div>
          <code className="text-xs text-gray-500">{shortMossHash(review.candidate.sha256)}</code>
        </div>
        {review.candidate.isActive && (
          <p className="mt-2 text-xs text-gray-600" role="status">
            {t('comparison.activeLocked')}
          </p>
        )}
        {diagnostics && (
          <div className="mt-2 rounded-md border border-blue-100 bg-white/70 px-2 py-1.5 text-[11px] leading-5 text-gray-600">
            <p>
              {t('comparison.alignmentAudit', {
                aligned: diagnostics.alignedSegmentCount,
                fallback: diagnostics.fallbackSegmentCount,
                hash: shortMossHash(diagnostics.sourceExpectedSha256),
              })}
            </p>
            <p>
              {diagnostics.lastActiveMs === null
                ? t('comparison.activityAuditSilent', {
                  modelLast: formatMossTimestamp(diagnostics.modelLastTimestampMs),
                })
                : t('comparison.activityAudit', {
                  lastActive: formatMossTimestamp(diagnostics.lastActiveMs),
                  modelLast: formatMossTimestamp(diagnostics.modelLastTimestampMs),
                  delta: diagnostics.tailDeltaMs ?? 0,
                })}
            </p>
          </div>
        )}
        {audioTokenAlignment && (
          <div className="mt-2 rounded-md border border-purple-100 bg-purple-50/70 px-2 py-1.5 text-[11px] leading-5 text-gray-700">
            <p>
              {audioTokenAlignment.status === 'verified'
                ? t('comparison.audioTokenAudit', {
                  aligned: audioTokenAlignment.tokenAlignedSegmentCount,
                  fallback: audioTokenAlignment.fallbackRawSegmentCount,
                  coverage: Math.round((audioTokenAlignment.globalMatchCoverage ?? 0) * 100),
                  hash: shortMossHash(audioTokenAlignment.tokenTrackSha256 ?? ''),
                })
                : t('comparison.audioTokenFallback', {
                  reason: audioTokenAlignment.fallbackReason,
                })}
            </p>
          </div>
        )}
        {review.candidate.segments.length === 0 ? (
          <p className="mt-4 text-sm text-gray-600">{t('comparison.empty')}</p>
        ) : (
          <ol className="mt-3 max-h-[50vh] space-y-2 overflow-y-auto pr-1">
            {review.candidate.segments.map((segment) => {
              const alignment = alignments.get(segment.segmentId);
              return alignment ? (
                <CandidateSegmentEditor
                  key={segment.segmentId}
                  segment={segment}
                  alignment={alignment}
                  participantNames={participantNames}
                  anonymousLabel={t('speakers.anonymous')}
                  pending={pendingKeys.has(`candidate-segment:${segment.segmentId}`)}
                  locked={review.candidate.isActive}
                  onSave={(text) => onSaveSegment(segment.segmentId, text)}
                />
              ) : null;
            })}
          </ol>
        )}
      </section>
    </div>
  );
}
