'use client';

import React, { useEffect, useState } from 'react';
import { CheckCircle2, RotateCcw, ShieldCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import type { MossCandidateReview } from '../types';

type Confirmation = 'activate' | 'rollback' | null;

export function MossActivationPanel({
  review,
  pendingKeys,
  onActivate,
  onRollback,
}: {
  review: MossCandidateReview;
  pendingKeys: ReadonlySet<string>;
  onActivate: () => Promise<unknown>;
  onRollback: () => Promise<unknown>;
}) {
  const { t } = useTranslation('moss');
  const [confirmation, setConfirmation] = useState<Confirmation>(null);
  const isActive = review.activation.activeRunId === review.candidate.runId;

  useEffect(() => setConfirmation(null), [review.candidate.runId, isActive]);

  const confirm = async () => {
    if (!confirmation) return;
    const result = confirmation === 'activate' ? await onActivate() : await onRollback();
    if (result) setConfirmation(null);
  };

  return (
    <section className="rounded-lg border border-gray-200 bg-white p-4" aria-labelledby="moss-activation-heading">
      <h3 id="moss-activation-heading" className="flex items-center gap-2 font-semibold text-gray-900">
        <ShieldCheck className="h-4 w-4 text-blue-600" aria-hidden="true" />
        {t('activation.title')}
      </h3>

      {isActive ? (
        <p className="mt-2 flex items-center gap-2 text-sm text-emerald-800">
          <CheckCircle2 className="h-4 w-4" aria-hidden="true" />
          {t('activation.active')}
        </p>
      ) : review.activation.activateBlocker ? (
        <p className="mt-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="alert">
          {t(`activation.blockers.${review.activation.activateBlocker}`)}
        </p>
      ) : (
        <p className="mt-2 text-sm text-gray-600">{t('activation.ready')}</p>
      )}

      {confirmation && (
        <div
          className="mt-3 rounded-md border border-blue-200 bg-blue-50 p-3"
          role="group"
          aria-labelledby="moss-activation-confirmation"
        >
          <p id="moss-activation-confirmation" className="text-sm text-blue-950" aria-live="polite">
            {t(confirmation === 'activate' ? 'activation.confirmActivate' : 'activation.confirmRollback')}
          </p>
          <div className="mt-3 flex flex-wrap justify-end gap-2">
            <Button type="button" size="sm" variant="outline" onClick={() => setConfirmation(null)}>
              {t('actions.keepReviewing')}
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={() => void confirm()}
              disabled={pendingKeys.has(confirmation === 'activate' ? 'activate-candidate' : 'rollback-activation')}
            >
              {t(confirmation === 'activate' ? 'actions.confirmActivate' : 'actions.confirmRollback')}
            </Button>
          </div>
        </div>
      )}

      {!confirmation && (
        <div className="mt-3 flex justify-end">
          {isActive ? (
            <Button
              type="button"
              variant="outline"
              disabled={!review.activation.canRollback}
              onClick={() => setConfirmation('rollback')}
            >
              <RotateCcw className="h-4 w-4" aria-hidden="true" />
              {t('actions.rollback')}
            </Button>
          ) : (
            <Button
              type="button"
              disabled={!review.activation.canActivate}
              onClick={() => setConfirmation('activate')}
            >
              <ShieldCheck className="h-4 w-4" aria-hidden="true" />
              {t('actions.activate')}
            </Button>
          )}
        </div>
      )}
    </section>
  );
}
