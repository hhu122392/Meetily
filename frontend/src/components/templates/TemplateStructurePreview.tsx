'use client';

import { useTranslation } from 'react-i18next';
import { Eye, List, MessageSquareText, Text } from 'lucide-react';
import type { TemplateEditorDraft } from '@/lib/template-editor';

export function TemplateStructurePreview({ draft }: { draft: TemplateEditorDraft }) {
  const { t } = useTranslation('templates');
  return (
    <aside className="rounded-lg border border-gray-200 bg-white shadow-sm" aria-labelledby="template-preview-title">
      <div className="sticky top-0 z-[1] rounded-t-lg border-b border-gray-100 bg-white p-4">
        <div className="flex items-center gap-2">
          <Eye className="h-4 w-4 text-blue-600" aria-hidden="true" />
          <h2 id="template-preview-title" className="font-semibold">{t('editor.preview.title')}</h2>
        </div>
        <p className="mt-1 text-xs text-gray-500">{t('editor.preview.description')}</p>
      </div>
      <div className="space-y-6 p-5">
        <div>
          <div className="text-xs font-semibold uppercase tracking-wide text-gray-400">H1</div>
          <h1 className="mt-1 text-xl font-bold text-gray-900">{t('editor.preview.meetingTitle')}</h1>
        </div>

        {draft.sections.map((section, index) => {
          const title = section.title.trim() || t('editor.preview.untitled');
          const placeholder = section.emptyBehavior === 'show_not_mentioned'
            ? t('editor.preview.notMentioned')
            : t('editor.preview.placeholder');
          return (
            <section key={section.draftKey} className="border-l-2 border-blue-100 pl-4">
              <div className="flex items-center gap-2 text-xs font-medium text-gray-400">
                <span>H2 · {index + 1}</span>
                {section.format === 'list' ? <List className="h-3.5 w-3.5" aria-hidden="true" /> : section.format === 'paragraph' ? <MessageSquareText className="h-3.5 w-3.5" aria-hidden="true" /> : <Text className="h-3.5 w-3.5" aria-hidden="true" />}
                {section.required && <span className="rounded bg-blue-50 px-1.5 py-0.5 text-blue-600">{t('preview.required')}</span>}
              </div>
              <h2 className="mt-1 text-lg font-semibold text-gray-900">{title}</h2>
              {section.format === 'list' ? (
                <ul className="mt-2 space-y-1.5 text-sm text-gray-500">
                  <li className="flex gap-2"><span aria-hidden="true">•</span><span>{section.exampleItemFormat || section.itemFormat || placeholder}</span></li>
                  <li className="flex gap-2 opacity-60"><span aria-hidden="true">•</span><span>{placeholder}</span></li>
                </ul>
              ) : (
                <p className="mt-2 text-sm italic text-gray-500">{placeholder}</p>
              )}
            </section>
          );
        })}
      </div>
    </aside>
  );
}
