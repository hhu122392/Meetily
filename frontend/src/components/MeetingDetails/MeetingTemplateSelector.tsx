'use client';

import { HelpHint } from '@/components/ui/help-hint';
import { useMemo, useState } from 'react';
import { useRouter } from 'next/navigation';
import { useTranslation } from 'react-i18next';
import {
  Check,
  FileText,
  Globe2,
  Loader2,
  Search,
  Settings2,
  TriangleAlert,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import type {
  MeetingTemplateIssue,
  MeetingTemplateMode,
  MeetingTemplateStorage,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';

interface MeetingTemplateSelectorProps {
  templates: TemplateListItem[];
  selectedTemplateId: string;
  selectedTemplateName: string;
  preferenceMode: MeetingTemplateMode | null;
  storage: MeetingTemplateStorage | null;
  issue: MeetingTemplateIssue | null;
  error: TemplateApiError | null;
  isLoading: boolean;
  isSaving: boolean;
  disabled?: boolean;
  /** PRO 版式：顶部图标簇里只显示图标，文字靠 tooltip 表达 */
  trigger?: 'default' | 'icon';
  onSelect: (templateId: string, templateName: string) => void;
  onUseGlobalDefault: () => void;
  onRetry: () => void;
}

export function MeetingTemplateSelector({
  templates,
  selectedTemplateId,
  selectedTemplateName,
  preferenceMode,
  storage,
  issue,
  error,
  isLoading,
  isSaving,
  disabled = false,
  trigger = 'default',
  onSelect,
  onUseGlobalDefault,
  onRetry,
}: MeetingTemplateSelectorProps) {
  const { t } = useTranslation('summary');
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const filtered = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!normalized) return templates;
    return templates.filter((template) => [template.name, template.description, template.id]
      .some((value) => value.toLocaleLowerCase().includes(normalized)));
  }, [query, templates]);
  const custom = filtered.filter((template) => template.origin === 'custom');
  const builtIn = filtered.filter((template) => template.origin !== 'custom');
  const busy = isLoading || isSaving;

  const choose = (template: TemplateListItem) => {
    setOpen(false);
    onSelect(template.id, template.name);
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {trigger === 'icon' ? (
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
            disabled={disabled || busy}
            title={`${t('templatePreference.select')}：${selectedTemplateName}`}
            aria-label={`${t('templatePreference.select')}：${selectedTemplateName}`}
            aria-busy={busy}
          >
            {busy ? <Loader2 className="animate-spin" /> : <FileText />}
          </Button>
        ) : (
          <Button
            variant="outline"
            size="sm"
            disabled={disabled || busy}
            title={t('templatePreference.select')}
            aria-label={t('templatePreference.select')}
            aria-busy={busy}
          >
            {busy ? <Loader2 className="animate-spin" /> : <FileText />}
            <span className="hidden max-w-44 truncate lg:inline">{selectedTemplateName}</span>
          </Button>
        )}
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[min(24rem,calc(100vw-2rem))] p-0">
        <div className="border-b p-3">
          <p className="font-semibold text-gray-900">{t('templatePreference.title')}</p>
          <p className="mt-1 text-xs text-gray-500">
            {preferenceMode === 'meeting_override'
              ? t('templatePreference.meetingOverride')
              : t('templatePreference.inheritsGlobal')}
            {storage && ` · ${storage === 'metadata'
              ? t('templatePreference.savedWithMeeting')
              : t('templatePreference.savedLocally')}`}
          </p>
        </div>

        {(issue || error) && (
          <div className="m-3 flex gap-2 text-sm text-amber-800" role="alert">
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <div>
              <p className="font-medium">
                {issue ? t('templatePreference.unavailableTitle') : t('templatePreference.loadFailed')}
              </p>
              <HelpHint text={issue
                ? t('templatePreference.unavailableDescription', { templateId: issue.selectedTemplateId })
                : t('templatePreference.retryDescription')} />
              {error && (
                <button
                  type="button"
                  className="mt-2 text-xs font-semibold underline underline-offset-2"
                  onClick={onRetry}
                >
                  {t('templatePreference.retry')}
                </button>
              )}
            </div>
          </div>
        )}

        <div className="relative px-3 pt-3">
          <Search className="pointer-events-none absolute left-6 top-5.5 h-4 w-4 text-gray-400" aria-hidden="true" />
          <Input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t('templatePreference.search')}
            aria-label={t('templatePreference.search')}
            className="pl-9"
          />
        </div>

        <div className="max-h-72 overflow-y-auto p-2 custom-scrollbar">
          {custom.length > 0 && (
            <TemplateGroup
              label={t('templatePreference.custom')}
              templates={custom}
              selectedTemplateId={selectedTemplateId}
              preferenceMode={preferenceMode}
              onChoose={choose}
            />
          )}
          {builtIn.length > 0 && (
            <TemplateGroup
              label={t('templatePreference.builtIn')}
              templates={builtIn}
              selectedTemplateId={selectedTemplateId}
              preferenceMode={preferenceMode}
              onChoose={choose}
            />
          )}
          {filtered.length === 0 && (
            <p className="px-3 py-8 text-center text-sm text-gray-500">
              {t('templatePreference.noResults')}
            </p>
          )}
        </div>

        <div className="space-y-1 border-t p-2">
          <button
            type="button"
            className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-gray-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
            onClick={() => {
              setOpen(false);
              onUseGlobalDefault();
            }}
          >
            <Globe2 className="h-4 w-4" aria-hidden="true" />
            <span className="flex-1">{t('templatePreference.useGlobalDefault')}</span>
            {preferenceMode === 'inherit' && <Check className="h-4 w-4 text-green-600" aria-hidden="true" />}
          </button>
          <button
            type="button"
            className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-blue-700 hover:bg-blue-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
            onClick={() => router.push('/settings/templates')}
          >
            <Settings2 className="h-4 w-4" aria-hidden="true" />
            {t('templatePreference.manage')}
          </button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

function TemplateGroup({
  label,
  templates,
  selectedTemplateId,
  preferenceMode,
  onChoose,
}: {
  label: string;
  templates: TemplateListItem[];
  selectedTemplateId: string;
  preferenceMode: MeetingTemplateMode | null;
  onChoose: (template: TemplateListItem) => void;
}) {
  return (
    <div className="mb-2 last:mb-0">
      <p className="px-3 py-1 text-[11px] font-semibold uppercase tracking-wide text-gray-400">{label}</p>
      {templates.map((template) => {
        const selected = preferenceMode === 'meeting_override' && template.id === selectedTemplateId;
        return (
          <button
            key={`${template.origin}:${template.id}`}
            type="button"
            className="flex w-full items-start gap-3 rounded-md px-3 py-2 text-left hover:bg-gray-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
            onClick={() => onChoose(template)}
            title={template.description}
          >
            <div className="min-w-0 flex-1">
              <p className="truncate text-sm font-medium text-gray-900">{template.name}</p>
              <p className="mt-0.5 truncate text-xs text-gray-500">
                {template.sectionCount} · {template.locale ?? template.id}
              </p>
            </div>
            {selected && <Check className="mt-1 h-4 w-4 shrink-0 text-green-600" aria-hidden="true" />}
          </button>
        );
      })}
    </div>
  );
}
