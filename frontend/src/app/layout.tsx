'use client'

import './globals.css'
import { Source_Sans_3 } from 'next/font/google'
import Sidebar from '@/components/Sidebar'
import { SidebarProvider } from '@/components/Sidebar/SidebarProvider'
import MainContent from '@/components/MainContent'
import AnalyticsProvider from '@/components/AnalyticsProvider'
import { Toaster, toast } from 'sonner'
import "sonner/dist/styles.css"
import { useState, useEffect, useCallback } from 'react'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RecordingStateProvider } from '@/contexts/RecordingStateContext'
import { OllamaDownloadProvider } from '@/contexts/OllamaDownloadContext'
import { TranscriptProvider } from '@/contexts/TranscriptContext'
import { ConfigProvider, useConfig } from '@/contexts/ConfigContext'
import { OnboardingProvider } from '@/contexts/OnboardingContext'
import { OnboardingFlow } from '@/components/onboarding'
import { loadBetaFeatures } from '@/types/betaFeatures'
import { DownloadProgressToastProvider } from '@/components/shared/DownloadProgressToast'
import { UpdateCheckProvider } from '@/components/UpdateCheckProvider'
import { RecordingPostProcessingProvider } from '@/contexts/RecordingPostProcessingProvider'
import { ImportAudioDialog, ImportDropOverlay } from '@/components/ImportAudio'
import { ImportDialogProvider } from '@/contexts/ImportDialogContext'
import { isAudioExtension, getAudioFormatsDisplayList } from '@/constants/audioFormats'
import { shouldDelegateDropToTemplateImport } from '@/lib/file-drop-routing'
import { MeetilyI18nProvider } from '@/i18n/I18nProvider'
import { UI_LOCALE_BOOTSTRAP_SCRIPT } from '@/i18n/locale'
import { i18n } from '@/i18n'
import { useTranslation } from 'react-i18next'
import { DOCUMENT_TITLE } from '@/constants/app'


const sourceSans3 = Source_Sans_3({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-source-sans-3',
})

// Module-level component — stable reference across RootLayout re-renders.
// Defined here (not inside RootLayout) so React never sees a new function type
// on re-render, which would cause unmount/remount and break initialization logic.
function ConditionalImportDialog({
  showImportDialog,
  handleImportDialogClose,
  importFilePath,
}: {
  showImportDialog: boolean;
  handleImportDialogClose: (open: boolean) => void;
  importFilePath: string | null;
}) {
  const { betaFeatures } = useConfig();

  // Only mount ImportAudioDialog (and its hooks/listeners) when feature is enabled
  if (!betaFeatures.importAndRetranscribe) {
    return null;
  }

  return (
    <ImportAudioDialog
      open={showImportDialog}
      onOpenChange={handleImportDialogClose}
      preselectedFile={importFilePath}
    />
  );
}

function LocalizedToaster() {
  const { t } = useTranslation('onboarding')

  return (
    <Toaster
      position="bottom-center"
      richColors
      closeButton
      containerAriaLabel={t('accessibility.notifications')}
      toastOptions={{ closeButtonAriaLabel: t('accessibility.closeNotification') }}
    />
  )
}

function LocalizedSetupLoading() {
  const { t } = useTranslation('onboarding')

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-gray-50 text-gray-700"
      role="status"
      aria-live="polite"
    >
      {t('status.loadingSetup')}
    </div>
  )
}

