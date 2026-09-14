'use client';

import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { FileSearch } from 'lucide-react';
import { HelpHint } from '@/components/ui/help-hint';
import type { SummaryFieldTrace } from '@/types/summary-source';
import { resolveSummaryEvidence, type EvidenceTranscript, type ResolvedSummaryTrace } from '@/lib/summary-evidence';

export function SummaryEvidencePanel({ traces, transcripts, disabled = false }: {
  traces: readonly SummaryFieldTrace[];
  transcripts: readonly EvidenceTranscript[];
  disabled?: boolean;
}) {
  const { t } = useTranslation('summary');
  const [resolved, setResolved] = useState<ResolvedSummaryTrace[]>([]);
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setResolved([]);
    if (disabled || !traces.length) { setLoading(false); return; }
    setLoading(true);
    void resolveSummaryEvidence(traces, transcripts).then(result => {
      if (!cancelled) setResolved(result);
    }).catch(() => {
      if (!cancelled) setResolved(traces.map(trace => ({ trace, sources: [] })));
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [traces, transcripts, disabled]);

  if (!traces.length) return null;
  return (
    <details className="mb-4 text-xs text-gray-600" data-summary-evidence>
      <summary className="w-fit cursor-pointer rounded py-1 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500">
        <FileSearch className="mr-1 inline-block h-3.5 w-3.5" aria-hidden="true" />
        {t('evidence.title', { count: traces.length })}
      </summary>
      <div className="mt-2 flex items-center gap-1">
        <span>{t('evidence.description')}</span>
        <HelpHint label={t('evidence.helpLabel')} text={t('evidence.help')} />
      </div>
      {disabled ? <p className="py-2">{t('evidence.refreshRequired')}</p>
        : loading ? <p className="py-2">{t('evidence.loading')}</p>
        : <div className="divide-y divide-gray-100">
          {resolved.map(({ trace, sources }, index) => (
            <details key={index} className="py-2" data-evidence-field={trace.field}>
              <summary className="cursor-pointer break-words leading-relaxed">
                <span className="text-gray-800">{trace.task || t('evidence.line', { line: trace.markdownLine })}</span>
                {' · '}{t(`evidence.fields.${trace.field}`)}：{trace.value}
                {trace.status === 'needs_review' && <span className="ml-2 text-amber-700">{t('evidence.needsReview')}</span>}
              </summary>
              <div className="mt-2 space-y-2 pl-3">
                {sources.length ? sources.map(source => (
                  <div key={source.segmentId}>
                    <p className="mb-1 text-gray-500">
                      {t(source.related ? 'evidence.related' : 'evidence.original')}
                      {source.startMs !== null && ` · ${Math.floor(source.startMs / 60000)}:${String(Math.floor(source.startMs / 1000) % 60).padStart(2, '0')}`}
                    </p>
                    <blockquote className="border-l-2 border-gray-200 pl-3 text-sm leading-relaxed text-gray-700">{source.text}</blockquote>
                  </div>
                )) : <p>{t('evidence.unavailable')}</p>}
              </div>
            </details>
          ))}
        </div>}
    </details>
  );
}
