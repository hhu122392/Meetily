'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { useRouter } from 'next/navigation';
import { ChevronRight, FolderOpen, LayoutTemplate, RefreshCw, TriangleAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { templateService, normalizeTemplateApiError } from '@/services/templateService';
import type {
  ListTemplatesResponse,
  TemplateApiError,
  TemplatesDirectoryInfo,
} from '@/types/summary-template';
import { templateErrorToastId, templateTranslationKey } from '@/lib/template-library';

interface SummaryTemplateCardState {
  data: ListTemplatesResponse | null;
  directory: TemplatesDirectoryInfo | null;
  error: TemplateApiError | null;
  loading: boolean;
}

export function SummaryTemplateSettings() {
  const router = useRouter();
  const { t, i18n } = useTranslation('templates');
  const [state, setState] = useState<SummaryTemplateCardState>({
    data: null,
    directory: null,
    error: null,
    loading: true,
  });
  const [openingFolder, setOpeningFolder] = useState(false);
  const mounted = useRef(true);

  const load = useCallback(async () => {
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      const [data, directory] = await Promise.all([
        templateService.list({
          includeInvalid: false,
          contentLocale: i18n.resolvedLanguage ?? i18n.language,
        }),
        templateService.getDirectory(),
      ]);
      if (!mounted.current) return;
      setState({ data, directory, error: null, loading: false });
    } catch (error) {
      if (!mounted.current) return;
      setState((current) => ({
        ...current,
        error: normalizeTemplateApiError(error),
        loading: false,
      }));
    }
  }, [i18n.language, i18n.resolvedLanguage]);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
    };
  }, [load]);

  const openFolder = async () => {
    if (openingFolder) return;
    setOpeningFolder(true);
    try {
      await templateService.openDirectory();
      toast.success(t('actions.folderOpened'));
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      toast.error(t(templateTranslationKey(normalized.messageKey), {
        defaultValue: t('errors.io'),
      }), {
        id: templateErrorToastId('open-directory', normalized),
      });
    } finally {
      setOpeningFolder(false);
    }
  };

  const customCount = state.data?.templates.filter((item) => item.origin === 'custom').length ?? 0;
  const defaultId = state.data?.defaultTemplateId ?? 'standard_meeting';
  const defaultTemplate = state.data?.templates.find((item) => item.id === defaultId);

  return (
    <section className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm" aria-labelledby="summary-templates-title">
      <div className="flex flex-col gap-5 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0">
          <div className="mb-2 flex items-center gap-2">
            <LayoutTemplate className="h-5 w-5 text-blue-600" aria-hidden="true" />
            <h3 id="summary-templates-title" className="text-lg font-semibold text-gray-900">
              {t('settingsCard.title')}
            </h3>
          </div>
          <p className="max-w-2xl text-sm text-gray-600">{t('settingsCard.description')}</p>
        </div>
        <Button
          type="button"
          variant="outline"
          onClick={() => router.push('/settings/templates')}
          className="shrink-0"
        >
          {t('settingsCard.manage')}
          <ChevronRight aria-hidden="true" />
        </Button>
      </div>

      {state.loading ? (
        <div className="mt-5 space-y-2" aria-live="polite" aria-label={t('library.loading')}>
          <div className="h-4 w-56 animate-pulse rounded bg-gray-200" />
          <div className="h-4 w-36 animate-pulse rounded bg-gray-100" />
        </div>
      ) : state.error ? (
        <div className="mt-5 flex flex-wrap items-center gap-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
          <TriangleAlert className="h-4 w-4 shrink-0" aria-hidden="true" />
          <span className="flex-1">{t('settingsCard.unavailable')}</span>
          <Button type="button" size="sm" variant="outline" onClick={() => void load()}>
            <RefreshCw aria-hidden="true" />
            {t('library.retry')}
          </Button>
        </div>
      ) : (
        <div className="mt-5 grid gap-3 sm:grid-cols-2">
          <div className="rounded-md border border-gray-200 bg-gray-50 px-4 py-3">
            <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
              {t('settingsCard.defaultLabel')}
            </div>
            <div className="mt-1 truncate font-medium text-gray-900">
              {defaultTemplate?.name ?? defaultId}
            </div>
          </div>
          <div className="rounded-md border border-gray-200 bg-gray-50 px-4 py-3">
            <div className="text-xs font-medium uppercase tracking-wide text-gray-500">
              {t('card.custom')}
            </div>
            <div className="mt-1 font-medium text-gray-900">
              {t('settingsCard.customCount', { count: customCount })}
            </div>
          </div>
        </div>
      )}

      {!state.loading && state.directory && !state.directory.writable && (
        <div className="mt-4 flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-900" role="status">
          <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
          <span>{t('settingsCard.notWritable')}</span>
        </div>
      )}

      <div className="mt-5 flex flex-wrap items-center gap-3 border-t border-gray-100 pt-4">
        <Button type="button" variant="ghost" size="sm" onClick={() => void openFolder()} disabled={openingFolder}>
          <FolderOpen aria-hidden="true" />
          {t('settingsCard.openFolder')}
        </Button>
        {!state.loading && !state.error && customCount === 0 && (
          <p className="text-xs text-gray-500">{t('settingsCard.noCustom')}</p>
        )}
      </div>
    </section>
  );
}
