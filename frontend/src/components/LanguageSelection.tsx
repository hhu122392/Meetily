import React, { useState } from 'react';
import { Globe } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import { LANGUAGES } from '@/constants/languages';
import { formatLanguageName } from '@/i18n/formatters';
import type { SupportedUiLocale } from '@/i18n/types';

interface LanguageSelectionProps {
  selectedLanguage: string;
  onLanguageChange: (language: string) => void | Promise<void>;
  disabled?: boolean;
  provider?: 'localWhisper' | 'parakeet' | 'sensevoice' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai';
}

export function LanguageSelection({
  selectedLanguage,
  onLanguageChange,
  disabled = false,
  provider = 'localWhisper'
}: LanguageSelectionProps) {
  const [saving, setSaving] = useState(false);
  const { t, i18n } = useTranslation('transcription');
  const uiLocale: SupportedUiLocale = i18n.resolvedLanguage === 'zh-CN' ? 'zh-CN' : 'en';

  const languageName = (code: string, fallback: string) => {
    if (code === 'auto') return t('labels.autoDetectOriginalLanguage');
    if (code === 'auto-translate') return t('labels.autoDetectTranslateToEnglish');
    return formatLanguageName(code, uiLocale) || fallback;
  };

  // Language is the user's intent, not the current engine's capabilities.
  const isParakeet = provider === 'parakeet';
  const availableLanguages = LANGUAGES;
  const compatibleSelectedLanguage = selectedLanguage;

  const handleLanguageChange = async (languageCode: string) => {
    setSaving(true);
    try {
      // Do not report success until the Rust transcription worker has accepted
      // the new language mode.
      await onLanguageChange(languageCode);
      console.log('Language preference saved:', languageCode);

      // Track language selection analytics
      const selectedLang = LANGUAGES.find(lang => lang.code === languageCode);
      await Analytics.track('language_selected', {
        language_code: languageCode,
        language_name: selectedLang?.name || 'Unknown',
        is_auto_detect: (languageCode === 'auto').toString(),
        is_auto_translate: (languageCode === 'auto-translate').toString()
      });

      // Show success toast
      const languageFallbackName = selectedLang?.name || languageCode;
      toast.success(t('messages.languagePreferenceSaved'), {
        description: t('descriptions.transcriptionLanguageSetToValue', {
          languageName: languageName(languageCode, languageFallbackName),
        })
      });
    } catch (error) {
      console.error('Failed to save language preference:', error);
      toast.error(t('errors.failedToSaveLanguagePreference'), {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setSaving(false);
    }
  };

  // Find the selected language name for display
  const selectedLanguageName = LANGUAGES.find(
    lang => lang.code === compatibleSelectedLanguage
  );
  const selectedLanguageDisplayName = languageName(
    compatibleSelectedLanguage,
    selectedLanguageName?.name || compatibleSelectedLanguage,
  );

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Globe className="h-4 w-4 text-gray-600" />
          <h4 className="text-sm font-medium text-gray-900">{t('labels.transcriptionLanguage')}</h4>
        </div>
      </div>

      <div className="space-y-2">
        <select
          value={compatibleSelectedLanguage}
          onChange={(e) => handleLanguageChange(e.target.value)}
          disabled={disabled || saving}
          className="w-full px-3 py-2 text-sm bg-white border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500 disabled:bg-gray-50 disabled:text-gray-500"
        >
          {availableLanguages.map((language) => (
            <option key={language.code} value={language.code}>
              {languageName(language.code, language.name)}
              {language.code !== 'auto' && language.code !== 'auto-translate' && ` (${language.code})`}
            </option>
          ))}
        </select>

        {/* Parakeet language limitation warning */}
        {isParakeet && (
          <div className="p-2 bg-amber-50 border border-amber-200 rounded text-amber-800">
            <p className="font-medium">{t('labels.parakeetLanguageSupport')}</p>
            <p className="mt-1 text-xs">{t('descriptions.parakeetTdtV3LanguageLimitations')}</p>
          </div>
        )}

        {/* Info text */}
        <div className="text-xs space-y-2 pt-2">
          <p className="rounded border border-slate-200 bg-slate-50 p-2 text-slate-700">
            {t('descriptions.transcriptionLanguageIsSourceNotTranslationTarget')}
          </p>
          <p className="text-gray-600">
            <strong>{t('labels.current')}:</strong> {selectedLanguageDisplayName}
          </p>
          {compatibleSelectedLanguage === 'auto' && !isParakeet && (
            <div className="p-2 bg-yellow-50 border border-yellow-200 rounded text-yellow-800">
              <p className="font-medium">{t('labels.autoDetectMayProduceIncorrectResults')}</p>
              <p className="mt-1">{t('labels.forBestAccuracySelectYourSpecificLanguageEGEnglish')}</p>
            </div>
          )}
          {compatibleSelectedLanguage === 'auto-translate' && (
            <div className="p-2 bg-blue-50 border border-blue-200 rounded text-blue-800">
              <p className="font-medium">{t('labels.translationModeActive')}</p>
              <p className="mt-1">{t('descriptions.allAudioWillBeAutomaticallyTranslatedToEnglishBestFor')}</p>
            </div>
          )}
          {compatibleSelectedLanguage !== 'auto' && compatibleSelectedLanguage !== 'auto-translate' && (
            <p className="text-gray-600">
              {t('labels.transcriptionWillBeOptimizedFor')} <strong>{selectedLanguageDisplayName}</strong>
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
