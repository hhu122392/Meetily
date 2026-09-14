'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { templateService, type TemplateService } from '@/services/templateService';
import type {
  DefaultTemplatePreference,
  DeleteTemplateRequest,
  DuplicateTemplateRequest,
  ListTemplatesResponse,
  RestoreTemplateRequest,
  TemplateApiError,
  TemplateDetails,
  TemplatesDirectoryInfo,
} from '@/types/summary-template';
import { normalizeTemplateApiError } from '@/services/templateService';
import { shouldApplyTemplateResponse } from '@/lib/template-library';

interface TemplateLibraryState {
  data: ListTemplatesResponse | null;
  directory: TemplatesDirectoryInfo | null;
  loading: boolean;
  error: TemplateApiError | null;
}

export function useTemplateLibrary(
  service: TemplateService = templateService,
  contentLocale?: string,
) {
  const [state, setState] = useState<TemplateLibraryState>({
    data: null,
    directory: null,
    loading: true,
    error: null,
  });
  const [pendingOperations, setPendingOperations] = useState<ReadonlySet<string>>(new Set());
  const requestSequence = useRef(0);
  const mounted = useRef(true);
  const pendingOperationKeys = useRef(new Set<string>());

  useEffect(() => () => {
    mounted.current = false;
  }, []);

  const refresh = useCallback(async () => {
    const sequence = ++requestSequence.current;
    setState((current) => ({ ...current, loading: true, error: null }));

    try {
      const [data, directory] = await Promise.all([
        service.list({ includeInvalid: true, includeTrash: true, contentLocale }),
        service.getDirectory(),
      ]);

      if (!mounted.current || !shouldApplyTemplateResponse(sequence, requestSequence.current)) return;
      setState({ data, directory, loading: false, error: null });
    } catch (error) {
      if (!mounted.current || !shouldApplyTemplateResponse(sequence, requestSequence.current)) return;
      setState((current) => ({
        ...current,
        loading: false,
        error: normalizeTemplateApiError(error),
      }));
    }
  }, [contentLocale, service]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runOperation = useCallback(async <T,>(
    operationKey: string,
    operation: () => Promise<T>,
    refreshAfter = true,
  ): Promise<T | null> => {
    if (pendingOperationKeys.current.has(operationKey)) return null;
    pendingOperationKeys.current.add(operationKey);
    setPendingOperations((current) => new Set(current).add(operationKey));

    try {
      const result = await operation();
      if (refreshAfter) await refresh();
      return result;
    } finally {
      if (mounted.current) {
        pendingOperationKeys.current.delete(operationKey);
        setPendingOperations((current) => {
          const next = new Set(current);
          next.delete(operationKey);
          return next;
        });
      }
    }
  }, [refresh]);

  return {
    ...state,
    pendingOperations,
    refresh,
    getDetails: (templateId: string, origin?: 'builtin' | 'bundled' | 'custom') =>
      runOperation<TemplateDetails>(
        `preview:${templateId}`,
        () => service.get({ templateId, origin, contentLocale }),
        false,
      ),
    openDirectory: () => runOperation<void>('open-directory', () => service.openDirectory(), false),
    setDefault: (templateId: string | null) =>
      runOperation<DefaultTemplatePreference>('set-default', () => service.setDefault(templateId)),
    duplicate: (request: DuplicateTemplateRequest) =>
      runOperation<TemplateDetails>(`duplicate:${request.templateId}`, () => service.duplicate(request)),
    deleteTemplate: (request: DeleteTemplateRequest) =>
      runOperation(`delete:${request.templateId}`, () => service.delete(request)),
    restore: (request: RestoreTemplateRequest) =>
      runOperation<TemplateDetails>(`restore:${request.trashId}`, () => service.restore(request)),
    // 回收站里的模板可以永久删除（文件直接从 .trash 移除，不可恢复）
    purge: (trashId: string) =>
      runOperation<void>(`purge:${trashId}`, () => service.purge(trashId)),
  };
}
