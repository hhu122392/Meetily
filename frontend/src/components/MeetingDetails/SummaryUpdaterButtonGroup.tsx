"use client";

import { Button } from '@/components/ui/button';
import { Copy, Save, Loader2 } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { useTranslation } from 'react-i18next';

interface SummaryUpdaterButtonGroupProps {
  isSaving: boolean;
  isDirty: boolean;
  onSave: () => Promise<void>;
  onCopy: () => Promise<void>;
  onFind?: () => void;
  hasSummary: boolean;
}

/**
 * PRO 版式：底部悬浮操作条里的两个图标按钮。
 * 保存只在有改动（dirty）时出现，没有改动时不再常驻占位。
 */
export function SummaryUpdaterButtonGroup({
  isSaving,
  isDirty,
  onSave,
  onCopy,
  onFind,
  hasSummary
}: SummaryUpdaterButtonGroupProps) {
  const { t } = useTranslation(['meetings', 'summary']);
  const iconButtonClass = 'h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900';

  return (
    <div className="flex items-center gap-0.5">
      {/* Copy button */}
      <Button
        variant="ghost"
        size="icon"
        className={iconButtonClass}
        title={t('meetings:accessibilityActions.copySummary')}
        aria-label={t('meetings:accessibilityActions.copySummary')}
        onClick={() => {
          Analytics.trackButtonClick('copy_summary', 'meeting_details');
          onCopy();
        }}
        disabled={!hasSummary}
      >
        <Copy className="h-4 w-4" />
      </Button>

      {/* Save button：仅在有未保存改动时出现 */}
      {(isDirty || isSaving) && (
        <Button
          variant="ghost"
          size="icon"
          className={`${iconButtonClass} ${isDirty ? 'bg-green-50 text-green-700 hover:bg-green-100 hover:text-green-800' : ''}`}
          title={isSaving ? t('summary:status.processing') : t('meetings:actions.saveChanges')}
          aria-label={isSaving ? t('summary:status.processing') : t('meetings:accessibilityActions.saveChanges')}
          onClick={() => {
            Analytics.trackButtonClick('save_changes', 'meeting_details');
            void onSave().catch(() => undefined);
          }}
          disabled={isSaving}
        >
          {isSaving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
        </Button>
      )}

      {/* Find button */}
      {/* {onFind && (
        <Button
          variant="outline"
          size="sm"
          title="Find in Summary"
          onClick={() => {
            Analytics.trackButtonClick('find_in_summary', 'meeting_details');
            onFind();
          }}
          disabled={!hasSummary}
          className="cursor-pointer"
        >
          <Search />
          <span className="hidden lg:inline">Find</span>
        </Button>
      )} */}
    </div>
  );
}
