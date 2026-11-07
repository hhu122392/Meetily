"use client";
import { Button } from '@/components/ui/button';
import { Copy, Save, Loader2 } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { t, useUiLanguage } from '@/i18n';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
}

export function SummaryUpdaterButtonGroup({ isSaving, isDirty, onSave, onCopy }: SummaryUpdaterButtonGroupProps) {
  useUiLanguage();
  const iconClass = 'h-8 w-8 shrink-0 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900';
  return <div className="flex items-center gap-0.5">
    <Button variant="ghost" size="icon" className={iconClass}
      title={t('Copy Summary')} aria-label={t('Copy Summary')}
      onClick={() => { Analytics.trackButtonClick('copy_summary', 'meeting_details'); void onCopy(); }}>
      <Copy className="h-4 w-4" />
    </Button>
    {(isDirty || isSaving) && <Button variant="ghost" size="icon"
      className={`${iconClass} ${isDirty ? 'bg-green-50 text-green-700 hover:bg-green-100 hover:text-green-800' : ''}`}
      title={isSaving ? t('Saving') : t('Save Changes')} aria-label={isSaving ? t('Saving') : t('Save Changes')}
      disabled={isSaving}
      onClick={() => { Analytics.trackButtonClick('save_changes', 'meeting_details'); void onSave(); }}>
      {isSaving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
    </Button>}
  </div>;
}
