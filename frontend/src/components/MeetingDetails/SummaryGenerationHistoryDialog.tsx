"use client";

import { HelpHint } from '@/components/ui/help-hint';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArchiveRestore, Download, Eye, History, Loader2, PencilLine, RefreshCw, ShieldCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { normalizeTemplateApiError, templateService } from '@/services/templateService';
import type {
  SnapshotCleanupPreview,
  SnapshotRetentionPolicy,
  SummaryGenerationHistoryItem,
  SummaryGenerationSnapshotDetails,
  ManualSummaryRevision,
} from '@/types/summary-template';

const DEFAULT_POLICY: SnapshotRetentionPolicy = {
  retainLatest: 20,
  retainDays: 90,
  maxTotalBytes: 512 * 1024 * 1024,
};

interface SummaryGenerationHistoryDialogProps {
  meetingId: string;
  disabled?: boolean;
  /** PRO 版式：顶部图标簇里只显示图标，文字靠 tooltip 表达 */
  trigger?: 'default' | 'icon';
  onRetryGeneration: (generationId: string) => Promise<void>;
  onManualRevisionRestored?: (summary: Record<string, unknown>) => void;
}

function byteLabel(bytes: number, locale: string): string {
  const units = ['B', 'KiB', 'MiB', 'GiB'];
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: unit === 0 ? 0 : 1 }).format(value)} ${units[unit]}`;
}

export function SummaryGenerationHistoryDialog({
  meetingId,
  disabled = false,
  trigger = 'default',
  onRetryGeneration,
  onManualRevisionRestored,
}: SummaryGenerationHistoryDialogProps) {
  const { t, i18n } = useTranslation('summary');
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [history, setHistory] = useState<SummaryGenerationHistoryItem[]>([]);
  const [manualRevisions, setManualRevisions] = useState<ManualSummaryRevision[]>([]);
  const [restoringRevisionId, setRestoringRevisionId] = useState<string | null>(null);
  const [policy, setPolicy] = useState(DEFAULT_POLICY);
  const [preview, setPreview] = useState<SnapshotCleanupPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [retryingId, setRetryingId] = useState<string | null>(null);
  const [detailsId, setDetailsId] = useState<string | null>(null);
  const [details, setDetails] = useState<SummaryGenerationSnapshotDetails | null>(null);
  const [detailsLoading, setDetailsLoading] = useState(false);
  const [debugId, setDebugId] = useState<string | null>(null);

  const loadHistory = useCallback(async () => {
    setLoading(true);
    setDebugId(null);
    try {
      const [generationHistory, savedRevisions] = await Promise.all([
        templateService.listSummaryGenerationHistory(meetingId),
        templateService.listManualSummaryRevisions(meetingId),
      ]);
      setHistory(generationHistory);
      setManualRevisions(savedRevisions);
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      setDebugId(normalized.debugId);
      toast.error(t('generationHistory.loadFailed'));
    } finally {
      setLoading(false);
    }
  }, [meetingId, t]);

  useEffect(() => {
    if (open) void loadHistory();
    if (!open) setPreview(null);
  }, [loadHistory, open]);

  const completedCount = useMemo(
    () => history.filter((item) => item.status === 'completed').length,
    [history],
  );

  const updatePolicy = (key: keyof SnapshotRetentionPolicy, value: number) => {
    setPolicy((current) => ({ ...current, [key]: value }));
    setPreview(null);
  };

  const runPreview = async () => {
    setPreviewing(true);
    setDebugId(null);
    try {
      setPreview(await templateService.previewSnapshotCleanup(meetingId, policy));
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      setDebugId(normalized.debugId);
      toast.error(t('generationHistory.cleanup.previewFailed'));
    } finally {
      setPreviewing(false);
    }
  };

  const executeCleanup = async () => {
    if (!preview || preview.candidateFileCount === 0) return;
    setCleaning(true);
    setDebugId(null);
    try {
      const result = await templateService.executeSnapshotCleanup(
        meetingId,
        policy,
        preview.previewToken,
        preview.items.filter((item) => item.cleanupCandidate).map((item) => item.generationId),
      );
      toast.success(t('generationHistory.cleanup.completed', {
        count: result.quarantinedFileCount,
        size: byteLabel(result.quarantinedBytes, i18n.language),
      }));
      if (result.planChanged || result.skippedGenerationIds.length > 0) {
        toast.info(t('generationHistory.cleanup.planChanged'));
      }
      setPreview(null);
      await loadHistory();
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      setDebugId(normalized.debugId);
      toast.error(t('generationHistory.cleanup.executeFailed'));
    } finally {
      setCleaning(false);
    }
  };

  const retry = async (generationId: string) => {
    setRetryingId(generationId);
    try {
      setOpen(false);
      await onRetryGeneration(generationId);
    } finally {
      setRetryingId(null);
    }
  };

  const restoreManualRevision = async (revisionId: string) => {
    setRestoringRevisionId(revisionId);
    setDebugId(null);
    try {
      const summary = await templateService.restoreManualSummaryRevision(meetingId, revisionId);
      onManualRevisionRestored?.(summary);
      toast.success(t('generationHistory.manualRevisions.restored'));
      setOpen(false);
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      setDebugId(normalized.debugId);
      toast.error(t('generationHistory.manualRevisions.restoreFailed'));
    } finally {
      setRestoringRevisionId(null);
    }
  };

  const toggleSnapshotDetails = async (generationId: string) => {
    if (detailsId === generationId) {
      setDetailsId(null);
      setDetails(null);
      return;
    }
    setDetailsId(generationId);
    setDetails(null);
    setDetailsLoading(true);
    try {
      setDetails(await templateService.getSummaryGenerationSnapshot(meetingId, generationId));
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      setDebugId(normalized.debugId);
      setDetailsId(null);
      toast.error(t('generationHistory.snapshotDetailsLoadFailed'));
    } finally {
      setDetailsLoading(false);
    }
  };

  const exportDiagnostics = () => {
    const payload = {
      schemaVersion: 2,
      exportedAt: new Date().toISOString(),
      generationCount: history.length,
      manualRevisionCount: manualRevisions.length,
      manualRevisions: manualRevisions.map((item) => ({
        revisionId: item.revisionId,
        createdAt: item.createdAt,
        sourceGenerationId: item.sourceGenerationId,
        isCurrent: item.isCurrent,
      })),
      generations: history.map((item) => ({
        generationId: item.generationId,
        status: item.status,
        createdAt: item.createdAt,
        completedAt: item.completedAt,
        templateId: item.templateId,
        templateVersion: item.templateVersion,
        resolutionSource: item.resolutionSource,
        modelProvider: item.modelProvider,
        modelName: item.modelName,
        summaryLanguage: item.summaryLanguage,
        transcriptSource: item.transcriptSource,
        transcriptVersionId: item.transcriptVersionId,
        transcriptVersion: item.transcriptVersion,
        mossRunId: item.mossRunId,
        transcriptSha256: item.transcriptSha256,
        speakerBindingSnapshotId: item.speakerBindingSnapshotId,
        speakerBindingVersion: item.speakerBindingVersion,
        speakerBindingSha256: item.speakerBindingSha256,
        errorCategory: item.errorCategory,
        snapshotState: item.snapshotState,
        isCurrentSummary: item.isCurrentSummary,
      })),
    };
    const url = URL.createObjectURL(new Blob([JSON.stringify(payload, null, 2)], {
      type: 'application/json',
    }));
    const fileName = `meetily-generation-diagnostics-${new Date().toISOString().slice(0, 10)}.json`;
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = fileName;
    anchor.click();
    URL.revokeObjectURL(url);
    // P2-23：导出不再"静默成功"，明确告诉用户导出了什么文件
    toast.success(t('generationHistory.exported', { file: fileName }));
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        {trigger === 'icon' ? (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
            disabled={disabled}
            title={t('generationHistory.title')}
            aria-label={t('generationHistory.title')}
          >
            <History className="h-4 w-4" />
          </Button>
        ) : (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={disabled}
            title={t('generationHistory.title')}
            aria-label={t('generationHistory.title')}
          >
            <History className="h-4 w-4" />
            <span className="hidden lg:inline">{t('generationHistory.shortTitle')}</span>
          </Button>
        )}
      </DialogTrigger>
      <DialogContent className="max-h-[88vh] overflow-y-auto sm:max-w-3xl">
        <DialogTitle>{t('generationHistory.title')}</DialogTitle>
        <DialogDescription>
          {t('generationHistory.description', { count: history.length, completed: completedCount })}
        </DialogDescription>

        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" onClick={() => void loadHistory()} disabled={loading}>
            {loading ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <RefreshCw className="mr-2 h-4 w-4" />}
            {t('generationHistory.refresh')}
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={exportDiagnostics} disabled={!history.length}>
            <Download className="mr-2 h-4 w-4" />
            {t('generationHistory.exportDiagnostics')}
          </Button>
        </div>

        {debugId ? (
          <p className="rounded-md bg-red-50 p-3 text-sm text-red-700">
            {t('generationHistory.safeError', { debugId })}
          </p>
        ) : null}

        <section className="rounded-lg border border-gray-200 p-4">
          <div className="flex items-start gap-3">
            <PencilLine className="mt-0.5 h-5 w-5 text-gray-500" aria-hidden="true" />
            <div>
              <h3 className="flex items-center gap-1 font-medium">{t('generationHistory.manualRevisions.title')}<HelpHint text={t('generationHistory.manualRevisions.description', { count: manualRevisions.length })} /></h3>
            </div>
          </div>
          {manualRevisions.length === 0 ? (
            <p className="mt-4 rounded-md border border-dashed bg-white p-3 text-sm text-muted-foreground">
              {t('generationHistory.manualRevisions.empty')}
            </p>
          ) : (
            <div className="mt-4 space-y-3">
              {manualRevisions.map((revision) => (
                <article key={revision.revisionId} className="rounded-md border bg-white p-3">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium">{t('generationHistory.manualRevisions.savedVersion')}</span>
                        {revision.isCurrent ? (
                          <span className="rounded-full bg-green-100 px-2 py-0.5 text-xs text-green-800">
                            {t('generationHistory.currentSummary')}
                          </span>
                        ) : null}
                      </div>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {new Intl.DateTimeFormat(i18n.language, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(revision.createdAt))}
                      </p>
                      {revision.sourceGenerationId ? (
                        <p className="mt-1 break-all font-mono text-xs text-gray-600">
                          {t('generationHistory.manualRevisions.basedOn')}: {revision.sourceGenerationId}
                        </p>
                      ) : null}
                      <p className="mt-2 line-clamp-3 whitespace-pre-wrap text-sm text-gray-700">
                        {revision.markdown || t('generationHistory.manualRevisions.noPreview')}
                      </p>
                    </div>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={restoringRevisionId !== null || revision.isCurrent}
                      onClick={() => void restoreManualRevision(revision.revisionId)}
                    >
                      {restoringRevisionId === revision.revisionId
                        ? <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        : <ArchiveRestore className="mr-2 h-4 w-4" />}
                      {t('generationHistory.manualRevisions.restore')}
                    </Button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </section>

        <div className="space-y-3">
          {loading && history.length === 0 ? (
            <div className="flex justify-center py-8"><Loader2 className="h-6 w-6 animate-spin" /></div>
          ) : history.length === 0 ? (
            <p className="rounded-md border border-dashed p-5 text-center text-sm text-muted-foreground">
              {t('generationHistory.empty')}
            </p>
          ) : history.map((item) => (
            <article key={item.generationId} className="rounded-lg border p-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">{t(`generationHistory.status.${item.status}`)}</span>
                    {item.isCurrentSummary ? (
                      <span className="rounded-full bg-green-100 px-2 py-0.5 text-xs text-green-800">
                        {t('generationHistory.currentSummary')}
                      </span>
                    ) : null}
                    <span className="rounded-full bg-gray-100 px-2 py-0.5 text-xs text-gray-700">
                      {t(`generationHistory.snapshotState.${item.snapshotState}`)}
                    </span>
                  </div>
                  <p className="mt-1 text-sm text-muted-foreground">
                    {item.templateId} · v{item.templateVersion} · {item.modelProvider}/{item.modelName}
                  </p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {new Intl.DateTimeFormat(i18n.language, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(item.createdAt))}
                  </p>
                  <p className="mt-2 break-all font-mono text-xs text-gray-600">
                    {t('generationHistory.generationId')}: {item.generationId}
                  </p>
                  {item.transcriptSource ? (
                    <p className="mt-1 break-all font-mono text-xs text-gray-600">
                      {t('generationHistory.transcriptSource')}: {item.transcriptSource}
                      {item.mossRunId ? ` · ${t('generationHistory.mossRunId')}: ${item.mossRunId}` : ''}
                    </p>
                  ) : null}
                  {item.errorCategory ? (
                    <p className="mt-2 text-sm text-red-700">
                      {t('generationHistory.errorCategoryLabel')}: {t(`generationHistory.errorCategory.${item.errorCategory}`, {
                        defaultValue: t('generationHistory.errorCategory.generation_failed'),
                      })}
                    </p>
                  ) : null}
                </div>
                <div className="flex flex-col gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={item.snapshotState !== 'available' || detailsLoading}
                    onClick={() => void toggleSnapshotDetails(item.generationId)}
                  >
                    {detailsLoading && detailsId === item.generationId
                      ? <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      : <Eye className="mr-2 h-4 w-4" />}
                    {detailsId === item.generationId
                      ? t('generationHistory.hideSnapshot')
                      : t('generationHistory.viewSnapshot')}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={!item.canRetryWithSnapshot || retryingId !== null}
                    onClick={() => void retry(item.generationId)}
                  >
                    {retryingId === item.generationId
                      ? <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      : <ArchiveRestore className="mr-2 h-4 w-4" />}
                    {t('generationHistory.retryExact')}
                  </Button>
                </div>
              </div>
              {detailsId === item.generationId && details ? (
                <div className="mt-4 rounded-md bg-gray-50 p-3">
                  <p className="font-medium">{details.template.name}</p>
                  {details.template.description ? (
                    <p className="mt-1 text-sm text-muted-foreground">{details.template.description}</p>
                  ) : null}
                  <dl className="mt-3 grid gap-2 text-xs sm:grid-cols-2">
                    <div><dt className="text-muted-foreground">{t('generationHistory.semanticHash')}</dt><dd className="break-all font-mono">{details.semanticSha256}</dd></div>
                    <div><dt className="text-muted-foreground">{t('generationHistory.fileHash')}</dt><dd className="break-all font-mono">{details.fileSha256}</dd></div>
                    {details.meetingContextId ? <div><dt className="text-muted-foreground">{t('generationHistory.meetingContextId')}</dt><dd className="break-all font-mono">{details.meetingContextId}</dd></div> : null}
                    {details.meetingContextSha256 ? <div><dt className="text-muted-foreground">{t('generationHistory.meetingContextHash')}</dt><dd className="break-all font-mono">{details.meetingContextSha256}</dd></div> : null}
                    {details.summaryContextSha256 ? <div><dt className="text-muted-foreground">{t('generationHistory.summaryContextHash')}</dt><dd className="break-all font-mono">{details.summaryContextSha256}</dd></div> : null}
                    {details.summarySourceBinding ? <>
                      <div><dt className="text-muted-foreground">{t('generationHistory.transcriptSource')}</dt><dd className="break-all font-mono">{details.summarySourceBinding.transcriptSource}</dd></div>
                      <div><dt className="text-muted-foreground">{t('generationHistory.transcriptVersion')}</dt><dd className="break-all font-mono">{details.summarySourceBinding.transcriptVersionId} · v{details.summarySourceBinding.transcriptVersion}</dd></div>
                      {details.summarySourceBinding.mossRunId ? <div><dt className="text-muted-foreground">{t('generationHistory.mossRunId')}</dt><dd className="break-all font-mono">{details.summarySourceBinding.mossRunId}</dd></div> : null}
                      <div><dt className="text-muted-foreground">{t('generationHistory.transcriptHash')}</dt><dd className="break-all font-mono">{details.summarySourceBinding.transcriptSha256}</dd></div>
                      <div><dt className="text-muted-foreground">{t('generationHistory.speakerBindingHash')}</dt><dd className="break-all font-mono">{details.summarySourceBinding.speakerBindingSha256}</dd></div>
                    </> : null}
                  </dl>
                  <div className="mt-3 space-y-2">
                    {details.template.sections.map((section) => (
                      <div key={section.id} className="rounded border bg-white p-2">
                        <p className="text-sm font-medium">{section.title}</p>
                        <p className="mt-1 whitespace-pre-wrap text-xs text-muted-foreground">{section.instruction}</p>
                      </div>
                    ))}
                  </div>
                </div>
              ) : null}
            </article>
          ))}
        </div>

        <section className="rounded-lg border bg-gray-50 p-4">
          <div className="flex items-start gap-3">
            <ShieldCheck className="mt-0.5 h-5 w-5 text-blue-700" />
            <div>
              <h3 className="font-medium">{t('generationHistory.cleanup.title')}</h3>
              <p className="mt-1 text-sm text-muted-foreground">{t('generationHistory.cleanup.description')}</p>
            </div>
          </div>
          <div className="mt-4 grid gap-3 sm:grid-cols-3">
            <label className="text-sm">
              <span className="mb-1 block">{t('generationHistory.cleanup.retainLatest')}</span>
              <input className="w-full rounded-md border bg-white px-3 py-2" type="number" min={0} max={10000} value={policy.retainLatest}
                onChange={(event) => updatePolicy('retainLatest', Math.max(0, Number(event.target.value) || 0))} />
            </label>
            <label className="text-sm">
              <span className="mb-1 block">{t('generationHistory.cleanup.retainDays')}</span>
              <input className="w-full rounded-md border bg-white px-3 py-2" type="number" min={1} max={36500} value={policy.retainDays}
                onChange={(event) => updatePolicy('retainDays', Math.max(1, Number(event.target.value) || 1))} />
            </label>
            <label className="text-sm">
              <span className="mb-1 block">{t('generationHistory.cleanup.maxMiB')}</span>
              <input className="w-full rounded-md border bg-white px-3 py-2" type="number" min={1} max={1048576} value={Math.round(policy.maxTotalBytes / 1024 / 1024)}
                onChange={(event) => updatePolicy('maxTotalBytes', Math.max(1, Number(event.target.value) || 1) * 1024 * 1024)} />
            </label>
          </div>
          <div className="mt-4 flex flex-wrap items-center gap-3">
            <Button type="button" variant="outline" size="sm" onClick={() => void runPreview()} disabled={previewing || cleaning}>
              {previewing ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              {t('generationHistory.cleanup.preview')}
            </Button>
            {preview ? (
              <span className="text-sm text-muted-foreground">
                {t('generationHistory.cleanup.previewSummary', {
                  count: preview.candidateFileCount,
                  size: byteLabel(preview.candidateBytes, i18n.language),
                  protected: preview.protectedFileCount,
                })}
              </span>
            ) : null}
          </div>
          {preview ? (
            <div className="mt-4 rounded-md border bg-white p-3">
              <p className="text-sm">{t('generationHistory.cleanup.quarantineNotice')}</p>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="mt-3 border-red-300 text-red-700 hover:bg-red-50"
                disabled={cleaning || preview.candidateFileCount === 0}
                onClick={() => void executeCleanup()}
              >
                {cleaning ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
                {t('generationHistory.cleanup.execute', { count: preview.candidateFileCount })}
              </Button>
            </div>
          ) : null}
        </section>
      </DialogContent>
    </Dialog>
  );
}
