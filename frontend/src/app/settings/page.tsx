'use client';

import React, { useState, useLayoutEffect, useRef } from 'react';
import { ArrowLeft, Settings2, Mic, Database as DatabaseIcon, SparkleIcon } from 'lucide-react';
import { useRouter } from 'next/navigation';
import { motion } from 'framer-motion';
import { TranscriptSettings } from '@/components/TranscriptSettings';
import { RecordingSettings } from '@/components/RecordingSettings';
import { PreferenceSettings } from '@/components/PreferenceSettings';
import { SummaryModelSettings } from '@/components/SummaryModelSettings';
import { useConfig } from '@/contexts/ConfigContext';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs';
import { useTranslation } from 'react-i18next';

// Tabs configuration (constant)
const TABS = [
  { value: 'general', labelKey: 'tabs.general', icon: Settings2 },
  { value: 'recording', labelKey: 'tabs.recordings', icon: Mic },
  { value: 'Transcriptionmodels', labelKey: 'tabs.transcription', icon: DatabaseIcon },
  { value: 'summaryModels', labelKey: 'tabs.summary', icon: SparkleIcon }
] as const;

export default function SettingsPage() {
  const { t } = useTranslation('settings');
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();

  // Animation state for tabs
  const [activeTab, setActiveTab] = useState(() => {
    if (typeof window === 'undefined') return 'general';
    const searchParams = new URLSearchParams(window.location.search);
    const requestedTab = searchParams.get('tab');
    return TABS.some(tab => tab.value === requestedTab) ? requestedTab! : 'general';
  });
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [underlineStyle, setUnderlineStyle] = useState({ left: 0, width: 0 });

  // Update underline position when active tab changes
  useLayoutEffect(() => {
    const activeIndex = TABS.findIndex(tab => tab.value === activeTab);
    const activeTabElement = tabRefs.current[activeIndex];

    if (activeTabElement) {
      const { offsetLeft, offsetWidth } = activeTabElement;
      setUnderlineStyle({ left: offsetLeft, width: offsetWidth });
    }
  }, [activeTab]);

  return (
    <div className="flex h-screen min-w-0 flex-col overflow-x-hidden bg-gray-50">
      {/* Fixed Header */}
      <div className="sticky top-0 z-10 bg-gray-50 border-b border-gray-200">
        <div className="max-w-6xl mx-auto px-8 py-6">
          <div className="flex items-center gap-4">
            <button
              onClick={() => router.back()}
              aria-label={t('page.back')}
              className="flex items-center gap-2 text-gray-600 hover:text-gray-900 transition-colors"
            >
              <ArrowLeft className="w-5 h-5" />
              <span>{t('page.back')}</span>
            </button>
            <h1 className="text-3xl font-bold">{t('page.title')}</h1>
          </div>
        </div>
      </div>

      {/* Scrollable Content */}
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto min-w-0 max-w-6xl p-8 pt-6">
          {/* Tabs */}
          <Tabs value={activeTab} onValueChange={setActiveTab} className="min-w-0">
            <TabsList className="relative flex h-auto w-full max-w-full justify-start overflow-x-auto rounded-none border-b border-gray-200 bg-transparent p-0">
              {TABS.map((tab, index) => {
                const Icon = tab.icon;
                return (
                  <TabsTrigger
                    key={tab.value}
                    value={tab.value}
                    ref={el => { tabRefs.current[index] = el }}
                    className="relative z-10 flex shrink-0 items-center gap-2 rounded-none border-0 bg-transparent px-3 py-4 text-gray-600 hover:text-gray-900 data-[state=active]:bg-transparent data-[state=active]:text-blue-700 data-[state=active]:shadow-none sm:px-6"
                  >
                    <Icon className="w-4 h-4" />
                    {t(tab.labelKey)}
                  </TabsTrigger>
                );
              })}

              <motion.div
                className="absolute bottom-0 z-20 h-0.5 bg-blue-600"
                layoutId="underline"
                style={{ left: underlineStyle.left, width: underlineStyle.width }}
                transition={{ type: 'spring', stiffness: 400, damping: 40 }}
              />
            </TabsList>

            <TabsContent value="general">
              <PreferenceSettings />
            </TabsContent>
            <TabsContent value="recording">
              <RecordingSettings />
            </TabsContent>
            <TabsContent value="Transcriptionmodels">
              <div className="space-y-6">
                <TranscriptSettings
                  transcriptModelConfig={transcriptModelConfig}
                  setTranscriptModelConfig={setTranscriptModelConfig}
                />
              </div>
            </TabsContent>
            <TabsContent value="summaryModels">
              <SummaryModelSettings />
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  );
};
