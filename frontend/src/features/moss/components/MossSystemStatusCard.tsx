'use client';

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { AlertCircle, CheckCircle2, Cpu, HardDrive, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { mossReviewService, normalizeMossApiError, type MossReviewService } from '../service';
import { MOSS_ERROR_I18N_KEYS, type MossApiError, type MossSystemStatus } from '../types';
import { formatMossBytes, shortMossHash } from '../utils';

export function MossSystemStatusCard({
  enabled,
  service = mossReviewService,
}: {
  enabled: boolean;
  service?: MossReviewService;
}) {
  const { t, i18n } = useTranslation('moss');
  const [status, setStatus] = useState<MossSystemStatus | null>(null);
  const [error, setError] = useState<MossApiError | null>(null);
  const [loading, setLoading] = useState(false);
  const requestSequence = useRef(0);

  const load = useCallback(async () => {
    if (!enabled) return;
    const sequence = ++requestSequence.current;
    setLoading(true);
    setError(null);
    try {
      const next = await service.getSystemStatus();
      if (sequence !== requestSequence.current) return;
      setStatus(next);
    } catch (loadError) {
      if (sequence !== requestSequence.current) return;
      setStatus(null);
      setError(normalizeMossApiError(loadError));
    } finally {
      if (sequence === requestSequence.current) setLoading(false);
    }
  }, [enabled, service]);

  useEffect(() => {
    if (!enabled) {
      requestSequence.current += 1;
      setStatus(null);
      setError(null);
      setLoading(false);
      return;
    }
    void load();
  }, [enabled, load]);

  const fields = status ? [
    [t('settings.installation'), status.installed ? t('settings.installed') : t('settings.notInstalled')],
    [t('settings.version'), status.version ?? '—'],
    [t('settings.runtimeHash'), shortMossHash(status.runtimeSha256)],
    [t('settings.modelHash'), shortMossHash(status.modelSha256)],
    [t('settings.modelSize'), formatMossBytes(status.modelBytes, i18n.resolvedLanguage ?? 'en')],
    [t('settings.availableDisk'), formatMossBytes(status.availableDiskBytes, i18n.resolvedLanguage ?? 'en')],
    [t('settings.device'), status.deviceName ?? '—'],
    [t('settings.health'), t(`health.${status.health}`)],
    [t('settings.nativeHotwords'), t('settings.nativeHotwordsUnsupported')],
  ] : [];

  return (
    <section
      className="rounded-lg border border-gray-200 bg-white p-6 shadow-sm"
      aria-labelledby="moss-system-status-title"
    >
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h2 id="moss-system-status-title" className="flex items-center gap-2 text-lg font-semibold text-gray-900">
            <Cpu className="h-5 w-5 text-blue-600" aria-hidden="true" />
            {t('settings.title')}
          </h2>
          <p className="mt-1 text-sm text-gray-600">{t('settings.description')}</p>
        </div>
        {enabled && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void load()}
            disabled={loading}
            aria-label={t('settings.refresh')}
          >
            <RefreshCw className={`h-4 w-4 ${loading ? 'animate-spin' : ''}`} aria-hidden="true" />
            {t('actions.refresh')}
          </Button>
        )}
      </div>

      {!enabled && (
        <Alert className="mt-4 border-amber-200 bg-amber-50 text-amber-900">
          <AlertCircle className="h-4 w-4" aria-hidden="true" />
          <AlertDescription>{t('settings.disabled')}</AlertDescription>
        </Alert>
      )}

      {enabled && loading && !status && (
        <p className="mt-4 text-sm text-gray-600" role="status" aria-live="polite">
          {t('settings.loading')}
        </p>
      )}

      {enabled && error && (
        <Alert variant="destructive" className="mt-4">
          <AlertCircle className="h-4 w-4" aria-hidden="true" />
          <AlertTitle>{t(MOSS_ERROR_I18N_KEYS[error.code])}</AlertTitle>
          <AlertDescription>{t('workspace.debugReference', { debugId: error.debugId })}</AlertDescription>
        </Alert>
      )}

      {enabled && status && (
        <div className="mt-5" aria-live="polite">
          <div className={`flex items-center gap-2 rounded-md p-3 text-sm ${
            status.availability === 'ready'
              ? 'bg-emerald-50 text-emerald-900'
              : 'bg-amber-50 text-amber-900'
          }`}>
            {status.availability === 'ready'
              ? <CheckCircle2 className="h-4 w-4" aria-hidden="true" />
              : <AlertCircle className="h-4 w-4" aria-hidden="true" />}
            <span className="font-medium">{t(`availability.${status.availability}`)}</span>
          </div>

          <dl className="mt-4 grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2">
            {fields.map(([label, value]) => (
              <div key={label} className="min-w-0 border-b border-gray-100 pb-2">
                <dt className="text-gray-500">{label}</dt>
                <dd className="mt-1 break-all font-medium text-gray-900">{value}</dd>
              </div>
            ))}
          </dl>

          <div className="mt-4 flex items-start gap-2 rounded-md border border-blue-200 bg-blue-50 p-3 text-sm text-blue-900">
            <HardDrive className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
            <p>{t('settings.nativeHotwordsNote')}</p>
          </div>
        </div>
      )}
    </section>
  );
}
