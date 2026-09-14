'use client';

import { HelpHint } from '@/components/ui/help-hint';
import { useEffect, useMemo, useRef, useState } from 'react';
import { open as openFile, save } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import {
  AlertTriangle,
  CheckCircle2,
  Download,
  LoaderCircle,
  PackageOpen,
  RefreshCw,
  ShieldCheck,
  Upload,
  XCircle,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { templateTranslationKey } from '@/lib/template-library';
import {
  applyPortableStrategyToConflictKind,
  buildPortableImportDecisions,
  createPortableExecutionId,
  ensurePortablePackExtension,
  formatPortableBytes,
  portableConflictCounts,
  portableExportCandidates,
  portablePackFileName,
  reconcilePortableImportDecisions,
  shouldRestartPortablePreview,
  unresolvedPortableImportItems,
} from '@/lib/portable-template-pack';
import { normalizeTemplateApiError, templateService } from '@/services/templateService';
import type {
  ExecuteTemplatePackImportResponse,
  ExportTemplatePackResponse,
  PlanTemplatePackImportResponse,
  PortablePackConflictKind,
  PortablePackConflictStrategy,
  PortablePackImportItem,
  PreviewTemplatePackExportResponse,
  PreviewTemplatePackImportResponse,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';

type PackMode = 'export' | 'import';
type ExportStage = 'select' | 'previewing' | 'review' | 'exporting' | 'complete';
type ImportStage =
  | 'idle'
  | 'previewing'
  | 'review'
  | 'planning'
  | 'planned'
  | 'executing'
  | 'cancelling'
  | 'complete'
  | 'cancelled'
  | 'failed';

interface PortableTemplatePackDialogProps {
  disabled?: boolean;
  templates: readonly TemplateListItem[];
  onImported: () => void | Promise<void>;
}

const PACK_EXTENSION = 'meetily-template-pack';

export function PortableTemplatePackDialog({
  disabled = false,
  templates,
  onImported,
}: PortableTemplatePackDialogProps) {
  const { t, i18n } = useTranslation('templates');
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const candidates = useMemo(() => portableExportCandidates(templates), [templates]);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [mode, setMode] = useState<PackMode>('export');
  const [selectedTemplateIds, setSelectedTemplateIds] = useState<Set<string>>(new Set());
  const [exportStage, setExportStage] = useState<ExportStage>('select');
  const [exportPreview, setExportPreview] = useState<PreviewTemplatePackExportResponse | null>(null);
  const [exportResult, setExportResult] = useState<ExportTemplatePackResponse | null>(null);
  const [exportError, setExportError] = useState<TemplateApiError | null>(null);
  const [importStage, setImportStage] = useState<ImportStage>('idle');
  const [sourceFileName, setSourceFileName] = useState('');
  const [importPreview, setImportPreview] = useState<PreviewTemplatePackImportResponse | null>(null);
  const [importDecisions, setImportDecisions] = useState<Record<string, PortablePackConflictStrategy>>({});
  const [importPlan, setImportPlan] = useState<PlanTemplatePackImportResponse | null>(null);
  const [importResult, setImportResult] = useState<ExecuteTemplatePackImportResponse | null>(null);
  const [importError, setImportError] = useState<TemplateApiError | null>(null);
  const [cancelError, setCancelError] = useState<TemplateApiError | null>(null);
  const sourcePathRef = useRef('');
  const executionIdRef = useRef<string | null>(null);
  const importOperationRef = useRef(0);
  const exportOperationRef = useRef(0);

  const exportBusy = exportStage === 'previewing' || exportStage === 'exporting';
  const importBusy = ['previewing', 'planning', 'executing', 'cancelling'].includes(importStage);
  const busy = exportBusy || importBusy;
  const importHasProgress = !['idle', 'complete', 'cancelled'].includes(importStage);
  const exportHasProgress = exportStage !== 'complete'
    && (selectedTemplateIds.size > 0 || exportStage !== 'select');
  const hasProgress = exportHasProgress || importHasProgress;
  const unresolvedItems = importPreview
    ? unresolvedPortableImportItems(importPreview.items, importDecisions)
    : [];
  const conflictCounts = importPreview ? portableConflictCounts(importPreview) : null;

  const resetExport = () => {
    exportOperationRef.current += 1;
    setSelectedTemplateIds(new Set());
    setExportStage('select');
    setExportPreview(null);
    setExportResult(null);
    setExportError(null);
  };

  const resetImport = () => {
    importOperationRef.current += 1;
    sourcePathRef.current = '';
    executionIdRef.current = null;
    setImportStage('idle');
    setSourceFileName('');
    setImportPreview(null);
    setImportDecisions({});
    setImportPlan(null);
    setImportResult(null);
    setImportError(null);
    setCancelError(null);
  };

  const resetAndClose = () => {
    resetExport();
    resetImport();
    setDiscardOpen(false);
    setMode('export');
    setDialogOpen(false);
  };

  const requestClose = () => {
    if (busy) {
      toast.warning(t('portable.leave.busy'));
      return;
    }
    if (hasProgress) {
      setDiscardOpen(true);
      return;
    }
    resetAndClose();
  };

  const toggleExportTemplate = (templateId: string) => {
    if (exportBusy) return;
    setSelectedTemplateIds((current) => {
      const next = new Set(current);
      if (next.has(templateId)) next.delete(templateId);
      else next.add(templateId);
      return next;
    });
    setExportPreview(null);
    setExportResult(null);
    setExportError(null);
    setExportStage('select');
  };

  const selectAllExportTemplates = () => {
    if (exportBusy) return;
    const allSelected = candidates.length > 0 && selectedTemplateIds.size === candidates.length;
    setSelectedTemplateIds(allSelected ? new Set() : new Set(candidates.map((item) => item.id)));
    setExportPreview(null);
    setExportResult(null);
    setExportError(null);
    setExportStage('select');
  };

  const previewExport = async () => {
    if (exportBusy || selectedTemplateIds.size === 0) return;
    const operationId = ++exportOperationRef.current;
    setExportStage('previewing');
    setExportError(null);
    setExportResult(null);
    try {
      const preview = await templateService.previewPackExport({
        templateIds: [...selectedTemplateIds].sort(),
      });
      if (operationId !== exportOperationRef.current) return;
      setExportPreview(preview);
      setExportStage('review');
    } catch (caught) {
      if (operationId !== exportOperationRef.current) return;
      setExportError(normalizeTemplateApiError(caught));
      setExportStage('select');
    }
  };

  const exportPack = async () => {
    if (!exportPreview || exportBusy) return;
    try {
      const selection = await save({
        title: t('portable.export.filePickerTitle'),
        defaultPath: `meetily-templates.${PACK_EXTENSION}`,
        filters: [{ name: t('portable.fileType'), extensions: [PACK_EXTENSION] }],
      });
      if (!selection) return;
      const operationId = ++exportOperationRef.current;
      setExportStage('exporting');
      setExportError(null);
      try {
        const result = await templateService.exportPack({
          planToken: exportPreview.planToken,
          destinationPath: ensurePortablePackExtension(selection),
          overwrite: true,
        });
        if (operationId !== exportOperationRef.current) return;
        setExportResult(result);
        setExportStage('complete');
        toast.success(t('portable.export.success', { count: result.package.templateCount }));
      } catch (caught) {
        if (operationId !== exportOperationRef.current) return;
        const error = normalizeTemplateApiError(caught);
        setExportError(error);
        setExportPreview(null);
        setExportStage('select');
      }
    } catch (caught) {
      setExportError(normalizeTemplateApiError(caught));
    }
  };

  const previewImportPath = async (sourcePath: string) => {
    if (importBusy || !sourcePath) return;
    const operationId = ++importOperationRef.current;
    sourcePathRef.current = sourcePath;
    setSourceFileName(portablePackFileName(sourcePath));
    setImportStage('previewing');
    setImportPreview(null);
    setImportDecisions({});
    setImportPlan(null);
    setImportResult(null);
    setImportError(null);
    setCancelError(null);
    try {
      const preview = await templateService.previewPackImport({ sourcePath });
      if (operationId !== importOperationRef.current) return;
      setSourceFileName(preview.package.packageFileName || portablePackFileName(sourcePath));
      setImportPreview(preview);
      setImportDecisions((current) => reconcilePortableImportDecisions(preview.items, current));
      setImportStage('review');
    } catch (caught) {
      if (operationId !== importOperationRef.current) return;
      setImportError(normalizeTemplateApiError(caught));
      setImportStage('idle');
    }
  };

  const chooseImportPack = async () => {
    if (importBusy) return;
    try {
      const selection = await openFile({
        multiple: false,
        directory: false,
        title: t('portable.import.filePickerTitle'),
        filters: [{ name: t('portable.fileType'), extensions: [PACK_EXTENSION] }],
      });
      if (!selection || Array.isArray(selection)) return;
      await previewImportPath(selection);
    } catch (caught) {
      setImportError(normalizeTemplateApiError(caught));
    }
  };

  const setImportDecision = (item: PortablePackImportItem, strategy: PortablePackConflictStrategy) => {
    if (importBusy || importPlan || !item.allowedStrategies.includes(strategy)) return;
    setImportDecisions((current) => ({ ...current, [item.itemId]: strategy }));
    setImportError(null);
  };

  const applyDecisionToConflictKind = (
    conflictKind: PortablePackConflictKind,
    strategy: PortablePackConflictStrategy,
  ) => {
    if (!importPreview || importBusy || importPlan) return;
    setImportDecisions((current) => applyPortableStrategyToConflictKind(
      importPreview.items,
      current,
      conflictKind,
      strategy,
    ));
    setImportError(null);
  };

  const planImport = async () => {
    if (!importPreview || importBusy || unresolvedItems.length > 0) return;
    const operationId = ++importOperationRef.current;
    setImportStage('planning');
    setImportError(null);
    try {
      const plan = await templateService.planPackImport({
        previewPlanToken: importPreview.planToken,
        decisions: buildPortableImportDecisions(importPreview.items, importDecisions),
      });
      if (operationId !== importOperationRef.current) return;
      setImportPlan(plan);
      setImportStage('planned');
    } catch (caught) {
      if (operationId !== importOperationRef.current) return;
      setImportError(normalizeTemplateApiError(caught));
      setImportStage('review');
    }
  };

  const executeImport = async () => {
    if (!importPlan || importBusy) return;
    const operationId = ++importOperationRef.current;
    let executionId: string;
    try {
      executionId = createPortableExecutionId();
    } catch {
      setImportError(normalizeTemplateApiError(null));
      return;
    }
    executionIdRef.current = executionId;
    setImportStage('executing');
    setImportError(null);
    setCancelError(null);
    try {
      const result = await templateService.executePackImport({
        executionPlanToken: importPlan.executionPlanToken,
        executionId,
      });
      if (operationId !== importOperationRef.current) return;
      executionIdRef.current = null;
      setImportResult(result);
      setImportStage('complete');
      await onImported();
      toast.success(t('portable.import.success', {
        count: result.summary.createCount + result.summary.replaceCount + result.summary.transformedCount,
      }));
    } catch (caught) {
      if (operationId !== importOperationRef.current) return;
      executionIdRef.current = null;
      const error = normalizeTemplateApiError(caught);
      setImportError(error);
      setImportStage(error.code === 'TEMPLATE_CANCELLED' ? 'cancelled' : 'failed');
      if (error.code === 'TEMPLATE_CANCELLED') {
        toast.info(t('portable.import.cancelled'));
      }
    }
  };

  const cancelImport = async () => {
    const executionId = executionIdRef.current;
    if (importStage !== 'executing' || !executionId) return;
    setImportStage('cancelling');
    setCancelError(null);
    try {
      await templateService.cancelPackImport({ executionId });
      toast.info(t('portable.import.cancelRequested'));
    } catch (caught) {
      setCancelError(normalizeTemplateApiError(caught));
      setImportStage('executing');
    }
  };

  const restartImportPreview = () => {
    const sourcePath = sourcePathRef.current;
    resetImport();
    if (sourcePath) void previewImportPath(sourcePath);
  };

  useEffect(() => () => {
    importOperationRef.current += 1;
    exportOperationRef.current += 1;
    const executionId = executionIdRef.current;
    if (executionId) {
      void templateService.cancelPackImport({ executionId }).catch(() => undefined);
    }
  }, []);

  return (
    <>
      <Button
        type="button"
        variant="outline"
        onClick={() => setDialogOpen(true)}
        disabled={disabled}
      >
        <PackageOpen aria-hidden="true" />
        {t('portable.trigger')}
      </Button>

      <Dialog open={dialogOpen} onOpenChange={(next) => !next && requestClose()}>
        <DialogContent className="max-h-[92vh] overflow-hidden sm:max-w-4xl">
          <DialogHeader>
            <DialogTitle>{t('portable.title')}</DialogTitle>
            <DialogDescription>{t('portable.description')}</DialogDescription>
          </DialogHeader>

          <div className="flex items-start gap-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-900">
            <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>{t('portable.localOnly')}</span>
          </div>

          <Tabs
            value={mode}
            onValueChange={(value) => !busy && setMode(value as PackMode)}
            className="min-h-0"
          >
            <TabsList className="grid w-full grid-cols-2" aria-label={t('portable.modeLabel')}>
              <TabsTrigger value="export" disabled={busy}>
                <Download className="mr-2 h-4 w-4" aria-hidden="true" />
                {t('portable.tabs.export')}
              </TabsTrigger>
              <TabsTrigger value="import" disabled={busy}>
                <Upload className="mr-2 h-4 w-4" aria-hidden="true" />
                {t('portable.tabs.import')}
              </TabsTrigger>
            </TabsList>

            <TabsContent value="export" className="max-h-[62vh] min-h-0 overflow-y-auto pr-1 custom-scrollbar">
              <section className="space-y-4" aria-labelledby="portable-export-title">
                <div>
                  <h3 id="portable-export-title" className="font-semibold text-gray-900">
                    {t('portable.export.title')}
                  </h3>
                  <p className="mt-1 text-sm text-gray-600">{t('portable.export.description')}</p>
                </div>

                {candidates.length === 0 ? (
                  <EmptyPortableState
                    title={t('portable.export.emptyTitle')}
                    description={t('portable.export.emptyDescription')}
                  />
                ) : exportStage === 'complete' && exportResult ? (
                  <PortableSuccessPanel
                    title={t('portable.export.completeTitle')}
                    description={t('portable.export.completeDescription', {
                      count: exportResult.package.templateCount,
                      size: formatPortableBytes(exportResult.byteSize, locale),
                    })}
                    packageId={exportResult.package.packageId}
                    archiveSha256={exportResult.package.archiveSha256}
                  />
                ) : (
                  <>
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <p className="text-sm font-medium">
                        {t('portable.export.selectedCount', { count: selectedTemplateIds.size })}
                      </p>
                      <Button type="button" variant="ghost" size="sm" onClick={selectAllExportTemplates} disabled={exportBusy}>
                        {selectedTemplateIds.size === candidates.length
                          ? t('portable.export.clearSelection')
                          : t('portable.export.selectAll')}
                      </Button>
                    </div>
                    <ul className="max-h-64 space-y-2 overflow-y-auto rounded-md border border-gray-200 p-2 custom-scrollbar" aria-label={t('portable.export.listLabel')}>
                      {candidates.map((template) => (
                        <li key={template.id}>
                          <label className="flex cursor-pointer items-start gap-3 rounded-md p-2 hover:bg-gray-50">
                            <input
                              type="checkbox"
                              className="mt-1 h-4 w-4 rounded border-gray-300"
                              checked={selectedTemplateIds.has(template.id)}
                              onChange={() => toggleExportTemplate(template.id)}
                              disabled={exportBusy}
                            />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-sm font-medium text-gray-900">{template.name}</span>
                              <span className="block truncate font-mono text-xs text-gray-500">{template.id}</span>
                            </span>
                            <span className="text-xs text-gray-500">v{template.version}</span>
                          </label>
                        </li>
                      ))}
                    </ul>

                    {exportPreview && (
                      <div className="rounded-md border border-gray-200 p-3 text-sm text-gray-700" role="status">
                        <div className="flex items-center gap-1 font-medium">{t('portable.export.previewReady')}<HelpHint text={t('portable.export.tokenNotice')} /></div>
                        <p className="mt-1">
                          {t('portable.export.previewSummary', {
                            count: exportPreview.templateCount,
                            size: formatPortableBytes(exportPreview.estimatedUncompressedBytes, locale),
                          })}
                        </p>
                      </div>
                    )}
                    {exportError && <PortableErrorPanel error={exportError} />}
                    <div className="flex flex-wrap justify-end gap-2">
                      {exportPreview ? (
                        <>
                          <Button type="button" variant="outline" onClick={previewExport} disabled={exportBusy}>
                            <RefreshCw aria-hidden="true" />
                            {t('portable.export.refreshPreview')}
                          </Button>
                          <Button type="button" onClick={exportPack} disabled={exportBusy}>
                            {exportStage === 'exporting' ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <Download aria-hidden="true" />}
                            {exportStage === 'exporting' ? t('portable.export.exporting') : t('portable.export.chooseDestination')}
                          </Button>
                        </>
                      ) : (
                        <Button type="button" onClick={previewExport} disabled={exportBusy || selectedTemplateIds.size === 0}>
                          {exportStage === 'previewing' ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <ShieldCheck aria-hidden="true" />}
                          {exportStage === 'previewing' ? t('portable.export.previewing') : t('portable.export.preview')}
                        </Button>
                      )}
                    </div>
                  </>
                )}
                {exportStage === 'complete' && (
                  <div className="flex justify-end">
                    <Button type="button" variant="outline" onClick={resetExport}>
                      {t('portable.export.another')}
                    </Button>
                  </div>
                )}
              </section>
            </TabsContent>

            <TabsContent value="import" className="max-h-[62vh] min-h-0 overflow-y-auto pr-1 custom-scrollbar">
              <section className="space-y-4" aria-labelledby="portable-import-title">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <h3 id="portable-import-title" className="font-semibold text-gray-900">
                      {t('portable.import.title')}
                    </h3>
                    <p className="mt-1 text-sm text-gray-600">{t('portable.import.description')}</p>
                  </div>
                  <Button type="button" variant="outline" onClick={chooseImportPack} disabled={importBusy}>
                    {importStage === 'previewing' ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <Upload aria-hidden="true" />}
                    {sourceFileName ? t('portable.import.chooseAnother') : t('portable.import.chooseFile')}
                  </Button>
                </div>

                {sourceFileName && (
                  <div className="rounded-md border border-gray-200 bg-gray-50 p-3">
                    <div className="flex items-center gap-1 text-xs font-medium text-gray-500">{t('portable.import.selectedFile')}<HelpHint text={t('portable.import.pathHidden')} /></div>
                    <p className="mt-1 break-all text-sm font-medium text-gray-900">{sourceFileName}</p>
                  </div>
                )}

                {importStage === 'previewing' && <PortableLoading label={t('portable.import.previewing')} />}

                {importPreview && importStage !== 'complete' && (
                  <>
                    <div className="grid gap-2 sm:grid-cols-4" aria-label={t('portable.import.conflictSummary')}>
                      <Metric label={t('portable.conflicts.none')} value={conflictCounts?.none ?? 0} />
                      <Metric label={t('portable.conflicts.custom')} value={conflictCounts?.custom ?? 0} tone="amber" />
                      <Metric label={t('portable.conflicts.readonly')} value={conflictCounts?.readonly ?? 0} tone="violet" />
                      <Metric label={t('portable.import.totalSize')} value={formatPortableBytes(importPreview.totalUncompressedBytes, locale)} />
                    </div>

                    <div className="rounded-md border border-gray-200">
                      <div className="border-b border-gray-200 bg-gray-50 px-3 py-2 text-sm font-medium">
                        {t('portable.import.reviewTitle', { count: importPreview.items.length })}
                      </div>
                      <ul className="max-h-72 divide-y divide-gray-200 overflow-y-auto custom-scrollbar">
                        {importPreview.items.map((item) => (
                          <PortableImportItemRow
                            key={item.itemId}
                            item={item}
                            decision={importDecisions[item.itemId]}
                            disabled={importBusy || !!importPlan}
                            onDecision={(strategy) => setImportDecision(item, strategy)}
                            onApplyToKind={(strategy) => applyDecisionToConflictKind(item.conflictKind, strategy)}
                          />
                        ))}
                      </ul>
                    </div>

                    {unresolvedItems.length > 0 && !importPlan && (
                      <div className="flex items-start gap-2 py-2 text-sm text-amber-800" role="status">
                        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                        <span>{t('portable.import.unresolved', { count: unresolvedItems.length })}</span>
                      </div>
                    )}
                  </>
                )}

                {importPlan && importStage !== 'complete' && importStage !== 'cancelled' && (
                  <div className="space-y-3 rounded-md border border-gray-200 p-3">
                    <div className="flex items-start gap-2 text-gray-700">
                      <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                      <div>
                        <div className="flex items-center gap-1 font-medium">{t('portable.import.planReady')}<HelpHint text={t('portable.import.planNotice')} /></div>
                      </div>
                    </div>
                    <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                      <Metric label={t('portable.operations.create')} value={importPlan.summary.createCount} />
                      <Metric label={t('portable.operations.replace')} value={importPlan.summary.replaceCount} tone="amber" />
                      <Metric label={t('portable.operations.keepBoth')} value={importPlan.summary.transformedCount} tone="violet" />
                      <Metric label={t('portable.operations.skip')} value={importPlan.summary.skipCount} />
                    </div>
                    <p className="text-xs text-blue-800">
                      {t('portable.import.expiresAt', { date: formatPortableDate(importPlan.expiresAt, locale) })}
                    </p>
                  </div>
                )}

                {(importStage === 'executing' || importStage === 'cancelling') && (
                  <PortableLoading label={importStage === 'cancelling'
                    ? t('portable.import.cancelling')
                    : t('portable.import.executing')} />
                )}

                {importStage === 'cancelled' && (
                  <div className="flex items-start gap-2 py-2 text-sm text-amber-800" role="status">
                    <XCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                    <div>
                      <p className="font-medium">{t('portable.import.cancelledTitle')}</p>
                      <p className="mt-1">{t('portable.import.cancelledDescription')}</p>
                    </div>
                  </div>
                )}

                {importStage === 'complete' && importResult && (
                  <>
                    <PortableSuccessPanel
                      title={t('portable.import.completeTitle')}
                      description={t('portable.import.completeDescription', {
                        create: importResult.summary.createCount,
                        replace: importResult.summary.replaceCount,
                        keepBoth: importResult.summary.transformedCount,
                        skip: importResult.summary.skipCount,
                      })}
                      packageId={importResult.package.packageId}
                      archiveSha256={importResult.package.archiveSha256}
                    />
                    <div className="grid grid-cols-3 gap-2">
                      <Metric label={t('portable.recovery.rolledBack')} value={importResult.recovery.rolledBackTransactions} />
                      <Metric label={t('portable.recovery.finalized')} value={importResult.recovery.finalizedTransactions} />
                      <Metric label={t('portable.recovery.cleaned')} value={importResult.recovery.cleanedStagingDirectories} />
                    </div>
                  </>
                )}

                {importError && <PortableErrorPanel error={importError} />}
                {cancelError && <PortableErrorPanel error={cancelError} />}

                <div className="flex flex-wrap justify-end gap-2">
                  {importStage === 'review' && importPreview && (
                    <Button type="button" onClick={planImport} disabled={unresolvedItems.length > 0}>
                      <ShieldCheck aria-hidden="true" />
                      {t('portable.import.buildPlan')}
                    </Button>
                  )}
                  {importStage === 'planning' && (
                    <Button type="button" disabled>
                      <LoaderCircle className="animate-spin" aria-hidden="true" />
                      {t('portable.import.planning')}
                    </Button>
                  )}
                  {importStage === 'planned' && importPlan && (
                    <Button type="button" onClick={executeImport}>
                      <Upload aria-hidden="true" />
                      {t('portable.import.execute')}
                    </Button>
                  )}
                  {importStage === 'executing' && (
                    <Button type="button" variant="destructive" onClick={cancelImport}>
                      <XCircle aria-hidden="true" />
                      {t('portable.import.cancel')}
                    </Button>
                  )}
                  {importStage === 'cancelling' && (
                    <Button type="button" variant="destructive" disabled>
                      <LoaderCircle className="animate-spin" aria-hidden="true" />
                      {t('portable.import.cancelling')}
                    </Button>
                  )}
                  {(importStage === 'cancelled'
                    || importStage === 'failed'
                    || (importError && shouldRestartPortablePreview(importError.code)))
                    && sourcePathRef.current && (
                    <Button type="button" variant="outline" onClick={restartImportPreview} disabled={importBusy}>
                      <RefreshCw aria-hidden="true" />
                      {t('portable.import.previewAgain')}
                    </Button>
                  )}
                  {importStage === 'complete' && (
                    <Button type="button" variant="outline" onClick={resetImport}>
                      {t('portable.import.another')}
                    </Button>
                  )}
                </div>
              </section>
            </TabsContent>
          </Tabs>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={requestClose} disabled={busy}>
              {t('portable.close')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={discardOpen} onOpenChange={setDiscardOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('portable.leave.title')}</DialogTitle>
            <DialogDescription>{t('portable.leave.description')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDiscardOpen(false)}>
              {t('portable.leave.continue')}
            </Button>
            <Button type="button" variant="destructive" onClick={resetAndClose}>
              {t('portable.leave.discard')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function PortableImportItemRow({
  item,
  decision,
  disabled,
  onDecision,
  onApplyToKind,
}: {
  item: PortablePackImportItem;
  decision?: PortablePackConflictStrategy;
  disabled: boolean;
  onDecision: (strategy: PortablePackConflictStrategy) => void;
  onApplyToKind: (strategy: PortablePackConflictStrategy) => void;
}) {
  const { t } = useTranslation('templates');
  const automatic = item.conflictKind === 'none';
  return (
    <li className="space-y-2 p-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium text-gray-900">{item.template.name}</p>
          <p className="truncate font-mono text-xs text-gray-500">{item.template.id}</p>
        </div>
        <span className={`rounded-full px-2 py-0.5 text-xs font-medium ${automatic ? 'bg-emerald-100 text-emerald-800' : 'bg-amber-100 text-amber-900'}`}>
          {t(`portable.conflictKinds.${item.conflictKind}`)}
        </span>
      </div>
      {automatic ? (
        <p className="text-xs text-emerald-700">{t('portable.import.autoCreate')}</p>
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Select value={decision} onValueChange={(value) => onDecision(value as PortablePackConflictStrategy)} disabled={disabled}>
            <SelectTrigger className="min-w-56 flex-1" aria-label={t('portable.import.decisionLabel', { name: item.template.name })}>
              <SelectValue placeholder={t('portable.import.chooseDecision')} />
            </SelectTrigger>
            <SelectContent>
              {item.allowedStrategies.map((strategy) => (
                <SelectItem key={strategy} value={strategy}>{t(`portable.strategies.${strategy}`)}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {decision && (
            <Button type="button" variant="ghost" size="sm" onClick={() => onApplyToKind(decision)} disabled={disabled}>
              {t('portable.import.applySameKind')}
            </Button>
          )}
        </div>
      )}
      {item.existing && (
        <p className="text-xs text-gray-500">
          {t('portable.import.existingVersion', { version: item.existing.version })}
        </p>
      )}
    </li>
  );
}

function PortableErrorPanel({ error }: { error: TemplateApiError }) {
  const { t } = useTranslation('templates');
  return (
    <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
      <div className="min-w-0">
        <p>{t(templateTranslationKey(error.messageKey), { defaultValue: t('errors.io') })}</p>
        <p className="mt-1 select-all break-all font-mono text-[11px] text-red-600">{error.debugId}</p>
      </div>
    </div>
  );
}

function PortableSuccessPanel({
  title,
  description,
  packageId,
  archiveSha256,
}: {
  title: string;
  description: string;
  packageId: string;
  archiveSha256: string;
}) {
  const { t } = useTranslation('templates');
  return (
    <div className="rounded-md border border-emerald-200 bg-emerald-50 p-4 text-emerald-950" role="status" aria-live="polite">
      <div className="flex items-start gap-2">
        <CheckCircle2 className="mt-0.5 h-5 w-5 shrink-0 text-emerald-700" aria-hidden="true" />
        <div>
          <p className="font-semibold">{title}</p>
          <p className="mt-1 text-sm">{description}</p>
        </div>
      </div>
      <dl className="mt-3 grid gap-2 text-xs sm:grid-cols-2">
        <div>
          <dt className="text-emerald-700">{t('portable.packageId')}</dt>
          <dd className="mt-0.5 break-all font-mono">{packageId}</dd>
        </div>
        <div>
          <dt className="text-emerald-700">{t('portable.archiveSha256')}</dt>
          <dd className="mt-0.5 break-all font-mono">{archiveSha256}</dd>
        </div>
      </dl>
    </div>
  );
}

function PortableLoading({ label }: { label: string }) {
  return (
    <div className="flex items-center justify-center gap-3 rounded-md border border-blue-200 bg-blue-50 px-4 py-8 text-sm text-blue-900" role="status" aria-live="polite">
      <LoaderCircle className="h-5 w-5 animate-spin" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

function EmptyPortableState({ title, description }: { title: string; description: string }) {
  return (
    <div className="rounded-md border border-dashed border-gray-300 p-6 text-center">
      <PackageOpen className="mx-auto h-8 w-8 text-gray-400" aria-hidden="true" />
      <p className="mt-3 font-medium text-gray-900">{title}</p>
      <p className="mt-1 text-sm text-gray-600">{description}</p>
    </div>
  );
}

function Metric({
  label,
  value,
  tone = 'gray',
}: {
  label: string;
  value: string | number;
  tone?: 'gray' | 'amber' | 'violet';
}) {
  const tones = {
    gray: 'border-gray-200 bg-gray-50 text-gray-900',
    amber: 'border-amber-200 bg-amber-50 text-amber-950',
    violet: 'border-violet-200 bg-violet-50 text-violet-950',
  };
  return (
    <div className={`rounded-md border p-2 ${tones[tone]}`}>
      <p className="text-xs opacity-70">{label}</p>
      <p className="mt-1 text-sm font-semibold">{value}</p>
    </div>
  );
}

function formatPortableDate(value: string, locale: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}
