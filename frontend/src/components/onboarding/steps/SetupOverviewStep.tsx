import React, { useEffect, useState } from 'react';
import { HelpHint } from '@/components/ui/help-hint';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { useTranslation } from 'react-i18next';

export function SetupOverviewStep() {
  const { goNext } = useOnboarding();
  const { t } = useTranslation('onboarding');
  const [isMac, setIsMac] = useState(false);

  useEffect(() => {
    const checkPlatform = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  const steps = [
    {
      number: 1,
      type: 'transcription',
      title: t('actions.downloadTranscriptionEngine'),
    },
    {
      number: 2,
      type: 'summarization',
      title: t('actions.downloadSummarizationEngine'),
    },
  ];

  const handleContinue = () => {
    goNext();
  };

  return (
    <OnboardingContainer
      title={t('accessibility.setupOverview')}
      description={t('descriptions.meetilyRequiresThatYouDownloadTheTranscriptionAndSummarizationAI')}
      step={2}
      totalSteps={isMac ? 4 : 3}
    >
      <div className="flex flex-col items-center space-y-10">
        {/* Steps Card */}
        <div className="w-full max-w-md bg-white rounded-lg border border-gray-200 p-4">
          <div className="space-y-4">
            {steps.map((step) => {
              return (
                <div
                  key={step.number}
                  className={`flex items-start gap-4 p-1`}
                >
                  <div className="flex-1 ml-1">
                    <h3 className="font-medium text-gray-900 flex items-center gap-2">
                        {t('messages.stepWithTitle', {
                          step: step.number,
                          title: step.type === 'transcription' ? `${step.title} · SenseVoice` : step.title,
                        })}

                        {step.type === "summarization" && (
                            <HelpHint>{t('descriptions.youCanAlsoSelectExternalAIProvidersLikeOpenAIClaude')}</HelpHint>
                        )}
                        </h3>
                  </div>
                </div>
              );
            })}
          </div>
        </div>


        {/* CTA Section */}
        <div className="w-full max-w-xs space-y-4">
          <Button
            onClick={handleContinue}
            className="w-full h-11 bg-gray-900 hover:bg-gray-800 text-white"
          >
            {t('actions.letSGo')}
          </Button>
          <div className="text-center">
            <a
              href="https://github.com/Zackriya-Solutions/meeting-minutes"
              target="_blank"
              rel="noopener noreferrer"
              className="text-xs text-gray-600 hover:underline"
            >
              {t('actions.reportIssuesOnGitHub')}
            </a>
          </div>
        </div>
      </div>
    </OnboardingContainer>
  );
}
