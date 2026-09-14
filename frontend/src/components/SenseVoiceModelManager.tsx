'use client';
import { Check, Download, Loader2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useSenseVoiceModel } from '@/hooks/useSenseVoiceModel';
import { DEFAULT_TRANSCRIPT_CONFIG } from '@/lib/sensevoice';
import { Button } from './ui/button';
import { HelpHint } from './ui/help-hint';

export function SenseVoiceModelManager({ selectedModel, onModelSelect }: { selectedModel?: string; onModelSelect: (name: string) => void }) {
  const { t } = useTranslation('models');
  const { state, download } = useSenseVoiceModel();
  const ready = state.status === 'available';
  const busy = state.status === 'checking' || state.status === 'downloading';
  const percent = Math.min(99, Math.round(state.downloaded_bytes / Math.max(1, state.total_bytes) * 100));
  const selected = ready && selectedModel === DEFAULT_TRANSCRIPT_CONFIG.model;
  return <div className="rounded-lg border border-gray-200 p-3 space-y-2">
    <div className="flex flex-wrap items-center justify-between gap-3">
      <div className="min-w-0">
        <div className="flex items-center gap-1 text-sm font-medium text-gray-800">
          SenseVoice Small (int8)<HelpHint>{t('sensevoice.help')}</HelpHint>
        </div>
        <p className="text-xs text-gray-500" role="status">{t(`sensevoice.${state.status}`)}{state.status === 'downloading' ? ` · ${percent}%` : ''}</p>
      </div>
      <Button type="button" size="sm" variant={selected ? 'outline' : 'default'} disabled={busy || selected}
        onClick={() => ready ? onModelSelect(DEFAULT_TRANSCRIPT_CONFIG.model) : void download()}>
        {busy ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : selected ? <Check className="mr-2 h-4 w-4" /> : !ready ? <Download className="mr-2 h-4 w-4" /> : null}
        {selected ? t('sensevoice.selected') : ready ? t('sensevoice.select') : busy ? t(`sensevoice.${state.status}`) : t(state.status === 'error' || state.status === 'partial' ? 'sensevoice.retry' : 'sensevoice.download')}
      </Button>
    </div>
    {state.status === 'downloading' && <div role="progressbar" aria-label={t('sensevoice.downloading')} aria-valuenow={percent} aria-valuemin={0} aria-valuemax={100} className="h-1 rounded bg-gray-100 overflow-hidden"><div className="h-full bg-blue-600 transition-all" style={{ width: `${percent}%` }} /></div>}
  </div>;
}
