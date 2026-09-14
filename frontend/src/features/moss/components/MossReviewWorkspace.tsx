'use client';

import React, { useEffect, useMemo } from 'react';
import { AlertCircle, Info, Loader2, RefreshCw, Sparkles, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { MOSS_ERROR_I18N_KEYS } from '../types';
import { useMossWorkspace } from '../useMossWorkspace';
import { isMossRunActive, mossRunFailureI18nKey } from '../utils';
import type { MossReviewService } from '../service';
import { MossActivationPanel } from './MossActivationPanel';
import { MossSpeakerBindings } from './MossSpeakerBindings';
import { MossTermCorrections } from './MossTermCorrections';
import { MossTranscriptComparison } from './MossTranscriptComparison';

function safeDate(value: string, locale: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? '—'
    : new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}

export function MossReviewWorkspace({
  meetingId,
  enabled,
  active,
  onBusyChange,
  onTranscriptChanged,
  service,
}: {
  meetingId: string;
  enabled: boolean;
  active: boolean;
  onBusyChange?: (busy: boolean) => void;
  onTranscriptChanged?: () => Promise<void> | void;
  service?: MossReviewService;
}) {
  const { t, i18n } = useTranslation('moss');
  const moss = useMossWorkspace({ meetingId, enabled, active, service });
  const workspace = moss.workspace;
  const review = moss.review;
  const locale = i18n.resolvedLanguage ?? 'en';
  const runningTask = useMemo(
    () => workspace?.runs.find(isMossRunActive) ?? null,
    [workspace?.runs],
  );
  const selectedRun = useMemo(() => {
    if (!workspace) return null;
    return workspace.runs.find((run) => run.runId === workspace.selectedRunId)
      ?? workspace.runs[0]
      ?? null;
  }, [workspace]);

  useEffect(() => {
    onBusyChange?.(moss.hasRunningTask);
    return () => onBusyChange?.(false);
  }, [moss.hasRunningTask, onBusyChange]);

  const afterTranscriptMutation = async (operation: () => Promise<unknown>) => {
    const result = await operation();
    if (result) await onTranscriptChanged?.();
    return result;
  };

  if (!enabled) return null;

  return (
    <section className="space-y-4" aria-labelledby="moss-review-heading" aria-busy={moss.loading}>
      <div className="sr-only" aria-live="polite">
        {moss.pendingKeys.size > 0 ? t('workspace.loading') : ''}
      </div>

      <Alert className="border-blue-200 bg-blue-50 text-blue-950">
        <Info className="h-4 w-4" aria-hidden="true" />
        <AlertDescription>{t('workspace.nativeHotwordsWarning')}</AlertDescription>
      </Alert>

      {moss.error && (
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" aria-hidden="true" />
          <AlertTitle>{t(MOSS_ERROR_I18N_KEYS[moss.error.code])}</AlertTitle>
          <AlertDescription className="space-y-2">
            <p>{t('workspace.debugReference', { debugId: moss.error.debugId })}</p>
            {moss.error.retryable && (
              <Button type="button" size="sm" variant="outline" onClick={() => void moss.refresh()}>
                <RefreshCw className="h-4 w-4" aria-hidden="true" />
                {t('actions.refresh')}
              </Button>
            )}
          </AlertDescription>
        </Alert>
      )}

      {moss.loading && !workspace && (
        <div className="flex min-h-40 items-center justify-center gap-2 text-sm text-gray-600" role="status" aria-live="polite">
          <Loader2 className="h-5 w-5 animate-spin" aria-hidden="true" />
          {t('workspace.loading')}
        </div>
      )}

      {workspace && (
        <>
          <div className="flex flex-wrap items-end justify-between gap-3 rounded-lg border border-gray-200 bg-white p-4">
            <div className="min-w-[220px] flex-1">
              <label className="text-sm font-medium text-gray-800" htmlFor="moss-run-selector">
                {t('workspace.selectRun')}
              </label>
              {workspace.runs.length > 0 ? (
                <Select
                  value={selectedRun?.runId}
                  onValueChange={(runId) => void moss.selectRun(runId)}
                  disabled={moss.pendingKeys.has('select-run')}
                >
                  <SelectTrigger id="moss-run-selector" className="mt-2 w-full bg-white">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {workspace.runs.map((run) => (
                      <SelectItem key={run.runId} value={run.runId}>
                        {t(`runState.${run.state}`)} · {safeDate(run.createdAt, locale)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              ) : (
                <p className="mt-2 text-sm text-gray-600">{t('workspace.noRuns')}</p>
              )}
            </div>

            <div className="flex flex-wrap gap-2">
              <Button
                type="button"
                variant="outline"
                onClick={() => void moss.refresh()}
                disabled={moss.loading}
              >
                <RefreshCw className={`h-4 w-4 ${moss.loading ? 'animate-spin' : ''}`} aria-hidden="true" />
                {t('actions.refresh')}
              </Button>
              <Button
                type="button"
                onClick={() => void moss.startRun()}
                disabled={
                  workspace.system.availability !== 'ready'
                  || moss.hasRunningTask
                  || moss.pendingKeys.has('start-run')
                }
              >
                <Sparkles className="h-4 w-4" aria-hidden="true" />
                {t('actions.start')}
              </Button>
            </div>
          </div>

          {runningTask && (
            <div className="rounded-lg border border-blue-200 bg-blue-50 p-4" role="status" aria-live="polite">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <p className="font-medium text-blue-950">{t(`runState.${runningTask.state}`)}</p>
                  <p className="text-sm text-blue-800">
                    {runningTask.progress ? t(`runStage.${runningTask.progress.stage}`) : t('runStage.queued')}
                  </p>
                </div>
                {runningTask.canCancel && (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => void moss.cancelRun(runningTask.runId)}
                    disabled={moss.pendingKeys.has(`cancel-run:${runningTask.runId}`)}
                  >
                    <X className="h-4 w-4" aria-hidden="true" />
                    {t('actions.cancelRun')}
                  </Button>
                )}
              </div>
              <Progress
                className="mt-3"
                value={runningTask.progress?.percentage ?? 0}
                aria-label={runningTask.progress ? t(`runStage.${runningTask.progress.stage}`) : t('runStage.queued')}
              />
              <p className="mt-1 text-right text-xs text-blue-800">
                {Math.round(runningTask.progress?.percentage ?? 0)}%
              </p>
            </div>
          )}

          {selectedRun?.state === 'failed' && (
            <Alert variant="destructive">
              <AlertCircle className="h-4 w-4" aria-hidden="true" />
              <AlertDescription>{t(mossRunFailureI18nKey(selectedRun.errorCode))}</AlertDescription>
            </Alert>
          )}

          {review ? (
            <div className="space-y-4">
              <Tabs defaultValue="comparison">
                <TabsList className="grid h-auto w-full grid-cols-3">
                  <TabsTrigger value="comparison" className="whitespace-normal">{t('tabs.comparison')}</TabsTrigger>
                  <TabsTrigger value="speakers" className="whitespace-normal">{t('tabs.speakers')}</TabsTrigger>
                  <TabsTrigger value="terms" className="whitespace-normal">{t('tabs.terms')}</TabsTrigger>
                </TabsList>
                <TabsContent value="comparison" className="mt-4">
                  <MossTranscriptComparison
                    review={review}
                    pendingKeys={moss.pendingKeys}
                    onSaveSegment={(segmentId, text) => moss.updateCandidateSegment({
                      runId: review.candidate.runId,
                      segmentId,
                      text,
                    })}
                  />
                </TabsContent>
                <TabsContent value="speakers" className="mt-4">
                  <MossSpeakerBindings
                    review={review}
                    pendingKeys={moss.pendingKeys}
                    onSaveBinding={(speakerLabel, personId) => moss.saveSpeakerBinding({
                      runId: review.candidate.runId,
                      speakerLabel,
                      personId,
                    })}
                    onSaveOverride={(segmentId, personId) => moss.saveSegmentOverride({
                      runId: review.candidate.runId,
                      segmentId,
                      personId,
                    })}
                  />
                </TabsContent>
                <TabsContent value="terms" className="mt-4">
                  <MossTermCorrections
                    review={review}
                    pendingKeys={moss.pendingKeys}
                    onSetCorrectionState={(correctionId, applied) => moss.setCorrectionState({
                      runId: review.candidate.runId,
                      correctionId,
                      applied,
                    })}
                  />
                </TabsContent>
              </Tabs>

              <MossActivationPanel
                review={review}
                pendingKeys={moss.pendingKeys}
                onActivate={() => afterTranscriptMutation(moss.activateCandidate)}
                onRollback={() => afterTranscriptMutation(moss.rollbackActivation)}
              />
            </div>
          ) : workspace.runs.length > 0 && !runningTask ? (
            <p className="rounded-md border border-gray-200 bg-gray-50 p-4 text-sm text-gray-600">
              {t('workspace.noReview')}
            </p>
          ) : null}
        </>
      )}
    </section>
  );
}
