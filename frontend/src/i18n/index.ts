'use client';

import { useEffect, useSyncExternalStore } from 'react';
import translations from './zh-CN.json';
import englishTranslations from './en.json';

export type UiLanguage = 'zh-CN' | 'en';
export const LANGUAGE_STORAGE_KEY = 'meetily.ui-language';
const dictionary: Record<string, string> = translations;
const englishDictionary: Record<string, string> = englishTranslations;
let language: UiLanguage = 'zh-CN';
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};

export function setUiLanguage(next: UiLanguage) {
  language = next;
  if (typeof window !== 'undefined') {
    try { window.localStorage.setItem(LANGUAGE_STORAGE_KEY, next); } catch { /* Session-only preference. */ }
    document.documentElement.lang = next;
  }
  listeners.forEach(listener => listener());
}

export function t(message: string): string {
  return language === 'zh-CN' ? dictionary[message] ?? message : englishDictionary[message] ?? message;
}

// Subscribe without remounting editors or recording providers on language changes.
export function useUiLanguage() {
  return useSyncExternalStore(subscribe, () => language, () => 'zh-CN' as UiLanguage);
}

export function useInitializeUiLanguage() {
  const current = useUiLanguage();
  useEffect(() => {
    try {
      const saved = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
      setUiLanguage(saved === 'en' ? 'en' : 'zh-CN');
    } catch { document.documentElement.lang = language; }
    const onStorage = (event: StorageEvent) => {
      if (event.key === LANGUAGE_STORAGE_KEY) {
        language = event.newValue === 'en' ? 'en' : 'zh-CN';
        document.documentElement.lang = language;
        listeners.forEach(listener => listener());
      }
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);
  return current;
}
