"use client";

import { TemplateLibrary } from '@/components/templates/TemplateLibrary';
import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import {
  Dialog,
  DialogContent,
  DialogTrigger,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Sparkles, SlidersHorizontal, Loader2, FileText, Check, Square } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { useState, useEffect, ReactNode } from 'react';
import { t, useUiLanguage } from "@/i18n";

interface SummaryGeneratorButtonGroupProps {
  layout?: 'toolbar' | 'primary';
  languageSlot?: ReactNode;
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  availableTemplates: Array<{ id: string, name: string, description: string }>;
  selectedTemplate: string;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  hasTranscripts?: boolean;
  hasSummary?: boolean;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
}

export function SummaryGeneratorButtonGroup({
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryStatus,
  availableTemplates,
  selectedTemplate,
  onTemplateSelect,
  hasTranscripts = true,
  hasSummary = false,
  isModelConfigLoading = false,
  onOpenModelSettings,
  languageSlot,
  layout = 'primary'
}: SummaryGeneratorButtonGroupProps) {
  useUiLanguage();
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [templateLibraryOpen, setTemplateLibraryOpen] = useState(false);

  // Expose the function to open the modal via callback registration
  useEffect(() => {
    if (layout === 'toolbar' && onOpenModelSettings) {
      // Register our open dialog function with the parent by calling the callback
      // This allows the parent to store a reference to this function
      const openDialog = () => {
        console.log('📱 Opening model settings dialog via callback');
        setSettingsDialogOpen(true);
      };

      // Call the parent's callback with our open function
      // Note: This assumes onOpenModelSettings accepts a function parameter
      // We'll need to adjust the signature
      onOpenModelSettings(openDialog);
    }
  }, [layout, onOpenModelSettings]);

  if (!hasTranscripts) {
    return null;
  }

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  return (
    <div className="flex items-center gap-1" role="group" aria-label={layout === 'toolbar' ? '纪要设置' : '纪要生成'}>
      {layout === 'primary' && <>
      {/* Generate Summary or Stop button */}
      {isGenerating ? (
        <Button
          variant="outline"
          size="sm"
          className="bg-gradient-to-r from-red-50 to-orange-50 hover:from-red-100 hover:to-orange-100 border-red-200 h-8 rounded-full px-3 gap-2"
          onClick={() => {
            Analytics.trackButtonClick('stop_summary_generation', 'meeting_details');
            onStopGeneration();
          }}
          title={t("Stop summary generation")}
        >
          <Square size={18} fill="currentColor" />
          <span className="hidden @[24rem]:inline">{t("Stop")}</span>
        </Button>
      ) : (
        <Button
          variant="outline"
          size="sm"
          className="bg-gradient-to-r from-blue-50 to-purple-50 hover:from-blue-100 hover:to-purple-100 border-blue-200 h-8 rounded-full px-3 gap-2"
          onClick={() => {
            Analytics.trackButtonClick('generate_summary', 'meeting_details');
            void onGenerateSummary(customPrompt);
          }}
          disabled={isModelConfigLoading}
          title={
            isModelConfigLoading
              ? t("Loading model configuration...")
              : hasSummary ? t("Regenerate AI Summary") : t("Generate AI Summary")
          }
        >
          {isModelConfigLoading ? (
            <>
              <Loader2 className="animate-spin" size={18} />
              <span className="hidden @[24rem]:inline">{t("Processing...")}</span>
            </>
          ) : (
            <>
              <Sparkles size={18} />
              <span className="hidden @[24rem]:inline">{hasSummary ? t("Regenerate Summary") : t("Generate Summary")}</span>
            </>
          )}
        </Button>
      )}

      {languageSlot}
      </>}
      {layout === 'toolbar' && <>

      {/* Settings button */}
      <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
        <DialogTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
            aria-label={t("Summary Settings")}
            title={t("Summary Settings")}
          >
            <SlidersHorizontal className="h-4 w-4" />
          </Button>
        </DialogTrigger>
        <DialogContent
          aria-describedby={undefined}
        >
          <VisuallyHidden>
            <DialogTitle>{t("Model Settings")}</DialogTitle>
          </VisuallyHidden>
          <ModelSettingsModal
            onSave={async (config) => {
              await onSaveModelConfig(config);
              setSettingsDialogOpen(false);
            }}
            modelConfig={modelConfig}
            setModelConfig={setModelConfig}
            skipInitialFetch={true}
            layout="dialog"
          />
        </DialogContent>
      </Dialog>

      {/* Template selector dropdown */}
      {(
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
              title={`纪要模板：${availableTemplates.find(template => template.id === selectedTemplate)?.name || '原模板不可用，请重新选择'}`}
              aria-label={`纪要模板：${availableTemplates.find(template => template.id === selectedTemplate)?.name || '原模板不可用，请重新选择'}`}
            >
              <FileText />
              <span className="sr-only">{availableTemplates.find(template => template.id === selectedTemplate)?.name || t("Template")}</span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="max-h-[70vh] overflow-y-auto">
            <DropdownMenuItem onSelect={() => setTemplateLibraryOpen(true)}>管理纪要模板…</DropdownMenuItem>
            {availableTemplates.map((template) => (
              <DropdownMenuItem
                key={template.id}
                onClick={() => onTemplateSelect(template.id, template.name)}
                title={template.description}
                className="flex items-center justify-between gap-2"
              >
                <span>{template.name}</span>
                {selectedTemplate === template.id && (
                  <Check className="h-4 w-4 text-green-600" />
                )}
              </DropdownMenuItem>
            ))}

          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <Dialog open={templateLibraryOpen} onOpenChange={setTemplateLibraryOpen}>
        <DialogContent className="w-[calc(100vw-2rem)] max-w-6xl max-h-[90vh] overflow-y-auto rounded-2xl">
          <DialogTitle>会议纪要模板管理</DialogTitle>
          <TemplateLibrary />
        </DialogContent>
      </Dialog>
      </>}
    </div>
  );
}
