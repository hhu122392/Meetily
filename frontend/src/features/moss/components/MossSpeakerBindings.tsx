'use client';

import React, { useMemo } from 'react';
import { UserRoundCheck, UsersRound } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import type { MossCandidateReview } from '../types';
import { formatMossTimestamp } from '../utils';

const ANONYMOUS_VALUE = '__moss_anonymous__';
const INHERIT_VALUE = '__moss_inherit__';

export function MossSpeakerBindings({
  review,
  pendingKeys,
  onSaveBinding,
  onSaveOverride,
}: {
  review: MossCandidateReview;
  pendingKeys: ReadonlySet<string>;
  onSaveBinding: (speakerLabel: string, personId: string | null) => Promise<unknown>;
  onSaveOverride: (segmentId: string, personId: string | null) => Promise<unknown>;
}) {
  const { t } = useTranslation('moss');
  const bindings = useMemo(
    () => new Map(review.bindings.map((binding) => [binding.speakerLabel, binding.personId])),
    [review.bindings],
  );
  const participantNames = useMemo(
    () => new Map(review.participants.map((participant) => [participant.personId, participant.displayName])),
    [review.participants],
  );
  const speakerLabels = useMemo(() => Array.from(new Set([
    ...review.anonymousSpeakers,
    ...review.bindings.map((binding) => binding.speakerLabel),
    ...review.candidate.segments.flatMap((segment) => segment.speakerLabel ? [segment.speakerLabel] : []),
  ])).sort(), [review.anonymousSpeakers, review.bindings, review.candidate.segments]);

  return (
    <div className="space-y-5">
      <div>
        <h3 className="flex items-center gap-2 font-semibold text-gray-900">
          <UsersRound className="h-4 w-4 text-blue-600" aria-hidden="true" />
          {t('speakers.title')}
        </h3>
        <p className="mt-1 text-sm text-gray-600">{t('speakers.description')}</p>
      </div>

      {review.participants.length === 0 && (
        <p className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="status">
          {t('speakers.noParticipants')}
        </p>
      )}

      <section aria-labelledby="moss-bulk-binding-heading">
        <h4 id="moss-bulk-binding-heading" className="text-sm font-semibold text-gray-800">
          {t('speakers.bulkTitle')}
        </h4>
        <div className="mt-2 grid gap-3 sm:grid-cols-2">
          {speakerLabels.map((speakerLabel) => {
            const value = bindings.get(speakerLabel) ?? ANONYMOUS_VALUE;
            return (
              <label key={speakerLabel} className="rounded-md border border-gray-200 bg-white p-3 text-sm">
                <span className="font-semibold text-gray-900">{speakerLabel}</span>
                <Select
                  value={value}
                  disabled={
                    review.candidate.isActive
                    || pendingKeys.has(`speaker-binding:${speakerLabel}`)
                    || review.participants.length === 0
                  }
                  onValueChange={(personId) => void onSaveBinding(
                    speakerLabel,
                    personId === ANONYMOUS_VALUE ? null : personId,
                  )}
                >
                  <SelectTrigger
                    className="mt-2 w-full bg-white"
                    aria-label={t('speakers.selectPersonForSpeaker', { speaker: speakerLabel })}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={ANONYMOUS_VALUE}>{t('speakers.unbound', { speaker: speakerLabel })}</SelectItem>
                    {review.participants.map((participant) => (
                      <SelectItem key={participant.personId} value={participant.personId}>
                        {participant.displayName}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </label>
            );
          })}
        </div>
      </section>

      <section aria-labelledby="moss-segment-override-heading">
        <h4 id="moss-segment-override-heading" className="flex items-center gap-2 text-sm font-semibold text-gray-800">
          <UserRoundCheck className="h-4 w-4 text-purple-600" aria-hidden="true" />
          {t('speakers.segmentTitle')}
        </h4>
        <div className="mt-2 max-h-[38vh] space-y-2 overflow-y-auto pr-1">
          {review.candidate.segments.map((segment) => {
            const speakerLabel = segment.speakerLabel ?? t('speakers.anonymous');
            const time = formatMossTimestamp(segment.startMs);
            const resolvedName = segment.resolvedPersonId
              ? participantNames.get(segment.resolvedPersonId)
              : null;
            return (
              <div key={segment.segmentId} className="grid gap-3 rounded-md border border-gray-200 bg-white p-3 md:grid-cols-[minmax(0,1fr)_240px] md:items-center">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2 text-xs text-gray-500">
                    <span className="font-medium text-gray-800">{speakerLabel}</span>
                    <span>{time}</span>
                    {resolvedName && <span>{t('speakers.resolvedAs', { name: resolvedName })}</span>}
                  </div>
                  <p className="mt-1 line-clamp-2 text-sm text-gray-700">{segment.text}</p>
                </div>
                <Select
                  value={segment.segmentOverridePersonId ?? INHERIT_VALUE}
                  disabled={
                    review.candidate.isActive
                    || pendingKeys.has(`segment-override:${segment.segmentId}`)
                    || review.participants.length === 0
                  }
                  onValueChange={(personId) => void onSaveOverride(
                    segment.segmentId,
                    personId === INHERIT_VALUE ? null : personId,
                  )}
                >
                  <SelectTrigger
                    className="w-full bg-white"
                    aria-label={t('speakers.selectOverrideForSegment', { speaker: speakerLabel, time })}
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={INHERIT_VALUE}>{t('speakers.inherit')}</SelectItem>
                    {review.participants.map((participant) => (
                      <SelectItem key={participant.personId} value={participant.personId}>
                        {participant.displayName}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
