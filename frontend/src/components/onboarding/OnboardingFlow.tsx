import { useOnboarding } from '@/contexts/OnboardingContext';
import {
  WelcomeStep,
  PermissionsStep,
  DownloadProgressStep,
  SetupOverviewStep,
} from './steps';
import { Loader2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { usePlatform } from '@/hooks/usePlatform';

interface OnboardingFlowProps {
  onComplete: () => void;
}

export function OnboardingFlow({ onComplete }: OnboardingFlowProps) {
  const { currentStep, isStatusLoaded } = useOnboarding();
  const { t } = useTranslation('onboarding');
  const isMac = usePlatform() === 'macos';

  // 4-Step Onboarding Flow (System-Recommended Models):
  // Step 1: Welcome - Introduce Meetily features
  // Step 2: Setup Overview - Database initialization + show recommended downloads
  // Step 3: Download Progress - Download Parakeet + Summary Model (auto-selected based on platform/RAM)
  // Step 4: Permissions - Request mic + system audio (macOS only)

  if (!isStatusLoaded) {
    return (
      <div
        className="fixed inset-0 z-50 flex items-center justify-center bg-gray-50"
        role="status"
        aria-live="polite"
      >
        <div className="flex items-center gap-3 text-gray-700">
          <Loader2 className="h-5 w-5 animate-spin" aria-hidden="true" />
          <span>{t('status.loadingSetup')}</span>
        </div>
      </div>
    );
  }

  return (
    <div className="onboarding-flow">
      {currentStep === 1 && <WelcomeStep />}
      {currentStep === 2 && <SetupOverviewStep />}
      {currentStep === 3 && <DownloadProgressStep />}
      {currentStep === 4 && isMac && <PermissionsStep />}
    </div>
  );
}
