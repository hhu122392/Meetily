'use client';
import { useCallback } from 'react';
import { useUiLanguage } from './index';
import zh from './correction.zh-CN.json';
import en from './correction.en.json';

export function useCorrectionTranslation(_namespace?: string) {
  const language = useUiLanguage();
  const t = useCallback((key: string, params: Record<string, unknown> = {}): string => {
    if (key === 'accessibility.moreInformation') return language === 'en' ? 'More information' : '更多信息';
    const dictionary = language === 'en' ? en : zh;
    const lookup = (path: string) => path.split('.').reduce<unknown>((value, part) => value && typeof value === 'object' ? (value as Record<string, unknown>)[part] : undefined, dictionary);
    const plural = typeof params.count === 'number' ? lookup(key + (params.count === 1 ? '_one' : '_other')) : undefined;
    const value = plural ?? lookup(key);
    return (typeof value === 'string' ? value : String(params.defaultValue ?? key)).replace(/\{\{\s*(\w+)\s*\}\}/g, (_, name) => String(params[name] ?? ''));
  }, [language]);
  return { t };
}
