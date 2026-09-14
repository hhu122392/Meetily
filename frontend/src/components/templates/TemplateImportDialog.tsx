'use client';

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from 'react';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import {
  AlertCircle,
  Check,
  Eye,
  FileText,
  Import,
  LoaderCircle,
  Pencil,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldCheck,
  TriangleAlert,
  UploadCloud,
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
import { TemplateEditorFields } from '@/components/templates/TemplateEditorFields';
import { TemplateStructurePreview } from '@/components/templates/TemplateStructurePreview';
import {
  editorDraftToTemplate,
  isTemplateDraftDirty,
  templateToEditorDraft,
  validateTemplateDraft,
  type TemplateEditorDraft,
} from '@/lib/template-editor';
import { templateTranslationKey } from '@/lib/template-library';
import { normalizeTemplateApiError, templateService } from '@/services/templateService';
import type {
  DocumentImportPreview,
  PreviewTemplateDocumentItem,
  TemplateApiError,
  TemplateFieldIssue,
  TemplateListItem,
  TemplateValidationResult,
  TemplateV2,
} from '@/types/summary-template';

export type ImportItemStatus = 'processing' | 'cancelling' | 'ready' | 'failed' | 'saving' | 'saved' | 'skipped' | 'cancelled';
export type ImportConflictPolicy = 'keep_both' | 'skip' | 'replace_custom' | 'override_builtin';
export type ImportConflictGroup = 'custom' | 'read_only' | 'batch';

export interface ImportConflictState {
  group: ImportConflictGroup;
  templateId: string;
  existingOrigin?: TemplateListItem['origin'];
  primaryClientId?: string;
}

const MAX_IMPORT_FILES = 50;
const SUPPORTED_IMPORT_EXTENSIONS = new Set(['json', 'docx', 'doc']);

export interface ImportItemState {
  clientId: string;
  fileName: string;
  status: ImportItemStatus;
  preview?: DocumentImportPreview;
  error?: TemplateApiError;
  editorDraft?: TemplateEditorDraft;
  draftRevision: number;
  validation?: TemplateValidationResult;
  validating: boolean;
  validationError?: TemplateApiError;
  saveError?: TemplateApiError;
  reviewConfirmed: boolean;
  conflictPolicy?: ImportConflictPolicy;
}

interface TemplateImportDialogProps {
  disabled?: boolean;
  onImported: () => void | Promise<void>;
}

export function selectedFileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || 'selected-template';
}

export function filterSupportedImportPaths(paths: string[]): string[] {
  const seen = new Set<string>();
  return paths.filter((path) => {
    const extension = path.split('.').at(-1)?.toLocaleLowerCase() ?? '';
    const normalized = path.toLocaleLowerCase();
    if (!SUPPORTED_IMPORT_EXTENSIONS.has(extension) || seen.has(normalized)) return false;
    seen.add(normalized);
    return true;
  });
}

export function hasUnfinishedImportItems(statuses: ImportItemStatus[]): boolean {
  return statuses.some((status) => !['saved', 'skipped', 'cancelled'].includes(status));
}

function newClientId(index: number): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `import-${Date.now()}-${index}`;
}

export function mapDocumentPreviewResult(item: PreviewTemplateDocumentItem): ImportItemState {
  if (item.status === 'cancelled') {
    return {
      clientId: item.itemId,
      fileName: item.fileName,
      status: 'cancelled',
      draftRevision: 0,
      validating: false,
      reviewConfirmed: false,
    };
  }
  if (item.preview) {
    return {
      clientId: item.itemId,
      fileName: item.fileName,
      status: 'ready',
      preview: item.preview,
      editorDraft: templateToEditorDraft(item.preview.draft),
      draftRevision: 0,
      validation: {
        valid: true,
        errors: [],
        warnings: [],
        normalized: structuredClone(item.preview.draft),
      },
      validating: false,
      reviewConfirmed: item.preview.confidence !== 'low',
    };
  }
  return {
    clientId: item.itemId,
    fileName: item.fileName,
    status: 'failed',
    error: item.error ?? normalizeTemplateApiError(null),
    draftRevision: 0,
    validating: false,
    reviewConfirmed: false,
  };
}

