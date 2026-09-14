'use client';

import { useMemo, useRef, useState, type MouseEvent } from 'react';
import { useRouter } from 'next/navigation';
import { useVirtualizer } from '@tanstack/react-virtual';
import { save } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import {
  AlertCircle,
  ArrowLeft,
  Check,
  Copy,
  Download,
  FolderOpen,
  LayoutTemplate,
  MoreHorizontal,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
  TriangleAlert,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { useMeetilyI18n } from '@/i18n/I18nProvider';
import { useTemplateLibrary } from '@/hooks/useTemplateLibrary';
import { localizedLabelForCode } from '@/lib/summary-languages';
import { TemplateImportDialog } from '@/components/templates/TemplateImportDialog';
import { PortableTemplatePackDialog } from '@/components/templates/PortableTemplatePackDialog';
import { normalizeTemplateApiError, templateService } from '@/services/templateService';
import {
  countTemplateViews,
  filterDeletedTemplates,
  filterTemplates,
  stopTemplateCardActionPropagation,
  templateErrorToastId,
  templateTranslationKey,
  type TemplateLibraryView,
} from '@/lib/template-library';
import type {
  DeletedTemplateListItem,
  TemplateApiError,
  TemplateDetails,
  TemplateListItem,
} from '@/types/summary-template';

interface DuplicateDialogState {
  template: TemplateListItem;
  name: string;
  error: TemplateApiError | null;
}

interface DeleteDialogState {
  template: TemplateListItem;
  replacementId: string;
  error: TemplateApiError | null;
}

interface PreviewDialogState {
  item: TemplateListItem;
  details: TemplateDetails | null;
  error: TemplateApiError | null;
}

function preventCardAction(event: MouseEvent<HTMLElement>) {
  stopTemplateCardActionPropagation(event.nativeEvent);
}

function originLabelKey(origin: TemplateListItem['origin']) {
  if (origin === 'custom') return 'card.custom' as const;
  if (origin === 'bundled') return 'card.bundled' as const;
  return 'card.builtin' as const;
}

export function TemplateLibraryPage() {
  const router = useRouter();
  const { t, i18n } = useTranslation('templates');
  const { locale } = useMeetilyI18n();
  const {
    data,
    directory,
    loading,
    error,
    pendingOperations,
    refresh,
    getDetails,
    openDirectory,
    setDefault,
    duplicate,
    deleteTemplate,
    restore,
    purge,
  } = useTemplateLibrary(undefined, i18n.resolvedLanguage ?? i18n.language);
  const [view, setView] = useState<TemplateLibraryView>('all');
  const [query, setQuery] = useState('');
  const [previewDialog, setPreviewDialog] = useState<PreviewDialogState | null>(null);
  const [duplicateDialog, setDuplicateDialog] = useState<DuplicateDialogState | null>(null);
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState | null>(null);
  const [exportingTemplateId, setExportingTemplateId] = useState<string | null>(null);
  const [restoreConflict, setRestoreConflict] = useState<{
    item: DeletedTemplateListItem;
    error: TemplateApiError;
  } | null>(null);
  // 回收站里的"永久删除"：先确认再删，删完文件直接从 .trash 里消失
  const [purgeDialog, setPurgeDialog] = useState<DeletedTemplateListItem | null>(null);
  const [purgeError, setPurgeError] = useState<TemplateApiError | null>(null);

  const templates = useMemo(() => data?.templates ?? [], [data?.templates]);
  const deletedTemplates = useMemo(
    () => data?.deletedTemplates ?? [],
    [data?.deletedTemplates],
  );
  const counts = useMemo(
    () => countTemplateViews(templates, deletedTemplates),
    [deletedTemplates, templates],
  );
  const visibleTemplates = useMemo(
    () => view === 'trash' ? [] : filterTemplates(templates, view, query),
    [query, templates, view],
  );
  const visibleDeletedTemplates = useMemo(
    () => filterDeletedTemplates(deletedTemplates, query),
    [deletedTemplates, query],
  );
  const validReplacementTemplates = useMemo(
    () => templates.filter((item) => item.valid && !item.isDefault),
    [templates],
  );

  const listScrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: visibleTemplates.length,
    getScrollElement: () => listScrollRef.current,
    estimateSize: () => 146,
    overscan: 6,
  });

  const showOperationError = (operation: string, caught: unknown) => {
    const normalized = normalizeTemplateApiError(caught);
    toast.error(t(templateTranslationKey(normalized.messageKey), {
      defaultValue: t('errors.io'),
    }), {
      id: templateErrorToastId(operation, normalized),
      description: normalized.debugId,
    });
  };

  const handleOpenDirectory = async () => {
    try {
      await openDirectory();
      toast.success(t('actions.folderOpened'));
    } catch (caught) {
      showOperationError('open-directory', caught);
    }
  };

  const handleSetDefault = async (item: TemplateListItem) => {
    if (item.isDefault || !item.valid) return;
    try {
      await setDefault(item.id);
      toast.success(t('actions.defaultSuccess'));
    } catch (caught) {
      showOperationError('set-default', caught);
    }
  };

  const handleExport = async (item: TemplateListItem) => {
    if (!item.valid || exportingTemplateId) return;
    try {
      const destinationPath = await save({
        title: t('export.filePickerTitle'),
        defaultPath: `${item.id}.json`,
        filters: [{ name: t('export.jsonFiles'), extensions: ['json'] }],
      });
      if (!destinationPath) return;
      const jsonDestinationPath = destinationPath.toLocaleLowerCase().endsWith('.json')
        ? destinationPath
        : `${destinationPath}.json`;
      setExportingTemplateId(item.id);
      const result = await templateService.exportJson({
        templateId: item.id,
        origin: item.origin,
        contentLocale: i18n.resolvedLanguage ?? i18n.language,
        destinationPath: jsonDestinationPath,
      });
      toast.success(t('export.success', { fileName: result.fileName }));
    } catch (caught) {
      showOperationError(`export:${item.id}`, caught);
    } finally {
      setExportingTemplateId(null);
    }
  };

  const openPreview = async (item: TemplateListItem) => {
    setPreviewDialog({ item, details: null, error: null });
    try {
      const details = await getDetails(item.id, item.origin);
      if (details) setPreviewDialog((current) => current?.item.id === item.id
        ? { ...current, details }
        : current);
    } catch (caught) {
      const normalized = normalizeTemplateApiError(caught);
      setPreviewDialog((current) => current?.item.id === item.id
        ? { ...current, error: normalized }
        : current);
    }
  };

  const confirmDuplicate = async () => {
    if (!duplicateDialog || !duplicateDialog.name.trim()) return;
    const current = duplicateDialog;
    setDuplicateDialog({ ...current, error: null });
    try {
      const result = await duplicate({
        templateId: current.template.id,
        origin: current.template.origin,
        newName: current.name.trim(),
      });
      if (result) {
        setDuplicateDialog(null);
        toast.success(t('duplicate.success'));
        router.push(`/settings/templates/editor?mode=edit&id=${encodeURIComponent(result.template.id)}&origin=custom`);
      }
    } catch (caught) {
      setDuplicateDialog((dialog) => dialog
        ? { ...dialog, error: normalizeTemplateApiError(caught) }
        : dialog);
    }
  };

  const confirmDelete = async () => {
    if (!deleteDialog) return;
    const current = deleteDialog;
    if (current.template.isDefault && !current.replacementId) return;
    setDeleteDialog({ ...current, error: null });
    try {
      const result = await deleteTemplate({
        templateId: current.template.id,
        expectedFileSha256: current.template.fileSha256,
        replacementDefaultTemplateId: current.template.isDefault
          ? current.replacementId
          : undefined,
      });
      if (result) {
        setDeleteDialog(null);
        toast.success(t('delete.success'));
      }
    } catch (caught) {
      setDeleteDialog((dialog) => dialog
        ? { ...dialog, error: normalizeTemplateApiError(caught) }
        : dialog);
    }
  };

  const handleRestore = async (item: DeletedTemplateListItem) => {
    try {
      const result = await restore({ trashId: item.trashId, conflictPolicy: 'error' });
      if (result) toast.success(t('trash.success'));
    } catch (caught) {
      const normalized = normalizeTemplateApiError(caught);
      if (normalized.code === 'TEMPLATE_ALREADY_EXISTS' || normalized.code === 'TEMPLATE_CONFLICT') {
        setRestoreConflict({ item, error: normalized });
      } else {
        showOperationError('restore', normalized);
      }
    }
  };

  const confirmPurge = async () => {
    if (!purgeDialog) return;
    setPurgeError(null);
    try {
      await purge(purgeDialog.trashId);
      setPurgeDialog(null);
      toast.success(t('trash.purgeSuccess'));
    } catch (caught) {
      setPurgeError(normalizeTemplateApiError(caught));
    }
  };

  const emptySearch = query.trim().length > 0;
  // 内置模板现在会落成普通自定义模板，不再单列"内置"类别；该分类为空时直接隐藏这个标签
  const tabs: TemplateLibraryView[] = (['all', 'custom', 'builtin', 'trash'] as TemplateLibraryView[])
    .filter((tab) => tab !== 'builtin' || counts.builtin > 0);

  return (
    <div className="flex h-screen min-w-0 flex-col bg-gray-50 text-gray-900">
      <header className="shrink-0 border-b border-gray-200 bg-white">
        <div className="mx-auto flex w-full max-w-7xl items-center gap-4 px-4 py-4 sm:px-6">
          <Button type="button" variant="ghost" size="sm" onClick={() => router.push('/settings')}>
            <ArrowLeft aria-hidden="true" />
            {t('library.backToSettings')}
          </Button>
          <div className="min-w-0">
            <h1 className="truncate text-2xl font-bold">{t('library.title')}</h1>
            <p className="hidden text-sm text-gray-600 sm:block">{t('library.description')}</p>
          </div>
        </div>
      </header>

      <main className="mx-auto flex min-h-0 w-full max-w-7xl flex-1 flex-col gap-4 px-4 py-4 sm:px-6">
        <div className="flex flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <label className="relative block min-w-0 flex-1 lg:max-w-xl">
            <span className="sr-only">{t('library.search')}</span>
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-gray-400" aria-hidden="true" />
            <Input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t('library.search')}
              className="pl-9 pr-20"
            />
            {query && (
              <button
                type="button"
                onClick={() => setQuery('')}
                className="absolute right-3 top-1/2 -translate-y-1/2 text-xs font-medium text-gray-500 hover:text-gray-900 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
              >
                {t('library.clearSearch')}
              </button>
            )}
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <TemplateImportDialog
              disabled={loading || directory?.writable === false}
              onImported={refresh}
            />
            <Button type="button" onClick={() => router.push('/settings/templates/editor?mode=create')}>
              <Plus aria-hidden="true" />
              {t('library.newTemplate')}
            </Button>
          </div>
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-gray-200">
          <div className="flex max-w-full gap-1 overflow-x-auto" role="tablist" aria-label={t('library.title')}>
            {tabs.map((tab) => (
              <button
                key={tab}
                type="button"
                role="tab"
                aria-selected={view === tab}
                onClick={() => setView(tab)}
                className={`whitespace-nowrap border-b-2 px-3 py-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 ${
                  view === tab
                    ? 'border-blue-600 text-blue-700'
                    : 'border-transparent text-gray-600 hover:text-gray-900'
                }`}
              >
                {t(`library.tabs.${tab}`)} ({counts[tab]})
              </button>
            ))}
          </div>
          <div className="mb-2 flex items-center gap-1">
            {/* P2-20：迁移模板包属于内部/低频操作，放到次级工具行，不和「新建模板」并排 */}
            <PortableTemplatePackDialog
              disabled={loading || directory?.writable === false}
              templates={data?.templates ?? []}
              onImported={refresh}
            />
            <Button type="button" variant="ghost" size="sm" onClick={() => void refresh()} disabled={loading}>
              <RefreshCw className={loading ? 'animate-spin' : ''} aria-hidden="true" />
              {t('library.refresh')}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => void handleOpenDirectory()}
              disabled={pendingOperations.has('open-directory')}
            >
              <FolderOpen aria-hidden="true" />
              {t('library.openFolder')}
            </Button>
          </div>
        </div>

        {directory && !directory.writable && (
          <div className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="status">
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>{t('library.directoryReadOnly')}</span>
          </div>
        )}

        {!!data?.diagnostics.length && (
          <div className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="status">
            <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>{t('library.diagnostics', { count: data.diagnostics.length })}</span>
          </div>
        )}

        {error && data && (
          <div className="flex flex-wrap items-center gap-2 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
            <AlertCircle className="h-4 w-4 shrink-0" aria-hidden="true" />
            <span className="flex-1">{t(templateTranslationKey(error.messageKey))}</span>
            {error.retryable && (
              <Button type="button" variant="outline" size="sm" onClick={() => void refresh()}>
                {t('library.retry')}
              </Button>
            )}
          </div>
        )}

        {loading && !data ? (
          <LoadingState label={t('library.loading')} />
        ) : error && !data ? (
          <PageErrorState
            title={t('library.loadFailed')}
            detail={t(templateTranslationKey(error.messageKey), {
              defaultValue: t('errors.io'),
            })}
            retryable={error.retryable}
            retryLabel={t('library.retry')}
            onRetry={() => void refresh()}
          />
        ) : view === 'trash' ? (
          <div className="min-h-0 flex-1 overflow-y-auto pb-6 custom-scrollbar" role="tabpanel">
            {visibleDeletedTemplates.length === 0 ? (
              <EmptyState
                search={emptySearch}
                query={query}
                title={t('library.empty.trashTitle')}
                description={t('library.empty.trashDescription')}
                searchTitle={t('library.empty.searchTitle', { query })}
                searchDescription={t('library.empty.searchDescription')}
                clearLabel={t('library.clearSearch')}
                onClear={() => setQuery('')}
              />
            ) : (
              <ul className="space-y-3" aria-label={t('library.tabs.trash')}>
                {visibleDeletedTemplates.map((item) => (
                  <li key={item.trashId} className="flex flex-col gap-4 rounded-lg border border-gray-200 bg-white p-4 shadow-sm sm:flex-row sm:items-center sm:justify-between">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <h2 className="truncate font-semibold">{item.name}</h2>
                        {!item.valid && <Badge tone="red">{t('card.invalid')}</Badge>}
                      </div>
                      <p className="mt-1 truncate text-sm text-gray-500">{item.originalTemplateId}</p>
                      <p className="mt-1 text-xs text-gray-400">
                        {t('trash.deletedAt', { date: formatDate(item.deletedAt, locale) })}
                      </p>
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        onClick={() => void handleRestore(item)}
                        disabled={pendingOperations.has(`restore:${item.trashId}`)}
                      >
                        <RotateCcw className={pendingOperations.has(`restore:${item.trashId}`) ? 'animate-spin' : ''} aria-hidden="true" />
                        {pendingOperations.has(`restore:${item.trashId}`) ? t('trash.restoring') : t('trash.restore')}
                      </Button>
                      {/* 永久删除：模板大多是按自己需求定制的，回收站里留着的多半是废稿 */}
                      <Button
                        type="button"
                        variant="outline"
                        className="text-red-600 hover:text-red-700"
                        onClick={() => { setPurgeError(null); setPurgeDialog(item); }}
                        disabled={pendingOperations.has(`purge:${item.trashId}`)}
                        aria-label={t('trash.purgeAria', { name: item.name })}
                      >
                        <Trash2 className={pendingOperations.has(`purge:${item.trashId}`) ? 'animate-pulse' : ''} aria-hidden="true" />
                        {pendingOperations.has(`purge:${item.trashId}`) ? t('trash.purging') : t('trash.purge')}
                      </Button>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>
        ) : visibleTemplates.length === 0 ? (
          <EmptyState
            search={emptySearch}
            query={query}
            title={t('library.empty.title')}
            description={t('library.empty.description')}
            searchTitle={t('library.empty.searchTitle', { query })}
            searchDescription={t('library.empty.searchDescription')}
            clearLabel={t('library.clearSearch')}
            onClear={() => setQuery('')}
          />
        ) : (
          <div
            ref={listScrollRef}
            className="min-h-0 flex-1 overflow-y-auto pb-6 custom-scrollbar"
            role="tabpanel"
            aria-label={t(`library.tabs.${view}`)}
          >
            <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const item = visibleTemplates[virtualRow.index];
                return (
                  <div
                    key={item.id}
                    ref={virtualizer.measureElement}
                    data-index={virtualRow.index}
                    className="absolute left-0 top-0 w-full pb-3"
                    style={{ transform: `translateY(${virtualRow.start}px)` }}
                  >
                    <TemplateCard
                      item={item}
                      locale={locale}
                      pendingOperations={pendingOperations}
                      onPreview={() => void openPreview(item)}
                      onSetDefault={() => void handleSetDefault(item)}
                      onDuplicate={() => setDuplicateDialog({
                        template: item,
                        name: `${item.name} Copy`,
                        error: null,
                      })}
                      onEdit={() => router.push(`/settings/templates/editor?mode=edit&id=${encodeURIComponent(item.id)}&origin=custom`)}
                      onDelete={() => setDeleteDialog({ template: item, replacementId: '', error: null })}
                      onExport={() => void handleExport(item)}
                      exporting={exportingTemplateId === item.id}
                      onOpenDirectory={() => void handleOpenDirectory()}
                    />
                  </div>
                );
              })}
            </div>
          </div>
        )}
      </main>

      <PreviewDialog state={previewDialog} onOpenChange={(open) => !open && setPreviewDialog(null)} />

      <Dialog open={purgeDialog !== null} onOpenChange={(open) => { if (!open) { setPurgeDialog(null); setPurgeError(null); } }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('trash.purgeTitle', { name: purgeDialog?.name ?? '' })}</DialogTitle>
            <DialogDescription>{t('trash.purgeDescription')}</DialogDescription>
          </DialogHeader>
          {purgeError && <InlineOperationError error={purgeError} />}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => { setPurgeDialog(null); setPurgeError(null); }}>
              {t('trash.purgeCancel')}
            </Button>
            <Button
              type="button"
              className="bg-red-600 text-white hover:bg-red-700"
              onClick={() => void confirmPurge()}
              disabled={!purgeDialog || pendingOperations.has(`purge:${purgeDialog.trashId}`)}
            >
              <Trash2 aria-hidden="true" />
              {t('trash.purgeConfirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={duplicateDialog !== null} onOpenChange={(open) => !open && setDuplicateDialog(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('duplicate.title', { name: duplicateDialog?.template.name ?? '' })}</DialogTitle>
            <DialogDescription>{t('duplicate.description')}</DialogDescription>
          </DialogHeader>
          <label className="space-y-2 text-sm font-medium">
            <span>{t('duplicate.nameLabel')}</span>
            <Input
              autoFocus
              value={duplicateDialog?.name ?? ''}
              onChange={(event) => setDuplicateDialog((dialog) => dialog ? { ...dialog, name: event.target.value } : dialog)}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && duplicateDialog?.name.trim()) void confirmDuplicate();
              }}
            />
          </label>
          {duplicateDialog && !duplicateDialog.name.trim() && (
            <p className="text-sm text-red-600" role="alert">{t('duplicate.nameRequired')}</p>
          )}
          {duplicateDialog?.error && (
            <InlineOperationError error={duplicateDialog.error} />
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDuplicateDialog(null)}>
              {t('duplicate.cancel')}
            </Button>
            <Button
              type="button"
              onClick={() => void confirmDuplicate()}
              disabled={!duplicateDialog?.name.trim() || pendingOperations.has(`duplicate:${duplicateDialog.template.id}`)}
            >
              <Copy aria-hidden="true" />
              {t('duplicate.confirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={deleteDialog !== null} onOpenChange={(open) => !open && setDeleteDialog(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('delete.title', { name: deleteDialog?.template.name ?? '' })}</DialogTitle>
            <DialogDescription>{t('delete.description')}</DialogDescription>
          </DialogHeader>
          {deleteDialog?.template.isDefault && (
            <div className="space-y-3 rounded-md border border-amber-200 bg-amber-50 p-3">
              <p className="text-sm text-amber-900">{t('delete.defaultWarning')}</p>
              {validReplacementTemplates.length > 0 ? (
                <label className="block space-y-2 text-sm font-medium">
                  <span>{t('delete.replacementLabel')}</span>
                  <Select
                    value={deleteDialog.replacementId || undefined}
                    onValueChange={(replacementId) => setDeleteDialog((dialog) => dialog ? { ...dialog, replacementId } : dialog)}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder={t('delete.replacementPlaceholder')} />
                    </SelectTrigger>
                    <SelectContent>
                      {validReplacementTemplates.map((template) => (
                        <SelectItem key={`${template.origin}:${template.id}`} value={template.id}>
                          {template.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </label>
              ) : (
                <p className="text-sm font-medium text-red-700" role="alert">{t('delete.noReplacement')}</p>
              )}
            </div>
          )}
          {deleteDialog?.error && <InlineOperationError error={deleteDialog.error} />}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDeleteDialog(null)}>
              {t('delete.cancel')}
            </Button>
            <Button
              type="button"
              variant="destructive"
              onClick={() => void confirmDelete()}
              disabled={
                !deleteDialog ||
                (deleteDialog.template.isDefault && !deleteDialog.replacementId) ||
                pendingOperations.has(`delete:${deleteDialog?.template.id}`)
              }
            >
              <Trash2 aria-hidden="true" />
              {t('delete.confirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={restoreConflict !== null} onOpenChange={(open) => !open && setRestoreConflict(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('trash.conflictTitle')}</DialogTitle>
            <DialogDescription>{t('trash.conflictDescription')}</DialogDescription>
          </DialogHeader>
          <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="alert">
            <p className="font-medium">{restoreConflict?.item.name}</p>
            <p className="mt-1">{t('trash.conflict')}</p>
            <p className="mt-1 select-all font-mono text-xs">{restoreConflict?.error.debugId}</p>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setRestoreConflict(null)}>
              {t('trash.close')}
            </Button>
            <Button type="button" onClick={() => void handleOpenDirectory()}>
              <FolderOpen aria-hidden="true" />
              {t('trash.openFolder')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function TemplateCard({
  item,
  locale,
  pendingOperations,
  onPreview,
  onSetDefault,
  onDuplicate,
  onEdit,
  onDelete,
  onExport,
  exporting,
  onOpenDirectory,
}: {
  item: TemplateListItem;
  locale: string;
  pendingOperations: ReadonlySet<string>;
  onPreview: () => void;
  onSetDefault: () => void;
  onDuplicate: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onExport: () => void;
  exporting: boolean;
  onOpenDirectory: () => void;
}) {
  const { t } = useTranslation('templates');
  return (
    <div
      className="group flex min-h-[132px] flex-col gap-4 rounded-lg border border-gray-200 bg-white p-4 shadow-sm transition hover:border-blue-300 hover:shadow sm:flex-row sm:items-center sm:justify-between"
    >
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="truncate text-base font-semibold text-gray-900">{item.name}</h2>
          <Badge tone={item.origin === 'custom' ? 'blue' : 'gray'}>{t(originLabelKey(item.origin))}</Badge>
          {item.isDefault && <Badge tone="green"><Check aria-hidden="true" />{t('card.default')}</Badge>}
          {!item.valid && <Badge tone="red"><AlertCircle aria-hidden="true" />{t('card.invalid')}</Badge>}
          {item.readOnly && <Badge tone="gray">{t('card.readOnly')}</Badge>}
        </div>
        <p className="mt-2 line-clamp-2 text-sm text-gray-600">{item.description || item.id}</p>
        <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-gray-500">
          <span>{t('card.sections', { count: item.sectionCount })}</span>
          {/* P2-20：语言按界面语言本地化；内置/随应用提供的模板不显示"更新于 <占位时间>" */}
          {item.locale && <span>{localizedLabelForCode(item.locale, locale)}</span>}
          {item.origin === 'custom' && item.updatedAt && (
            <span>{t('card.updated', { date: formatDate(item.updatedAt, locale) })}</span>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2" onClick={(event) => event.stopPropagation()}>
        <Button type="button" variant="outline" size="sm" onClick={(event) => { preventCardAction(event); onPreview(); }}>
          {t('card.preview')}
        </Button>
        {!item.isDefault && item.valid && (
          <Button
            type="button"
            variant="blue"
            size="sm"
            onClick={(event) => { preventCardAction(event); onSetDefault(); }}
            disabled={pendingOperations.has('set-default')}
          >
            {t('card.use')}
          </Button>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('card.moreActions', { name: item.name })}
              onClick={preventCardAction}
            >
              <MoreHorizontal aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" onClick={(event) => event.stopPropagation()}>
            <DropdownMenuItem onSelect={onPreview}>{t('card.preview')}</DropdownMenuItem>
            {item.valid && !item.isDefault && (
              <DropdownMenuItem onSelect={onSetDefault}>{t('card.use')}</DropdownMenuItem>
            )}
            <DropdownMenuItem onSelect={onDuplicate} disabled={!item.valid}>
              <Copy aria-hidden="true" />
              {t('card.duplicate')}
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={onExport} disabled={!item.valid || exporting}>
              <Download aria-hidden="true" />
              {exporting ? t('card.exporting') : t('card.export')}
            </DropdownMenuItem>
            {item.origin === 'custom' && item.valid && (
              <DropdownMenuItem onSelect={onEdit}>
                <Pencil aria-hidden="true" />
                {t('card.edit')}
              </DropdownMenuItem>
            )}
            {item.origin === 'custom' && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem onSelect={onOpenDirectory}>
                  <FolderOpen aria-hidden="true" />
                  {t('card.openLocation')}
                </DropdownMenuItem>
                <DropdownMenuItem className="text-red-600 focus:text-red-700" onSelect={onDelete}>
                  <Trash2 aria-hidden="true" />
                  {t('card.delete')}
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  );
}

function PreviewDialog({
  state,
  onOpenChange,
}: {
  state: PreviewDialogState | null;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation('templates');
  return (
    <Dialog open={state !== null} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{state?.details?.template.name ?? state?.item.name ?? t('preview.title')}</DialogTitle>
          <DialogDescription>
            {state?.details?.template.description || state?.item.description || t('preview.noDescription')}
          </DialogDescription>
        </DialogHeader>
        {!state?.details && !state?.error && <LoadingState label={t('preview.loading')} compact />}
        {state?.error && (
          <div className="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
            {t(templateTranslationKey(state.error.messageKey), {
              defaultValue: t('errors.io'),
            })}
          </div>
        )}
        {state?.details && (
          <div>
            <h3 className="mb-3 text-sm font-semibold uppercase tracking-wide text-gray-500">{t('preview.sections')}</h3>
            <ol className="space-y-3">
              {state.details.template.sections.map((section, index) => (
                <li key={section.id} className="rounded-md border border-gray-200 bg-gray-50 p-3">
                  <div className="flex items-start justify-between gap-3">
                    <h4 className="font-medium">{index + 1}. {section.title}</h4>
                    <Badge tone={section.required ? 'blue' : 'gray'}>
                      {section.required ? t('preview.required') : t('preview.optional')}
                    </Badge>
                  </div>
                  <p className="mt-2 whitespace-pre-wrap text-sm text-gray-600">{section.instruction}</p>
                </li>
              ))}
            </ol>
          </div>
        )}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t('preview.close')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function InlineOperationError({ error }: { error: TemplateApiError }) {
  const { t } = useTranslation('templates');
  return (
    <div className="rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
      <p>{t(templateTranslationKey(error.messageKey), {
        defaultValue: t('errors.io'),
      })}</p>
      <p className="mt-1 select-all font-mono text-xs text-red-600">{error.debugId}</p>
    </div>
  );
}

function Badge({ children, tone }: { children: React.ReactNode; tone: 'blue' | 'green' | 'gray' | 'red' }) {
  const tones = {
    blue: 'border-blue-200 bg-blue-50 text-blue-700',
    green: 'border-emerald-200 bg-emerald-50 text-emerald-700',
    gray: 'border-gray-200 bg-gray-50 text-gray-600',
    red: 'border-red-200 bg-red-50 text-red-700',
  };
  return (
    <span className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-medium ${tones[tone]}`}>
      {children}
    </span>
  );
}

function LoadingState({ label, compact = false }: { label: string; compact?: boolean }) {
  return (
    <div className={`flex items-center justify-center gap-3 text-sm text-gray-600 ${compact ? 'py-8' : 'min-h-48 flex-1'}`} aria-live="polite">
      <RefreshCw className="h-5 w-5 animate-spin" aria-hidden="true" />
      <span>{label}</span>
    </div>
  );
}

function PageErrorState({
  title,
  detail,
  retryable,
  retryLabel,
  onRetry,
}: {
  title: string;
  detail: string;
  retryable: boolean;
  retryLabel: string;
  onRetry: () => void;
}) {
  return (
    <div className="flex min-h-48 flex-1 flex-col items-center justify-center rounded-lg border border-red-200 bg-red-50 p-6 text-center" role="alert">
      <AlertCircle className="h-8 w-8 text-red-600" aria-hidden="true" />
      <h2 className="mt-3 font-semibold text-red-900">{title}</h2>
      <p className="mt-1 max-w-lg text-sm text-red-700">{detail}</p>
      {retryable && <Button type="button" variant="outline" className="mt-4" onClick={onRetry}>{retryLabel}</Button>}
    </div>
  );
}

function EmptyState({
  search,
  title,
  description,
  searchTitle,
  searchDescription,
  clearLabel,
  onClear,
}: {
  search: boolean;
  query: string;
  title: string;
  description: string;
  searchTitle: string;
  searchDescription: string;
  clearLabel: string;
  onClear: () => void;
}) {
  return (
    <div className="flex min-h-48 flex-1 flex-col items-center justify-center rounded-lg border border-dashed border-gray-300 bg-white p-6 text-center">
      <LayoutTemplate className="h-9 w-9 text-gray-400" aria-hidden="true" />
      <h2 className="mt-3 font-semibold">{search ? searchTitle : title}</h2>
      <p className="mt-1 max-w-lg text-sm text-gray-600">{search ? searchDescription : description}</p>
      {search && <Button type="button" variant="outline" className="mt-4" onClick={onClear}>{clearLabel}</Button>}
    </div>
  );
}

function formatDate(value: string, locale: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}