// export { metadata } from './metadata'

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const [showOnboarding, setShowOnboarding] = useState(false)
  const [onboardingStatusChecked, setOnboardingStatusChecked] = useState(false)

  // Import audio state
  const [showDropOverlay, setShowDropOverlay] = useState(false)
  const [showImportDialog, setShowImportDialog] = useState(false)
  const [importFilePath, setImportFilePath] = useState<string | null>(null)

  useEffect(() => {
    // Check onboarding status first
    invoke<{ completed: boolean } | null>('get_onboarding_status')
      .then((status) => {
        const isComplete = status?.completed ?? false
        if (!isComplete) {
          console.log('[Layout] Onboarding not completed, showing onboarding flow')
          setShowOnboarding(true)
        } else {
          console.log('[Layout] Onboarding completed, showing main app')
        }
      })
      .catch((error) => {
        console.error('[Layout] Failed to check onboarding status:', error)
        // Default to showing onboarding if we can't check
        setShowOnboarding(true)
      })
      .finally(() => setOnboardingStatusChecked(true))
  }, [])

  // Disable context menu in production
  useEffect(() => {
    if (process.env.NODE_ENV === 'production') {
      const handleContextMenu = (e: MouseEvent) => e.preventDefault();
      document.addEventListener('contextmenu', handleContextMenu);
      return () => document.removeEventListener('contextmenu', handleContextMenu);
    }
  }, []);
  useEffect(() => {
    // Listen for tray recording toggle request
    const unlisten = listen('request-recording-toggle', () => {
      console.log('[Layout] Received request-recording-toggle from tray');

      if (showOnboarding) {
        toast.error(i18n.t('onboarding:errors.completeSetupBeforeRecording'), {
          description: i18n.t('onboarding:descriptions.finishOnboardingBeforeRecording')
        });
      } else {
        // Route native requests through SidebarProvider so navigation and the
        // process-bound one-shot authorization use the same guarded path.
        console.log('[Layout] Forwarding native recording request');
        window.dispatchEvent(new CustomEvent('request-recording-from-tray'));
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [showOnboarding]);

  // Handle file drop for audio import
  const handleFileDrop = useCallback(async (paths: string[]) => {
    // The template library owns JSON/Word drops. Tauri delivers drag events to
    // every listener, so the global audio importer must explicitly yield here.
    if (shouldDelegateDropToTemplateImport(window.location.pathname, paths)) {
      return;
    }

    // Check if beta features are enabled (read from localStorage directly since we're outside ConfigProvider)
    const betaFeatures = loadBetaFeatures();

    if (!betaFeatures.importAndRetranscribe) {
      toast.error(i18n.t('import:errors.betaFeatureDisabled'), {
        description: i18n.t('import:descriptions.enableBetaImportFeature')
      });
      return;
    }

    // The backend is authoritative here because this layout sits outside the
    // recording context. Do not open an import surface during an active session.
    if (await invoke<boolean>('is_recording')) {
      toast.error(i18n.t('import:errors.importFailed'), {
        description: i18n.t('import:errors.recordingInProgress'),
      });
      return;
    }

    // Find the first audio file
    const audioFile = paths.find(p => {
      const ext = p.split('.').pop()?.toLowerCase();
      return !!ext && isAudioExtension(ext);
    });

    if (audioFile) {
      console.log('[Layout] Audio file dropped:', audioFile);
      setImportFilePath(audioFile);
      setShowImportDialog(true);
    } else if (paths.length > 0) {
      toast.error(i18n.t('import:errors.dropAudioFileRequired'), {
        description: i18n.t('import:descriptions.supportedFormats', { formats: getAudioFormatsDisplayList() })
      });
    }
  }, []);

  // Listen for drag-drop events
  useEffect(() => {
    if (showOnboarding) return; // Don't handle drops during onboarding

    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Drag enter/over - show overlay only if beta feature is enabled
      const unlistenDragEnter = await listen<{ paths: string[] }>('tauri://drag-enter', (event) => {
        if (shouldDelegateDropToTemplateImport(window.location.pathname, event.payload.paths)) {
          setShowDropOverlay(false);
          return;
        }
        if (loadBetaFeatures().importAndRetranscribe) {
          setShowDropOverlay(true);
        }
      });
      if (cleanedUpRef.current) {
        unlistenDragEnter();
        return;
      }
      unlisteners.push(unlistenDragEnter);

      // Drag leave - hide overlay
      const unlistenDragLeave = await listen('tauri://drag-leave', () => {
        setShowDropOverlay(false);
      });
      if (cleanedUpRef.current) {
        unlistenDragLeave();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDragLeave);

      // Drop - process files
      const unlistenDrop = await listen<{ paths: string[] }>('tauri://drag-drop', (event) => {
        setShowDropOverlay(false);
        handleFileDrop(event.payload.paths);
      });
      if (cleanedUpRef.current) {
        unlistenDrop();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDrop);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [showOnboarding, handleFileDrop]);

  // Handle import dialog close
  const handleImportDialogClose = useCallback((open: boolean) => {
    setShowImportDialog(open);
    if (!open) {
      setImportFilePath(null);
    }
  }, []);

  // Handler for ImportDialogProvider - opens import dialog from any child component
  const handleOpenImportDialog = useCallback((filePath?: string | null) => {
    setImportFilePath(filePath ?? null);
    setShowImportDialog(true);
  }, []);

  const handleOnboardingComplete = () => {
    console.log('[Layout] Onboarding completed, reloading app')
    setShowOnboarding(false)
    // Optionally reload the window to ensure all state is fresh
    window.location.reload()
  }

  return (
    <html lang="en" dir="ltr" suppressHydrationWarning>
      <head>
        <title>{DOCUMENT_TITLE}</title>
        <script dangerouslySetInnerHTML={{ __html: UI_LOCALE_BOOTSTRAP_SCRIPT }} />
      </head>
      <body className={`${sourceSans3.variable} font-sans antialiased`}>
        <MeetilyI18nProvider>
          <AnalyticsProvider>
            <RecordingStateProvider>
              <TranscriptProvider>
                <ConfigProvider>
                  <OllamaDownloadProvider>
                    <OnboardingProvider>
                      <UpdateCheckProvider>
                        <SidebarProvider>
                          <TooltipProvider>
                            <RecordingPostProcessingProvider>
                              <ImportDialogProvider onOpen={handleOpenImportDialog}>
                                {/* The onboarding flow already has full download cards. Keep the
                                    compact global toast for the main app so it cannot obscure setup. */}
                                {onboardingStatusChecked && !showOnboarding && <DownloadProgressToastProvider />}

                                {/* Show onboarding or main app */}
                                {!onboardingStatusChecked ? (
                                  <LocalizedSetupLoading />
                                ) : showOnboarding ? (
                                  <OnboardingFlow onComplete={handleOnboardingComplete} />
                                ) : (
                                  <div className="flex">
                                    <Sidebar />
                                    <MainContent>{children}</MainContent>
                                  </div>
                                )}
                                {/* Import audio overlay and dialog */}
                                <ImportDropOverlay visible={showDropOverlay} />
                                <ConditionalImportDialog
                                  showImportDialog={showImportDialog}
                                  handleImportDialogClose={handleImportDialogClose}
                                  importFilePath={importFilePath}
                                />
                              </ImportDialogProvider>
                            </RecordingPostProcessingProvider>
                          </TooltipProvider>
                        </SidebarProvider>
                      </UpdateCheckProvider>
                    </OnboardingProvider>

                  </OllamaDownloadProvider>
                </ConfigProvider>
              </TranscriptProvider>
            </RecordingStateProvider>
          </AnalyticsProvider>

          <LocalizedToaster />
        </MeetilyI18nProvider>
      </body>
    </html>
  )
}