export function importDraftIssues(item: ImportItemState): TemplateFieldIssue[] {
  if (!item.editorDraft) return [];
  const issues = [...validateTemplateDraft(item.editorDraft), ...(item.validation?.errors ?? [])];
  const seen = new Set<string>();
  return issues.filter((issue) => {
    const key = `${issue.path}\u0000${issue.code}\u0000${issue.messageKey}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function isImportDraftDirty(item: ImportItemState): boolean {
  return !!item.preview
    && !!item.editorDraft
    && isTemplateDraftDirty(item.editorDraft, templateToEditorDraft(item.preview.draft));
}

export function allowedImportConflictPolicies(conflict: ImportConflictState): ImportConflictPolicy[] {
  if (conflict.group === 'custom') return ['keep_both', 'skip', 'replace_custom'];
  if (conflict.group === 'read_only') return ['keep_both', 'skip', 'override_builtin'];
  return ['keep_both', 'skip'];
}

export function isImportConflictPolicyAllowed(
  conflict: ImportConflictState,
  policy?: ImportConflictPolicy,
): boolean {
  return !!policy && allowedImportConflictPolicies(conflict).includes(policy);
}

export function buildImportConflictPlan(
  items: readonly ImportItemState[],
  existingTemplates: readonly TemplateListItem[],
): Record<string, ImportConflictState> {
  const existingById = new Map(existingTemplates.map((template) => [template.id, template]));
  const primaryById = new Map<string, string>();
  const plan: Record<string, ImportConflictState> = {};

  for (const item of items) {
    if (item.status !== 'ready' || !item.editorDraft) continue;
    const templateId = item.editorDraft.id;
    const primaryClientId = primaryById.get(templateId);
    if (primaryClientId) {
      plan[item.clientId] = {
        group: 'batch',
        templateId,
        primaryClientId,
        existingOrigin: existingById.get(templateId)?.origin,
      };
      continue;
    }
    primaryById.set(templateId, item.clientId);
    const existing = existingById.get(templateId);
    if (!existing) continue;
    plan[item.clientId] = {
      group: existing.origin === 'custom' ? 'custom' : 'read_only',
      templateId,
      existingOrigin: existing.origin,
    };
  }
  return plan;
}

export function reconcileImportConflictPolicies(
  items: readonly ImportItemState[],
  plan: Readonly<Record<string, ImportConflictState>>,
): ImportItemState[] {
  return items.map((item) => {
    const conflict = plan[item.clientId];
    if (!item.conflictPolicy) return item;
    if (conflict && isImportConflictPolicyAllowed(conflict, item.conflictPolicy)) return item;
    return { ...item, conflictPolicy: undefined };
  });
}

export function applyImportConflictPolicyToGroup(
  items: readonly ImportItemState[],
  plan: Readonly<Record<string, ImportConflictState>>,
  group: ImportConflictGroup,
  policy: ImportConflictPolicy,
): ImportItemState[] {
  return items.map((item) => {
    const conflict = plan[item.clientId];
    if (!conflict || conflict.group !== group) return item;
    if (!isImportConflictPolicyAllowed(conflict, policy)) return item;
    return { ...item, conflictPolicy: policy, saveError: undefined };
  });
}

export function isImportItemSaveable(
  item: ImportItemState,
  conflict?: ImportConflictState,
): boolean {
  return item.status === 'ready'
    && !!item.preview
    && !!item.editorDraft
    && item.reviewConfirmed
    && !item.validating
    && !item.validationError
    && item.validation?.valid === true
    && importDraftIssues(item).length === 0
    && (!conflict || isImportConflictPolicyAllowed(conflict, item.conflictPolicy));
}

export function replaceImportItemDraft(
  item: ImportItemState,
  editorDraft: TemplateEditorDraft,
): ImportItemState {
  return {
    ...item,
    editorDraft,
    draftRevision: item.draftRevision + 1,
    validation: undefined,
    validating: false,
    validationError: undefined,
    saveError: undefined,
    conflictPolicy: undefined,
  };
}

export function resetImportItemDraft(item: ImportItemState): ImportItemState {
  if (!item.preview) return item;
  return replaceImportItemDraft(item, templateToEditorDraft(item.preview.draft));
}

export function clearCancelledImportValidation(
  item: ImportItemState,
  clientId: string,
  revision: number,
): ImportItemState {
  return item.clientId === clientId
    && item.draftRevision === revision
    && item.validating
    ? { ...item, validating: false }
    : item;
}

export function TemplateImportDialog({ disabled = false, onImported }: TemplateImportDialogProps) {
  const { t } = useTranslation('templates');
  const [openDialog, setOpenDialog] = useState(false);
  const [items, setItems] = useState<ImportItemState[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [existingTemplates, setExistingTemplates] = useState<TemplateListItem[] | null>(null);
  const [conflictScanning, setConflictScanning] = useState(false);
  const [conflictScanError, setConflictScanError] = useState<TemplateApiError | undefined>();
  const [conflictScanRevision, setConflictScanRevision] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [reviewMode, setReviewMode] = useState<'preview' | 'edit'>('preview');
  const [discardDialogOpen, setDiscardDialogOpen] = useState(false);
  const [overrideConfirmOpen, setOverrideConfirmOpen] = useState(false);
  const [overrideConfirmCount, setOverrideConfirmCount] = useState(0);
  const previewOperationRef = useRef(0);
  const pendingConflictPlanRef = useRef<Record<string, ImportConflictState>>({});
  const sourcePathsRef = useRef(new Map<string, string>());
  const currentImportJobRef = useRef<string | null>(null);
  const busy = items.some((item) => ['processing', 'cancelling', 'saving'].includes(item.status));
  const processingAvailable = items.some((item) => item.status === 'processing');
  const processing = items.some((item) => item.status === 'processing' || item.status === 'cancelling');
  const batchCancellationPending = processing && !processingAvailable;
  const saving = items.some((item) => item.status === 'saving');
  const conflictPlan = useMemo(
    () => existingTemplates ? buildImportConflictPlan(items, existingTemplates) : {},
    [existingTemplates, items],
  );
  const saveableCount = existingTemplates
    ? items.filter((item) => isImportItemSaveable(item, conflictPlan[item.clientId])).length
    : 0;
  const unresolvedConflictCount = existingTemplates
    ? items.filter((item) => {
      const conflict = conflictPlan[item.clientId];
      return conflict && isImportItemSaveable({ ...item, conflictPolicy: 'keep_both' })
        && !isImportConflictPolicyAllowed(conflict, item.conflictPolicy);
    }).length
    : 0;
  const failedCount = items.filter((item) => item.status === 'failed').length;
  const unfinished = hasUnfinishedImportItems(items.map((item) => item.status));
  const selected = useMemo(
    () => items.find((item) => item.clientId === selectedId) ?? items[0] ?? null,
    [items, selectedId],
  );
  const selectedIssues = useMemo(
    () => selected ? importDraftIssues(selected) : [],
    [selected],
  );
  const selectedDirty = useMemo(
    () => selected ? isImportDraftDirty(selected) : false,
    [selected],
  );
  const selectedConflict = selected ? conflictPlan[selected.clientId] : undefined;
  const selectedConflictGroupCount = selectedConflict
    ? Object.values(conflictPlan).filter((conflict) => conflict.group === selectedConflict.group).length
    : 0;
  const conflictCounts = useMemo(() => ({
    custom: Object.values(conflictPlan).filter((conflict) => conflict.group === 'custom').length,
    read_only: Object.values(conflictPlan).filter((conflict) => conflict.group === 'read_only').length,
    batch: Object.values(conflictPlan).filter((conflict) => conflict.group === 'batch').length,
  }), [conflictPlan]);

  const processPaths = useCallback(async (candidatePaths: string[]) => {
    if (disabled) return;
    const paths = filterSupportedImportPaths(candidatePaths);
    if (paths.length === 0) {
      toast.error(t('import.noSupportedFiles'));
      return;
    }
    if (paths.length !== candidatePaths.length) {
      toast.warning(t('import.unsupportedFilesSkipped', {
        count: candidatePaths.length - paths.length,
      }));
    }
    if (paths.length > MAX_IMPORT_FILES) {
      toast.error(t('import.tooManyFiles', { max: MAX_IMPORT_FILES }));
      return;
    }
    const queued = paths.map((path, index): ImportItemState => ({
      clientId: newClientId(index),
      fileName: selectedFileName(path),
      status: 'processing',
      draftRevision: 0,
      validating: false,
      reviewConfirmed: false,
    }));
    const operationId = ++previewOperationRef.current;
    const jobId = newClientId(-1);
    currentImportJobRef.current = jobId;
    sourcePathsRef.current = new Map(queued.map((item, index) => [item.clientId, paths[index]]));
    setItems(queued);
    setSelectedId(queued[0].clientId);
    setReviewMode('preview');
    setExistingTemplates(null);
    setConflictScanError(undefined);
    setOpenDialog(true);
    try {
      const response = await templateService.previewImports(
        paths,
        jobId,
        queued.map((item) => item.clientId),
      );
      if (operationId !== previewOperationRef.current) return;
      if (currentImportJobRef.current === jobId) currentImportJobRef.current = null;
      const completed = response.items.map(mapDocumentPreviewResult);
      sourcePathsRef.current = new Map(completed.map((item, index) => [item.clientId, paths[index]]));
      setItems(completed);
      setSelectedId((current) => completed.some((item) => item.clientId === current)
        ? current
        : completed[0]?.clientId ?? null);
      if (response.status === 'cancelled') toast.info(t('import.cancelled'));
    } catch (caught) {
      if (operationId !== previewOperationRef.current) return;
      if (currentImportJobRef.current === jobId) currentImportJobRef.current = null;
      const error = normalizeTemplateApiError(caught);
      setItems(queued.map((item) => ({ ...item, status: 'failed', error })));
    }
  }, [disabled, t]);

  const conflictSignature = items
    .filter((item) => item.status === 'ready' && item.editorDraft)
    .map((item) => `${item.clientId}:${item.draftRevision}:${item.editorDraft!.id}`)
    .join('|');

  useEffect(() => {
    if (!openDialog || processing || saving) return undefined;
    let cancelled = false;
    setConflictScanning(true);
    setConflictScanError(undefined);
    const timeout = window.setTimeout(() => {
      void templateService.list({ origin: 'all', includeInvalid: true, includeTrash: false })
        .then((response) => {
          if (cancelled) return;
          setExistingTemplates(response.templates);
          setItems((current) => reconcileImportConflictPolicies(
            current,
            buildImportConflictPlan(current, response.templates),
          ));
          setConflictScanning(false);
        })
        .catch((caught) => {
          if (cancelled) return;
          setExistingTemplates(null);
          setConflictScanning(false);
          setConflictScanError(normalizeTemplateApiError(caught));
        });
    }, 250);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [conflictScanRevision, conflictSignature, openDialog, processing, saving]);

  const setSelectedEditorDraft = useCallback<Dispatch<SetStateAction<TemplateEditorDraft>>>((update) => {
    if (!selectedId) return;
    setItems((current) => current.map((item) => {
      if (item.clientId !== selectedId || item.status !== 'ready' || !item.editorDraft) return item;
      const editorDraft = typeof update === 'function' ? update(item.editorDraft) : update;
      if (editorDraft === item.editorDraft) return item;
      return replaceImportItemDraft(item, editorDraft);
    }));
  }, [selectedId]);

  const resetSelectedDraft = useCallback(() => {
    if (!selectedId) return;
    setItems((current) => current.map((item) => {
      if (item.clientId !== selectedId || item.status !== 'ready' || !item.preview) return item;
      return resetImportItemDraft(item);
    }));
  }, [selectedId]);

  const setSelectedConflictPolicy = useCallback((policy?: ImportConflictPolicy) => {
    if (!selectedId) return;
    setItems((current) => current.map((item) => item.clientId === selectedId
      ? { ...item, conflictPolicy: policy, saveError: undefined }
      : item));
  }, [selectedId]);

  const applySelectedPolicyToGroup = useCallback(() => {
    if (!selectedId) return;
    const selectedConflict = conflictPlan[selectedId];
    const selectedItem = items.find((item) => item.clientId === selectedId);
    const policy = selectedItem?.conflictPolicy;
    if (!selectedConflict || !policy || !isImportConflictPolicyAllowed(selectedConflict, policy)) return;
    setItems((current) => applyImportConflictPolicyToGroup(
      current,
      conflictPlan,
      selectedConflict.group,
      policy,
    ));
  }, [conflictPlan, items, selectedId]);

  useEffect(() => {
    if (!selected || selected.status !== 'ready' || !selected.editorDraft) return undefined;
    if (selected.validation) return undefined;
    const clientId = selected.clientId;
    const revision = selected.draftRevision;
    const candidate = editorDraftToTemplate(selected.editorDraft);
    const localIssues = validateTemplateDraft(selected.editorDraft);
    if (localIssues.length > 0) {
      setItems((current) => current.map((item) => item.clientId === clientId && item.draftRevision === revision
        ? { ...item, validation: undefined, validating: false, validationError: undefined }
        : item));
      return undefined;
    }

    setItems((current) => current.map((item) => item.clientId === clientId && item.draftRevision === revision
      ? { ...item, validating: true, validationError: undefined }
      : item));
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      void templateService.validate(candidate, 'create').then((validation) => {
        if (cancelled) return;
        setItems((current) => current.map((item) => item.clientId === clientId && item.draftRevision === revision
          ? { ...item, validation, validating: false, validationError: undefined }
          : item));
      }).catch((caught) => {
        if (cancelled) return;
        const validationError = normalizeTemplateApiError(caught);
        setItems((current) => current.map((item) => item.clientId === clientId && item.draftRevision === revision
          ? { ...item, validation: undefined, validating: false, validationError }
          : item));
      });
    }, 350);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
      setItems((current) => current.map((item) => clearCancelledImportValidation(
        item,
        clientId,
        revision,
      )));
    };
  }, [selected]);

  const retryFailed = async () => {
    if (busy) return;
    const failedItems = items.flatMap((item) => {
      if (item.status !== 'failed') return [];
      const path = sourcePathsRef.current.get(item.clientId);
      return path ? [{ item, path }] : [];
    });
    if (failedItems.length === 0) return;
    const operationId = ++previewOperationRef.current;
    const jobId = newClientId(-1);
    currentImportJobRef.current = jobId;
    const failedIds = new Set(failedItems.map(({ item }) => item.clientId));
    setItems((current) => current.map((item) => failedIds.has(item.clientId)
      ? { ...item, status: 'processing', error: undefined }
      : item));
    try {
      const response = await templateService.previewImports(
        failedItems.map(({ path }) => path),
        jobId,
        failedItems.map(({ item }) => item.clientId),
      );
      if (operationId !== previewOperationRef.current) return;
      if (currentImportJobRef.current === jobId) currentImportJobRef.current = null;
      setItems((current) => current.map((item) => {
        const failedIndex = failedItems.findIndex(({ item: failed }) => failed.clientId === item.clientId);
        if (failedIndex < 0) return item;
        const result = response.items[failedIndex];
        return result
          ? { ...mapDocumentPreviewResult(result), clientId: item.clientId }
          : { ...item, status: 'failed', error: normalizeTemplateApiError(null) };
      }));
    } catch (caught) {
      if (operationId !== previewOperationRef.current) return;
      if (currentImportJobRef.current === jobId) currentImportJobRef.current = null;
      const error = normalizeTemplateApiError(caught);
      setItems((current) => current.map((item) => failedIds.has(item.clientId)
        ? { ...item, status: 'failed', error }
        : item));
    }
  };

  const cancelProcessing = async () => {
    const jobId = currentImportJobRef.current;
    if (!processingAvailable || saving || !jobId) return;
    const requestedIds = new Set(
      items.filter((item) => item.status === 'processing').map((item) => item.clientId),
    );
    setItems((current) => current.map((item) => item.status === 'processing'
      ? { ...item, status: 'cancelling', error: undefined }
      : item));
    try {
      await templateService.cancelImportJob(jobId);
      toast.info(t('import.cancelRequested'));
    } catch (caught) {
      const error = normalizeTemplateApiError(caught);
      setItems((current) => current.map((item) => requestedIds.has(item.clientId) && item.status === 'cancelling'
        ? { ...item, status: 'processing' }
        : item));
      toast.error(t('import.cancelFailed'), { description: error.debugId });
    }
  };

  const cancelItemProcessing = async (clientId: string) => {
    const jobId = currentImportJobRef.current;
    if (!jobId || saving) return;
    setItems((current) => current.map((item) => item.clientId === clientId && item.status === 'processing'
      ? { ...item, status: 'cancelling', error: undefined }
      : item));
    try {
      await templateService.cancelImportItem(jobId, clientId);
      toast.info(t('import.itemCancelRequested'));
    } catch (caught) {
      const error = normalizeTemplateApiError(caught);
      setItems((current) => current.map((item) => item.clientId === clientId && item.status === 'cancelling'
        ? { ...item, status: 'processing' }
        : item));
      toast.error(t('import.cancelFailed'), { description: error.debugId });
    }
  };

  const chooseFiles = async () => {
    try {
      const selection = await open({
        multiple: true,
        directory: false,
        title: t('import.filePickerTitle'),
        filters: [
          { name: t('import.templateFiles'), extensions: ['json', 'docx', 'doc'] },
          { name: t('import.jsonFiles'), extensions: ['json'] },
          { name: t('import.wordFiles'), extensions: ['docx', 'doc'] },
        ],
      });
      if (!selection) return;
      const paths = Array.isArray(selection) ? selection : [selection];
      if (paths.length === 0) return;
      await processPaths(paths);
    } catch (caught) {
      const error = normalizeTemplateApiError(caught);
      toast.error(t('import.openFailed'), { description: error.debugId });
    }
  };

  useEffect(() => {
    if (disabled || typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return undefined;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === 'enter' || event.payload.type === 'over') {
        setDragging(true);
      } else if (event.payload.type === 'leave') {
        setDragging(false);
      } else if (event.payload.type === 'drop') {
        setDragging(false);
        void processPaths(event.payload.paths);
      }
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [disabled, processPaths]);

  useEffect(() => () => {
    const jobId = currentImportJobRef.current;
    if (jobId) void templateService.cancelImportJob(jobId).catch(() => undefined);
  }, []);

  const saveReady = async (confirmedConflictPlan: Record<string, ImportConflictState>) => {
    const readyItems = items.filter((item) => isImportItemSaveable(
      item,
      confirmedConflictPlan[item.clientId],
    ));
    if (readyItems.length === 0) return;
    let savedCount = 0;
    let skippedCount = 0;
    for (const item of readyItems) {
      const conflict = confirmedConflictPlan[item.clientId];
      const conflictPolicy = conflict ? item.conflictPolicy! : undefined;
      setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
        ? { ...candidate, status: 'saving', error: undefined, saveError: undefined }
        : candidate));
      try {
        if (conflict?.group === 'batch' && conflictPolicy === 'skip') {
          skippedCount += 1;
          setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
            ? { ...candidate, status: 'skipped', error: undefined, saveError: undefined }
            : candidate));
          continue;
        }
        const validation = await templateService.validate(
          editorDraftToTemplate(item.editorDraft!),
          'create',
        );
        if (!validation.valid || !validation.normalized) {
          setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
            ? { ...candidate, status: 'ready', validation, validating: false }
            : candidate));
          continue;
        }
        const draft = validation.normalized;
        if (conflictPolicy === 'replace_custom') {
          try {
            const existing = await templateService.get({
              templateId: draft.id,
              origin: 'custom',
            });
            await templateService.update({
              templateId: existing.template.id,
              expectedVersion: existing.template.version,
              expectedFileSha256: existing.fileSha256,
              template: { ...draft, id: existing.template.id },
            });
          } catch (caught) {
            const existingError = normalizeTemplateApiError(caught);
            if (existingError.code !== 'TEMPLATE_NOT_FOUND') throw existingError;
            await templateService.create({
              template: draft,
              conflictPolicy: 'error',
            });
          }
        } else {
          await templateService.create({
            template: draft,
            conflictPolicy: conflictPolicy === 'skip' || !conflictPolicy ? 'error' : conflictPolicy,
          });
        }
        savedCount += 1;
        setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
          ? { ...candidate, status: 'saved' }
          : candidate));
      } catch (caught) {
        const error = normalizeTemplateApiError(caught);
        if (conflictPolicy === 'skip' && error.code === 'TEMPLATE_ALREADY_EXISTS') {
          skippedCount += 1;
          setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
            ? { ...candidate, status: 'skipped', error: undefined }
            : candidate));
          continue;
        }
        setItems((current) => current.map((candidate) => candidate.clientId === item.clientId
          ? { ...candidate, status: 'ready', error: undefined, saveError: error }
          : candidate));
      }
    }
    if (savedCount > 0) {
      await onImported();
      toast.success(t('import.saved', { count: savedCount }));
    }
    if (skippedCount > 0) {
      toast.info(t('import.skipped', { count: skippedCount }));
    }
  };

  useEffect(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!openDialog || !unfinished) return;
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [openDialog, unfinished]);

  const resetAndCloseDialog = () => {
    previewOperationRef.current += 1;
    setOpenDialog(false);
    setItems([]);
    setSelectedId(null);
    setDragging(false);
    setReviewMode('preview');
    setDiscardDialogOpen(false);
    setOverrideConfirmOpen(false);
    setOverrideConfirmCount(0);
    setExistingTemplates(null);
    setConflictScanning(false);
    setConflictScanError(undefined);
    pendingConflictPlanRef.current = {};
    sourcePathsRef.current.clear();
  };

  const requestCloseDialog = () => {
    if (busy) {
      toast.warning(t('import.leave.busy'));
      return;
    }
    if (unfinished) {
      setDiscardDialogOpen(true);
      return;
    }
    resetAndCloseDialog();
  };

  const requestSaveReady = async () => {
    if (busy || conflictScanning) return;
    setConflictScanning(true);
    setConflictScanError(undefined);
    try {
      const response = await templateService.list({
        origin: 'all',
        includeInvalid: true,
        includeTrash: false,
      });
      const latestConflictPlan = buildImportConflictPlan(items, response.templates);
      setExistingTemplates(response.templates);
      setItems((current) => reconcileImportConflictPolicies(current, latestConflictPlan));
      const unresolved = items.filter((item) => {
        const conflict = latestConflictPlan[item.clientId];
        return conflict
          && isImportItemSaveable({ ...item, conflictPolicy: 'keep_both' })
          && !isImportConflictPolicyAllowed(conflict, item.conflictPolicy);
      });
      if (unresolved.length > 0) {
        setSelectedId(unresolved[0].clientId);
        toast.warning(t('import.conflict.resolveBeforeSave', { count: unresolved.length }));
        return;
      }
      const readyItems = items.filter((item) => isImportItemSaveable(
        item,
        latestConflictPlan[item.clientId],
      ));
      if (readyItems.length === 0) return;
      const overrideCount = readyItems.filter((item) => (
        latestConflictPlan[item.clientId]?.group === 'read_only'
        && item.conflictPolicy === 'override_builtin'
      )).length;
      pendingConflictPlanRef.current = latestConflictPlan;
      if (overrideCount > 0) {
        setOverrideConfirmCount(overrideCount);
        setOverrideConfirmOpen(true);
        return;
      }
      await saveReady(latestConflictPlan);
    } catch (caught) {
      setExistingTemplates(null);
      setConflictScanError(normalizeTemplateApiError(caught));
    } finally {
      setConflictScanning(false);
    }
  };

  return (
    <>
      <Button type="button" variant="outline" onClick={() => void chooseFiles()} disabled={disabled}>
        <Import aria-hidden="true" />
        {t('library.import')}
      </Button>

      {dragging && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-blue-950/40 p-8 backdrop-blur-sm" role="status" aria-live="polite">
          <div className="flex min-h-64 w-full max-w-2xl flex-col items-center justify-center rounded-2xl border-2 border-dashed border-blue-400 bg-white/95 p-10 text-center shadow-2xl">
            <UploadCloud className="h-12 w-12 text-blue-600" aria-hidden="true" />
            <p className="mt-4 text-xl font-semibold text-gray-900">{t('import.dropActive')}</p>
            <p className="mt-2 text-sm text-gray-600">{t('import.dropHint')}</p>
          </div>
        </div>
      )}

      <Dialog open={openDialog} onOpenChange={(next) => !next && requestCloseDialog()}>
        <DialogContent className="max-h-[90vh] overflow-hidden sm:max-w-5xl">
          <DialogHeader>
            <DialogTitle>{t('import.title')}</DialogTitle>
            <DialogDescription>{t('import.description')}</DialogDescription>
          </DialogHeader>

          <div className="flex items-start gap-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-900">
            <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <span>{t('import.localOnly')}</span>
          </div>

          <div className="rounded-md border border-gray-200 bg-gray-50 p-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <p className="text-sm font-medium text-gray-900">{t('import.conflict.planTitle')}</p>
                <p className="text-xs text-gray-600">{t('import.conflict.planDescription')}</p>
              </div>
              {conflictScanning ? (
                <span className="inline-flex items-center gap-1.5 text-xs font-medium text-blue-700" role="status">
                  <LoaderCircle className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                  {t('import.conflict.scanning')}
                </span>
              ) : conflictScanError ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => setConflictScanRevision((current) => current + 1)}
                  disabled={busy}
                >
                  <RefreshCw aria-hidden="true" />
                  {t('import.conflict.retryScan')}
                </Button>
              ) : (
                <span className={`text-xs font-medium ${unresolvedConflictCount > 0 ? 'text-amber-800' : 'text-emerald-700'}`}>
                  {unresolvedConflictCount > 0
                    ? t('import.conflict.unresolvedCount', { count: unresolvedConflictCount })
                    : t('import.conflict.allResolved')}
                </span>
              )}
            </div>
            <div className="mt-2 flex flex-wrap gap-2 text-xs">
              <span className="rounded-full bg-blue-100 px-2.5 py-1 text-blue-900">
                {t('import.conflict.groups.custom', { count: conflictCounts.custom })}
              </span>
              <span className="rounded-full bg-violet-100 px-2.5 py-1 text-violet-900">
                {t('import.conflict.groups.read_only', { count: conflictCounts.read_only })}
              </span>
              <span className="rounded-full bg-amber-100 px-2.5 py-1 text-amber-900">
                {t('import.conflict.groups.batch', { count: conflictCounts.batch })}
              </span>
            </div>
            {conflictScanError && (
              <div className="mt-2 text-xs text-red-700" role="alert">
                <p>{t('import.conflict.scanFailed')}</p>
                <p className="mt-1 select-all font-mono text-[11px] text-red-600">{conflictScanError.debugId}</p>
              </div>
            )}
          </div>

          <div className="grid min-h-0 gap-4 md:grid-cols-[minmax(220px,0.8fr)_minmax(0,2fr)]">
            <div className="max-h-[58vh] space-y-2 overflow-y-auto pr-1 custom-scrollbar" aria-label={t('import.queue')}>
              {items.map((item) => (
                <button
                  key={item.clientId}
                  type="button"
                  onClick={() => setSelectedId(item.clientId)}
                  disabled={saving}
                  className={`flex w-full items-center gap-3 rounded-md border p-3 text-left transition ${selected?.clientId === item.clientId ? 'border-blue-400 bg-blue-50' : 'border-gray-200 bg-white hover:border-gray-300'}`}
                >
                  <StatusIcon status={item.status} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">{item.fileName}</span>
                    <span className="mt-0.5 block text-xs text-gray-500">
                      {item.status === 'ready' && item.validating
                        ? t('import.status.validating')
                        : item.status === 'ready' && !item.reviewConfirmed
                          ? t('import.status.needsReview')
                          : item.status === 'ready' && conflictPlan[item.clientId]
                            && !isImportConflictPolicyAllowed(conflictPlan[item.clientId], item.conflictPolicy)
                            ? t('import.status.needsConflictDecision')
                          : item.status === 'ready' && (importDraftIssues(item).length > 0 || item.validationError || item.saveError)
                            ? t('import.status.needsFix')
                            : t(`import.status.${item.status}`)}
                    </span>
                    {conflictPlan[item.clientId] && (
                      <span className={`mt-1 inline-block rounded px-1.5 py-0.5 text-[10px] font-medium ${item.conflictPolicy ? 'bg-emerald-100 text-emerald-800' : 'bg-amber-100 text-amber-900'}`}>
                        {t(`import.conflict.groupLabels.${conflictPlan[item.clientId].group}`)}
                      </span>
                    )}
                  </span>
                </button>
              ))}
              <Button type="button" variant="outline" className="w-full" onClick={() => void chooseFiles()} disabled={busy}>
                <RefreshCw aria-hidden="true" />
                {t('import.chooseAgain')}
              </Button>
              {failedCount > 0 && (
                <Button type="button" variant="outline" className="w-full" onClick={() => void retryFailed()} disabled={busy}>
                  <RotateCcw aria-hidden="true" />
                  {t('import.retryFailed', { count: failedCount })}
                </Button>
              )}
            </div>

            <div className="max-h-[58vh] min-h-64 overflow-y-auto rounded-md border border-gray-200 bg-white p-4 custom-scrollbar">
              {selected?.status === 'processing' || selected?.status === 'cancelling' || selected?.status === 'saving' ? (
                <div className="flex min-h-56 flex-col items-center justify-center text-center text-gray-600" aria-live="polite">
                  <LoaderCircle className="h-8 w-8 animate-spin" aria-hidden="true" />
                  <p className="mt-3 font-medium">{t(`import.status.${selected.status}`)}</p>
                  <p className="mt-1 text-sm">{selected.fileName}</p>
                  {selected.status === 'processing' && (
                    <Button
                      type="button"
                      variant="outline"
                      className="mt-4"
                      onClick={() => void cancelItemProcessing(selected.clientId)}
                    >
                      <XCircle aria-hidden="true" />
                      {t('import.cancelItem')}
                    </Button>
                  )}
                </div>
              ) : selected?.error ? (
                <ImportErrorPanel error={selected.error} fileName={selected.fileName} />
              ) : selected?.preview && selected.editorDraft ? (
                <div className="space-y-4">
                  {selected.status === 'ready' && (
                    <div className="sticky top-0 z-10 flex flex-wrap items-center justify-between gap-2 border-b border-gray-100 bg-white pb-3">
                      <div className="inline-flex rounded-md border border-gray-200 bg-gray-50 p-1" role="tablist" aria-label={t('import.draft.tabsLabel')}>
                        <button
                          type="button"
                          role="tab"
                          aria-selected={reviewMode === 'preview'}
                          onClick={() => setReviewMode('preview')}
                          className={`inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-sm font-medium ${reviewMode === 'preview' ? 'bg-white text-gray-950 shadow-sm' : 'text-gray-600'}`}
                        >
                          <Eye className="h-4 w-4" aria-hidden="true" />
                          {t('import.draft.preview')}
                        </button>
                        <button
                          type="button"
                          role="tab"
                          aria-selected={reviewMode === 'edit'}
                          onClick={() => setReviewMode('edit')}
                          className={`inline-flex items-center gap-1.5 rounded px-3 py-1.5 text-sm font-medium ${reviewMode === 'edit' ? 'bg-white text-gray-950 shadow-sm' : 'text-gray-600'}`}
                        >
                          <Pencil className="h-4 w-4" aria-hidden="true" />
                          {t('import.draft.edit')}
                        </button>
                      </div>
                      <div className="flex items-center gap-2">
                        {selectedDirty && (
                          <span className="rounded-full bg-amber-100 px-2.5 py-1 text-xs font-medium text-amber-900">
                            {t('import.draft.modified')}
                          </span>
                        )}
                        <Button type="button" variant="outline" size="sm" onClick={resetSelectedDraft} disabled={!selectedDirty || saving}>
                          <RotateCcw aria-hidden="true" />
                          {t('import.draft.reset')}
                        </Button>
                      </div>
                    </div>
                  )}

                  {selected.status === 'ready' && selected.preview.confidence === 'low' && (
                    <label className="flex items-start gap-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm text-amber-950">
                      <input
                        type="checkbox"
                        checked={selected.reviewConfirmed}
                        onChange={(event) => {
                          const reviewConfirmed = event.target.checked;
                          setItems((current) => current.map((item) => item.clientId === selected.clientId
                            ? { ...item, reviewConfirmed }
                            : item));
                        }}
                        className="mt-0.5 h-4 w-4 rounded border-amber-400"
                      />
                      <span>
                        <span className="block font-medium">{t('import.draft.lowConfidenceTitle')}</span>
                        <span className="mt-0.5 block text-xs">{t('import.draft.lowConfidenceConfirm')}</span>
                      </span>
                    </label>
                  )}

                  {selected.status === 'ready' && selectedConflict && (
                    <ImportConflictDecisionPanel
                      conflict={selectedConflict}
                      policy={selected.conflictPolicy}
                      groupCount={selectedConflictGroupCount}
                      disabled={saving || conflictScanning}
                      onPolicyChange={setSelectedConflictPolicy}
                      onApplyToGroup={applySelectedPolicyToGroup}
                    />
                  )}

                  {selected.status === 'ready' && (
                    <DraftValidationBanner
                      issues={selectedIssues}
                      validating={selected.validating}
                      validation={selected.validation}
                      validationError={selected.validationError}
                      saveError={selected.saveError}
                    />
                  )}

                  {selected.status !== 'ready' || reviewMode === 'preview' ? (
                    <ImportPreviewPanel
                      preview={selected.preview}
                      draft={editorDraftToTemplate(selected.editorDraft)}
                      saved={selected.status === 'saved'}
                    />
                  ) : (
                    <div className={saving ? 'pointer-events-none opacity-70' : undefined} aria-busy={saving}>
                      <TemplateEditorFields
                        key={selected.clientId}
                        draft={selected.editorDraft}
                        setDraft={setSelectedEditorDraft}
                        issues={selectedIssues}
                        isCreate
                        onNameChange={(name) => setSelectedEditorDraft((current) => ({ ...current, name }))}
                        onIdChange={(id) => setSelectedEditorDraft((current) => ({ ...current, id }))}
                      />
                      <div className="mt-4">
                        <TemplateStructurePreview draft={selected.editorDraft} />
                      </div>
                    </div>
                  )}
                </div>
              ) : null}
            </div>
          </div>

          <DialogFooter className="gap-2 sm:justify-between">
            <p className="mr-auto text-xs text-gray-500">{t('import.reviewRequired')}</p>
            <Button
              type="button"
              variant="outline"
              onClick={processing ? () => void cancelProcessing() : requestCloseDialog}
              disabled={saving || batchCancellationPending}
            >
              {processing && <XCircle aria-hidden="true" />}
              {batchCancellationPending
                ? t('import.waitingForBackendStop')
                : processing
                  ? t('import.cancelProcessing')
                  : t('import.close')}
            </Button>
            <Button
              type="button"
              onClick={() => void requestSaveReady()}
              disabled={busy || conflictScanning || !!conflictScanError || unresolvedConflictCount > 0 || saveableCount === 0}
            >
              {conflictScanning ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <Save aria-hidden="true" />}
              {t('import.saveReady', { count: saveableCount })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={discardDialogOpen} onOpenChange={setDiscardDialogOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t('import.leave.title')}</DialogTitle>
            <DialogDescription>{t('import.leave.description')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDiscardDialogOpen(false)}>
              {t('import.leave.continue')}
            </Button>
            <Button type="button" variant="destructive" onClick={resetAndCloseDialog}>
              {t('import.leave.discard')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={overrideConfirmOpen} onOpenChange={setOverrideConfirmOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t('import.conflict.overrideConfirmTitle')}</DialogTitle>
            <DialogDescription>{t('import.conflict.overrideConfirmDescription', { count: overrideConfirmCount })}</DialogDescription>
          </DialogHeader>
          <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="alert">
            {t('import.conflict.overrideConfirmWarning')}
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setOverrideConfirmOpen(false)}>
              {t('import.conflict.overrideCancel')}
            </Button>
            <Button
              type="button"
              variant="destructive"
              onClick={() => {
                setOverrideConfirmOpen(false);
                void saveReady(pendingConflictPlanRef.current);
              }}
            >
              {t('import.conflict.overrideConfirm')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}

function ImportConflictDecisionPanel({
  conflict,
  policy,
  groupCount,
  disabled,
  onPolicyChange,
  onApplyToGroup,
}: {
  conflict: ImportConflictState;
  policy?: ImportConflictPolicy;
  groupCount: number;
  disabled: boolean;
  onPolicyChange: (policy?: ImportConflictPolicy) => void;
  onApplyToGroup: () => void;
}) {
  const { t } = useTranslation('templates');
  const policies = allowedImportConflictPolicies(conflict);
  return (
    <div className="rounded-md border border-amber-300 bg-amber-50 p-3" role="group" aria-label={t('import.conflict.itemDecision')}>
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <p className="text-sm font-semibold text-amber-950">
            {t(`import.conflict.groupTitles.${conflict.group}`)}
          </p>
          <p className="mt-1 text-xs text-amber-900">
            {t(`import.conflict.groupHelp.${conflict.group}`, { templateId: conflict.templateId })}
          </p>
          <code className="mt-1 block text-xs text-amber-800">{conflict.templateId}</code>
        </div>
        <span className="rounded-full bg-white px-2.5 py-1 text-xs font-medium text-amber-900">
          {t('import.conflict.sameGroupCount', { count: groupCount })}
        </span>
      </div>
      <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-center">
        <select
          value={policy ?? ''}
          onChange={(event) => onPolicyChange(
            event.target.value ? event.target.value as ImportConflictPolicy : undefined,
          )}
          disabled={disabled}
          aria-label={t('import.conflict.itemDecision')}
          className="h-10 min-w-56 flex-1 rounded-md border border-amber-300 bg-white px-3 text-sm text-gray-900 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-amber-500"
        >
          <option value="">{t('import.conflict.choosePolicy')}</option>
          {policies.map((candidate) => (
            <option key={candidate} value={candidate}>{t(`import.conflict.${candidate}`)}</option>
          ))}
        </select>
        <Button
          type="button"
          variant="outline"
          onClick={onApplyToGroup}
          disabled={disabled || !isImportConflictPolicyAllowed(conflict, policy) || groupCount < 2}
        >
          {t('import.conflict.applyToGroup', { count: groupCount })}
        </Button>
      </div>
      <p className="mt-2 text-xs text-amber-900">
        {policy ? t(`import.conflict.help.${policy}`) : t('import.conflict.decisionRequired')}
      </p>
    </div>
  );
}

function StatusIcon({ status }: { status: ImportItemStatus }) {
  if (status === 'processing' || status === 'cancelling' || status === 'saving') {
    return <LoaderCircle className="h-5 w-5 shrink-0 animate-spin text-blue-600" aria-hidden="true" />;
  }
  if (status === 'ready') {
    return <FileText className="h-5 w-5 shrink-0 text-blue-600" aria-hidden="true" />;
  }
  if (status === 'saved') {
    return <Check className="h-5 w-5 shrink-0 text-emerald-600" aria-hidden="true" />;
  }
  if (status === 'skipped') {
    return <Check className="h-5 w-5 shrink-0 text-gray-500" aria-hidden="true" />;
  }
  if (status === 'cancelled') {
    return <XCircle className="h-5 w-5 shrink-0 text-gray-500" aria-hidden="true" />;
  }
  return <AlertCircle className="h-5 w-5 shrink-0 text-red-600" aria-hidden="true" />;
}

function ImportErrorPanel({ error, fileName }: { error: TemplateApiError; fileName: string }) {
  const { t } = useTranslation('templates');
  return (
    <div className="flex min-h-56 flex-col items-center justify-center text-center" role="alert">
      <AlertCircle className="h-9 w-9 text-red-600" aria-hidden="true" />
      <h3 className="mt-3 font-semibold text-red-900">{fileName}</h3>
      <p className="mt-2 max-w-lg text-sm text-red-700">
        {t(error.messageKey.replace(/^templates\./, ''), {
          ...error.params,
          defaultValue: t('errors.io'),
        })}
      </p>
      {error.fieldErrors && error.fieldErrors.length > 0 && (
        <ul className="mt-3 max-w-xl space-y-1 text-left text-sm text-red-700">
          {error.fieldErrors.map((fieldError, index) => (
            <li key={`${fieldError.path}-${fieldError.code}-${index}`}>
              <code className="font-mono text-xs">{fieldError.path}</code>
              {' — '}
              {t(fieldError.messageKey.replace(/^templates\./, ''), {
                ...fieldError.params,
                defaultValue: fieldError.code,
              })}
            </li>
          ))}
        </ul>
      )}
      <p className="mt-2 select-all font-mono text-xs text-red-500">{error.debugId}</p>
    </div>
  );
}

function DraftValidationBanner({
  issues,
  validating,
  validation,
  validationError,
  saveError,
}: {
  issues: readonly TemplateFieldIssue[];
  validating: boolean;
  validation?: TemplateValidationResult;
  validationError?: TemplateApiError;
  saveError?: TemplateApiError;
}) {
  const { t } = useTranslation('templates');
  if (validating) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-blue-200 bg-blue-50 p-3 text-sm text-blue-900" role="status" aria-live="polite">
        <LoaderCircle className="h-4 w-4 animate-spin" aria-hidden="true" />
        {t('editor.validation.validating')}
      </div>
    );
  }
  if (validationError) {
    return (
      <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-900" role="alert">
        <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>
          {t('import.draft.validationUnavailable')}
          <span className="mt-1 block select-all font-mono text-xs text-red-600">{validationError.debugId}</span>
        </span>
      </div>
    );
  }
  if (saveError) {
    return (
      <div className="flex items-start gap-2 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-900" role="alert">
        <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <span>
          {t(saveError.messageKey.replace(/^templates\./, ''), {
            ...saveError.params,
            defaultValue: t('editor.errors.saveFailed'),
          })}
          <span className="mt-1 block select-all font-mono text-xs text-red-600">{saveError.debugId}</span>
        </span>
      </div>
    );
  }
  if (issues.length > 0) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-900" role="alert">
        <AlertCircle className="h-4 w-4" aria-hidden="true" />
        {t('editor.validation.summary', { count: issues.length })}
      </div>
    );
  }
  if (validation?.warnings.length) {
    return (
      <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="status">
        <div className="flex items-center gap-2 font-medium">
          <TriangleAlert className="h-4 w-4" aria-hidden="true" />
          {t('import.draft.validationWarnings')}
        </div>
        <ul className="mt-2 list-disc space-y-1 pl-5">
          {validation.warnings.map((warning, index) => (
            <li key={`${warning.path}:${warning.code}:${index}`}>
              {t(templateTranslationKey(warning.messageKey), warning.params ?? {})}
            </li>
          ))}
        </ul>
      </div>
    );
  }
  if (validation?.valid) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-emerald-200 bg-emerald-50 p-3 text-sm text-emerald-900" role="status">
        <Check className="h-4 w-4" aria-hidden="true" />
        {t('editor.validation.valid')}
      </div>
    );
  }
  return null;
}

function ImportPreviewPanel({
  preview,
  draft,
  saved,
}: {
  preview: DocumentImportPreview;
  draft: TemplateV2;
  saved: boolean;
}) {
  const { t } = useTranslation('templates');
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="text-lg font-semibold text-gray-900">{draft.name}</h3>
          <p className="mt-1 text-sm text-gray-600">{preview.fileName}</p>
        </div>
        <span className={`rounded-full px-2.5 py-1 text-xs font-medium ${saved ? 'bg-emerald-100 text-emerald-800' : preview.confidence === 'high' ? 'bg-blue-100 text-blue-800' : 'bg-amber-100 text-amber-900'}`}>
          {saved ? t('import.status.saved') : t(`import.confidence.${preview.confidence}`)}
        </span>
      </div>

      {preview.warnings.length > 0 && (
        <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900">
          <div className="flex items-center gap-2 font-medium">
            <TriangleAlert className="h-4 w-4" aria-hidden="true" />
            {t('import.warningsTitle')}
          </div>
          <ul className="mt-2 list-disc space-y-1 pl-5">
            {preview.warnings.map((warning, index) => (
              <li key={`${warning.code}-${index}`}>
                {t(warning.messageKey.replace(/^templates\./, ''), {
                  ...warning.params,
                  defaultValue: warning.code,
                })}
              </li>
            ))}
          </ul>
        </div>
      )}

      <div>
        <h4 className="text-sm font-semibold uppercase tracking-wide text-gray-500">{t('import.generatedSections')}</h4>
        <ol className="mt-2 space-y-2">
          {draft.sections.map((section, index) => (
            <li key={section.id} className="rounded-md border border-gray-200 bg-gray-50 p-3">
              <p className="font-medium">{index + 1}. {section.title}</p>
              <p className="mt-1 whitespace-pre-wrap text-sm text-gray-600">{section.instruction}</p>
            </li>
          ))}
        </ol>
      </div>

      {preview.outline.length > 0 ? (
        <details className="rounded-md border border-gray-200 p-3">
          <summary className="cursor-pointer text-sm font-medium">{t('import.sourceOutline')}</summary>
          <ol className="mt-3 space-y-2 text-sm text-gray-600">
            {preview.outline.map((node, index) => (
              <li key={`${node.kind}-${index}`} className="whitespace-pre-wrap">
                <span className="mr-2 text-xs uppercase text-gray-400">{node.kind}</span>
                {node.text}
              </li>
            ))}
          </ol>
        </details>
      ) : (
        <p className="rounded-md border border-gray-200 bg-gray-50 p-3 text-sm text-gray-600">
          {t('import.jsonValidated', { schemaVersion: draft.schemaVersion })}
        </p>
      )}

      <p className="text-xs text-gray-500">{saved ? t('import.draft.savedReadOnly') : t('import.draft.editBeforeSave')}</p>
    </div>
  );
}
