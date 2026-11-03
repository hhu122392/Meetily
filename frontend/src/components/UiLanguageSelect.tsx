'use client';

import { setUiLanguage, t, useUiLanguage, type UiLanguage } from '@/i18n';

export function UiLanguageSelect() {
  const language = useUiLanguage();
  return <div className="flex items-center justify-between gap-4 rounded-lg border bg-white p-4">
    <div>
      <label htmlFor="ui-language" className="text-sm font-medium">{t('Interface language')}</label>
      <p className="text-xs text-gray-500 mt-1">{t('This does not change transcription or summary language.')}</p>
    </div>
    <select id="ui-language" value={language} onChange={event => setUiLanguage(event.target.value as UiLanguage)}
      className="rounded-md border px-3 py-2 text-sm bg-white">
      <option value="zh-CN">简体中文</option>
      <option value="en">English</option>
    </select>
  </div>;
}
