'use client';

import { t, useUiLanguage } from '@/i18n';

// A client boundary for static-export pages that cannot use hooks themselves.
export function UiText({ message }: { message: string }) {
  useUiLanguage();
  return <>{t(message)}</>;
}
