'use client';

import { createContext, useContext, useCallback, ReactNode } from 'react';
import { useConfig } from './ConfigContext';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import { useRecordingState } from './RecordingStateContext';

interface ImportDialogContextType {
  openImportDialog: (filePath?: string | null) => void;
}

const ImportDialogContext = createContext<ImportDialogContextType | null>(null);

export const useImportDialog = () => {
  const ctx = useContext(ImportDialogContext);
  if (!ctx) throw new Error('useImportDialog must be used within ImportDialogProvider');
  return ctx;
};

interface ImportDialogProviderProps {
  children: ReactNode;
  onOpen: (filePath?: string | null) => void;
}

export function ImportDialogProvider({ children, onOpen }: ImportDialogProviderProps) {
  const { t } = useTranslation('common');
  const { betaFeatures } = useConfig();
  const { isRecording } = useRecordingState();

  const openImportDialog = useCallback((filePath?: string | null) => {
    // Gate: Check beta feature flag before opening dialog
    if (!betaFeatures.importAndRetranscribe) {
      toast.error(t('errors.betaFeatureDisabled'), {
        description: t('actions.enableImportAudioAndRetranscribeInSettingsBetaToUse')
      });
      return;
    }

    if (isRecording) {
      toast.error(t('errors.importDuringRecording'));
      return;
    }

    onOpen(filePath);
  }, [onOpen, betaFeatures, isRecording, t]);

  return (
    <ImportDialogContext.Provider value={{ openImportDialog }}>
      {children}
    </ImportDialogContext.Provider>
  );
}
