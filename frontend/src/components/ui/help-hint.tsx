'use client';

import type { ReactNode } from 'react';
import { HelpCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Popover, PopoverContent, PopoverTrigger } from './popover';

/** Ordinary explanations share one neutral, keyboard-accessible question mark. */
export function HelpHint({ text, children, label }: { text?: ReactNode; children?: ReactNode; label?: string }) {
  const { t } = useTranslation('common');
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button type="button" data-help-hint="true" aria-label={label ?? t('accessibility.moreInformation')}
          className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-gray-500 hover:bg-gray-100 hover:text-gray-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500">
          <HelpCircle className="h-4 w-4" aria-hidden="true" />
        </button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" aria-label={label ?? t('accessibility.moreInformation')}
        className="max-h-[min(50vh,24rem)] w-80 max-w-[calc(100vw-2rem)] overflow-y-auto break-words p-3 text-xs leading-relaxed text-gray-600">
        {text}{children}
      </PopoverContent>
    </Popover>
  );
}
