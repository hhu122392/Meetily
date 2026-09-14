import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import Analytics from '@/lib/analytics';
import {
  normalizeTemplateApiError,
  templateService,
} from '@/services/templateService';
import type {
  MeetingTemplatePreferenceResponse,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';

export function useTemplates(meetingId: string) {
  const { t, i18n } = useTranslation('summary');
  const displayContentLocale = i18n.resolvedLanguage ?? i18n.language;
  const [availableTemplates, setAvailableTemplates] = useState<TemplateListItem[]>([]);
  const [preferenceResponse, setPreferenceResponse] = useState<MeetingTemplatePreferenceResponse | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<TemplateApiError | null>(null);
  const [reloadVersion, setReloadVersion] = useState(0);
  const activeMeetingRef = useRef(meetingId);
  const loadVersionRef = useRef(0);
  const saveVersionRef = useRef(0);
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    activeMeetingRef.current = meetingId;
    const loadVersion = ++loadVersionRef.current;
    ++saveVersionRef.current;
    setIsLoading(true);
    setIsSaving(false);
    setError(null);
    setPreferenceResponse(null);

    void Promise.all([
      templateService.list({
        origin: 'all',
        includeInvalid: false,
        contentLocale: displayContentLocale,
      }),
      templateService.getMeetingPreference(meetingId),
    ])
      .then(([listed, preference]) => {
        if (loadVersion !== loadVersionRef.current || activeMeetingRef.current !== meetingId) return;
        setAvailableTemplates(listed.templates.filter((template) => template.valid));
        setPreferenceResponse(preference);
      })
      .catch((caught) => {
        if (loadVersion !== loadVersionRef.current || activeMeetingRef.current !== meetingId) return;
        setError(normalizeTemplateApiError(caught));
        setAvailableTemplates([]);
      })
      .finally(() => {
        if (loadVersion === loadVersionRef.current && activeMeetingRef.current === meetingId) {
          setIsLoading(false);
        }
      });
  }, [displayContentLocale, meetingId, reloadVersion]);

  const savePreference = useCallback(async (
    mode: 'inherit' | 'meeting_override',
    templateId: string | null,
    templateName?: string,
  ) => {
    const requestMeetingId = meetingId;
    const saveVersion = ++saveVersionRef.current;
    const previous = preferenceResponse;
    setIsSaving(true);
    setError(null);

    const operation = saveQueueRef.current
      .catch(() => undefined)
      .then(() => templateService.saveMeetingPreference({
        meetingId: requestMeetingId,
        preference: { mode, templateId },
      }));
    // Serialize writes so that the last user intent is also the last value
    // persisted by the backend, not only the last response rendered by React.
    saveQueueRef.current = operation.then(() => undefined, () => undefined);

    try {
      const saved = await operation;
      if (saveVersion !== saveVersionRef.current || activeMeetingRef.current !== requestMeetingId) return;
      setPreferenceResponse(saved);
      toast.success(t('templatePreference.saved'), {
        description: mode === 'inherit'
          ? t('templatePreference.usingGlobalDefault', { templateName: saved.resolved.name })
          : t('templatePreference.usingTemplate', { templateName: templateName ?? saved.resolved.name }),
      });
      Analytics.trackFeatureUsed('meeting_template_selected');
    } catch (caught) {
      if (saveVersion !== saveVersionRef.current || activeMeetingRef.current !== requestMeetingId) return;
      const normalized = normalizeTemplateApiError(caught);
      setPreferenceResponse(previous);
      setError(normalized);
      toast.error(t('templatePreference.saveFailed'), {
        description: t('templatePreference.retryDescription'),
      });
    } finally {
      if (saveVersion === saveVersionRef.current && activeMeetingRef.current === requestMeetingId) {
        setIsSaving(false);
      }
    }
  }, [meetingId, preferenceResponse, t]);

  const handleTemplateSelection = useCallback((templateId: string, templateName: string) => {
    void savePreference('meeting_override', templateId, templateName);
  }, [savePreference]);

  const handleUseGlobalDefault = useCallback(() => {
    void savePreference('inherit', null);
  }, [savePreference]);

  const handleRetry = useCallback(() => {
    setReloadVersion((version) => version + 1);
  }, []);

  const selectedTemplate = preferenceResponse?.resolved.templateId ?? 'standard_meeting';
  const selectedTemplateName = availableTemplates.find((template) => template.id === selectedTemplate)?.name
    ?? preferenceResponse?.resolved.name
    ?? selectedTemplate;
  const selectedListItem = useMemo(
    () => availableTemplates.find((template) => template.id === selectedTemplate) ?? null,
    [availableTemplates, selectedTemplate],
  );

  return {
    availableTemplates,
    selectedTemplate,
    selectedTemplateName,
    selectedListItem,
    preference: preferenceResponse?.preference ?? null,
    storage: preferenceResponse?.storage ?? null,
    resolutionSource: preferenceResponse?.resolved.source ?? null,
    issue: preferenceResponse?.issue ?? null,
    isLoading,
    isSaving,
    error,
    handleTemplateSelection,
    handleUseGlobalDefault,
    handleRetry,
  };
}
