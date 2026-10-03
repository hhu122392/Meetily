"use client";

import React, { useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { en, zh, type Dictionary } from '@blocknote/core/locales';

export function useBlockNoteLocale() {
  const { i18n } = useTranslation();
  const locale = (i18n.resolvedLanguage || i18n.language).startsWith('zh') ? 'zh' : 'en';
  const state = useRef<{ locale: string; dictionary: Dictionary }>();
  const source = locale === 'zh' ? zh : en;
  if (!state.current) state.current = { locale, dictionary: { ...source } };
  if (state.current.locale !== locale) {
    // Keep the editor's dictionary reference; recreating it would discard undo/selection.
    Object.assign(state.current.dictionary, source);
    state.current.locale = locale;
  }
  return { locale, dictionary: state.current.dictionary };
}

export function BlockNoteLocale({ locale, dictionary, children }: {
  locale: string; dictionary: Dictionary; children: ReactNode;
}) {
  // BlockNote 0.36 captures placeholder CSS at mount; override only its empty-block
  // selectors, scoped to this editor, so a live language change needs no transaction.
  const selector = `[data-meetily-blocknote-locale="${locale}"] .bn-editor .bn-block-content`;
  const rule = (suffix: string, text?: string) =>
    `${selector}${suffix} .bn-inline-content:has(> .ProseMirror-trailingBreak:only-child)::before { content: ${text === undefined ? 'none' : JSON.stringify(text)}; }`;
  const { default: hint, emptyDocument, ...blocks } = dictionary.placeholders;
  const css = Object.entries(blocks).map(([type, text]) => rule(`[data-content-type="${type}"]`, text)).join('\n')
    + rule('[data-is-only-empty-block]', emptyDocument) + rule('[data-is-empty-and-focused]', hint);
  return <div data-meetily-blocknote-locale={locale}><style>{css}</style>{children}</div>;
}
