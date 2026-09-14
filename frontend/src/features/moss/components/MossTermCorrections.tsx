'use client';

import { RotateCcw, WandSparkles } from 'lucide-react';
import React from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import type { MossCandidateReview } from '../types';

export function MossTermCorrections({
  review,
  pendingKeys,
  onSetCorrectionState,
}: {
  review: MossCandidateReview;
  pendingKeys: ReadonlySet<string>;
  onSetCorrectionState: (correctionId: string, applied: boolean) => Promise<unknown>;
}) {
  const { t } = useTranslation('moss');

  return (
    <div className="space-y-4">
      <div>
        <h3 className="flex items-center gap-2 font-semibold text-gray-900">
          <WandSparkles className="h-4 w-4 text-purple-600" aria-hidden="true" />
          {t('terms.title')}
        </h3>
        <p className="mt-1 text-sm text-gray-600">{t('terms.description')}</p>
      </div>

      {review.corrections.length === 0 ? (
        <p className="rounded-md border border-gray-200 bg-gray-50 p-4 text-sm text-gray-600">
          {t('terms.none')}
        </p>
      ) : (
        <ol className="max-h-[48vh] space-y-3 overflow-y-auto pr-1">
          {review.corrections.map((correction) => {
            const applied = correction.state === 'applied';
            return (
              <li key={correction.correctionId} className="rounded-lg border border-gray-200 bg-white p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="grid min-w-0 flex-1 gap-3 sm:grid-cols-2">
                    <div className="rounded-md bg-red-50 p-3">
                      <p className="text-xs font-semibold uppercase tracking-wide text-red-700">{t('terms.before')}</p>
                      <p className="mt-1 whitespace-pre-wrap text-sm text-gray-900">{correction.originalText}</p>
                    </div>
                    <div className="rounded-md bg-emerald-50 p-3">
                      <p className="text-xs font-semibold uppercase tracking-wide text-emerald-700">{t('terms.after')}</p>
                      <p className="mt-1 whitespace-pre-wrap text-sm text-gray-900">{correction.correctedText}</p>
                    </div>
                  </div>
                  <span className={`rounded-full px-2 py-1 text-xs font-medium ${
                    applied ? 'bg-emerald-100 text-emerald-800' : 'bg-gray-100 text-gray-700'
                  }`}>
                    {t(applied ? 'terms.applied' : 'terms.reverted')}
                  </span>
                </div>

                <dl className="mt-3 grid gap-2 text-xs text-gray-600 sm:grid-cols-3">
                  <div><dt className="font-medium">{t('terms.matchedAlias')}</dt><dd>{correction.matchedAlias}</dd></div>
                  <div><dt className="font-medium">{t('terms.canonical')}</dt><dd>{correction.canonical}</dd></div>
                  <div><dt className="font-medium">{t('terms.rule')}</dt><dd className="break-all">{correction.ruleId}</dd></div>
                  <div>
                    <dt className="font-medium">{t('terms.source')}</dt>
                    <dd>{t(`terms.sources.${correction.sourceLayer}`)}</dd>
                  </div>
                  {correction.machineSource && (
                    <>
                      <div>
                        <dt className="font-medium">{t('terms.audioTokens')}</dt>
                        <dd>{correction.machineSource.firstTokenIndex}–{correction.machineSource.lastTokenIndex}</dd>
                      </div>
                      <div>
                        <dt className="font-medium">{t('terms.machineEvidence')}</dt>
                        <dd title={correction.machineSource.tokenTrackSha256}>
                          {correction.machineSource.termId} · {Math.round(correction.machineSource.confidence * 100)}%
                        </dd>
                      </div>
                    </>
                  )}
                </dl>

                <div className="mt-3 flex justify-end">
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={
                      review.candidate.isActive
                      || pendingKeys.has(`correction:${correction.correctionId}`)
                    }
                    onClick={() => void onSetCorrectionState(correction.correctionId, !applied)}
                  >
                    <RotateCcw className="h-4 w-4" aria-hidden="true" />
                    {t(applied ? 'actions.undoCorrection' : 'actions.reapplyCorrection')}
                  </Button>
                </div>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
