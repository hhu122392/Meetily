'use client';

import { motion } from 'framer-motion';
import { FileQuestion, Sparkles } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { useTranslation } from 'react-i18next';

interface EmptyStateSummaryProps {
  onGenerate: () => void;
  hasModel: boolean;
  isGenerating?: boolean;
  isDisabled?: boolean;
  /** PRO 版式里生成按钮统一放在底部操作条，空态只保留说明 */
  showAction?: boolean;
}

export function EmptyStateSummary({
  onGenerate,
  hasModel,
  isGenerating = false,
  isDisabled = false,
  showAction = true,
}: EmptyStateSummaryProps) {
  const { t } = useTranslation('summary');
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col items-center justify-center h-full p-8 text-center"
    >
      <FileQuestion className="w-16 h-16 text-gray-300 mb-4" />
      <h3 className="text-lg font-semibold text-gray-900 mb-2">
        {t('labels.noSummaryGeneratedYet')}
      </h3>
      <p className="text-sm text-gray-500 mb-6 max-w-md">
        {t('descriptions.generateAnAIPoweredSummaryOfYourMeetingTranscriptTo')}
      </p>

      {showAction && (
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <div>
                <Button
                  onClick={onGenerate}
                  disabled={!hasModel || isGenerating || isDisabled}
                  className="gap-2"
                >
                  <Sparkles className="w-4 h-4" />
                  {isGenerating ? t('status.generating') : t('labels.generateSummary')}
                </Button>
              </div>
            </TooltipTrigger>
            {!hasModel && (
              <TooltipContent>
                <p>{t('labels.pleaseSelectAModelInSettingsFirst')}</p>
              </TooltipContent>
            )}
          </Tooltip>
        </TooltipProvider>
      )}

      {!hasModel && (
        <p className="text-xs text-amber-600 mt-3">
          {t('labels.pleaseSelectAModelInSettingsFirst')}
        </p>
      )}
    </motion.div>
  );
}
