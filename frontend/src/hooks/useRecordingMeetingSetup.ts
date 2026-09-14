'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import {
  meetingContextProfileFromExtensions,
  meetingContextProfileSha256,
  normalizeMeetingContextProfile,
  validateRecordingMeetingContextDraft,
} from '@/lib/meeting-context';
import { templateService } from '@/services/templateService';
import type {
  MeetingContextProfile,
  PreparedRecordingMetadata,
  RecordingMeetingContextDraft,
  TemplateDetails,
  TemplateListItem,
} from '@/types/summary-template';

interface LoadedMeetingSetup {
  details: TemplateDetails;
  profile: MeetingContextProfile | null;
  draft: RecordingMeetingContextDraft | null;
}

export interface RecordingMeetingSetupState {
  templates: TemplateListItem[];
  details: TemplateDetails | null;
  profile: MeetingContextProfile | null;
  draft: RecordingMeetingContextDraft | null;
  isCustomized: boolean;
  isLoading: boolean;
  error: string | null;
  selectTemplate: (templateId: string) => Promise<void>;
  updateDraft: (updater: (draft: RecordingMeetingContextDraft) => RecordingMeetingContextDraft) => void;
  resetAdjustments: () => Promise<void>;
  prepareRecordingMetadata: () => Promise<PreparedRecordingMetadata>;
}

async function loadSetup(templateId: string): Promise<LoadedMeetingSetup> {
  const details = await templateService.get({ templateId });
  const parsed = meetingContextProfileFromExtensions(details.template.extensions);
  const profile = parsed ? normalizeMeetingContextProfile(parsed) : null;
  const draft = profile ? {
    expectedProfileSha256: await meetingContextProfileSha256(profile),
    attendance: profile.people
      .filter((person) => person.enabled)
      // 模板里固定的名单默认按"出席"预填：实际开会时多数人就是在场，
      // 用户只需要把没来的人改成"缺席"（几十人时按"待确认"逐个点太费劲）。
      // "待确认"仍然保留：它只是姓名候选、不算摘要事实，需要时可以一键批量切回。
      .map((person) => ({ personId: person.person_id, attendance: 'attending' as const })),
    hostPersonId: null,
    guests: [],
    additionalTerms: [],
  } : null;
  return { details, profile, draft };
}

export function useRecordingMeetingSetup(): RecordingMeetingSetupState {
  const { t } = useTranslation('templates');
  const [templates, setTemplates] = useState<TemplateListItem[]>([]);
  const [loaded, setLoaded] = useState<LoadedMeetingSetup | null>(null);
  const [draft, setDraft] = useState<RecordingMeetingContextDraft | null>(null);
  const [isCustomized, setIsCustomized] = useState(false);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const initialLoadRef = useRef<Promise<LoadedMeetingSetup | null> | null>(null);

  const applyLoaded = useCallback((next: LoadedMeetingSetup) => {
    setLoaded(next);
    setDraft(next.draft);
    setIsCustomized(false);
    setError(null);
  }, []);

  const loadInitial = useCallback((): Promise<LoadedMeetingSetup | null> => {
    if (initialLoadRef.current) return initialLoadRef.current;
    initialLoadRef.current = (async () => {
      try {
        const [list, preference] = await Promise.all([
          templateService.list(),
          templateService.getDefault(),
        ]);
        setTemplates(list.templates.filter((template) => template.valid));
        const next = await loadSetup(preference.resolvedTemplateId);
        applyLoaded(next);
        return next;
      } catch (loadError) {
        console.error('Failed to prepare recording meeting context:', loadError);
        setError('RECORDING_MEETING_SETUP_UNAVAILABLE');
        return null;
      } finally {
        setIsLoading(false);
      }
    })();
    return initialLoadRef.current;
  }, [applyLoaded]);

  useEffect(() => {
    void loadInitial();
  }, [loadInitial]);

  const selectTemplate = useCallback(async (templateId: string) => {
    setIsLoading(true);
    try {
      const next = await loadSetup(templateId);
      applyLoaded(next);
    } catch (loadError) {
      console.error('Failed to load selected recording template:', loadError);
      setError('RECORDING_TEMPLATE_LOAD_FAILED');
      throw loadError;
    } finally {
      setIsLoading(false);
    }
  }, [applyLoaded]);

  const updateDraft = useCallback((updater: (current: RecordingMeetingContextDraft) => RecordingMeetingContextDraft) => {
    setDraft((current) => current ? updater(current) : current);
    setIsCustomized(true);
  }, []);

  const resetAdjustments = useCallback(async () => {
    if (!loaded) return;
    const next = await loadSetup(loaded.details.template.id);
    applyLoaded(next);
  }, [applyLoaded, loaded]);

  const prepareRecordingMetadata = useCallback(async (): Promise<PreparedRecordingMetadata> => {
    const current = loaded ?? await loadInitial();
    if (!current) {
      toast.warning(t('recordingSetup.warningNoContext'));
      return { templateSelection: null, meetingContextDraft: null };
    }
    if (isCustomized && current.profile && draft) {
      const issues = validateRecordingMeetingContextDraft(current.profile, draft);
      if (issues.length > 0) {
        console.error('Recording meeting context draft is invalid:', issues);
        toast.error(t('recordingSetup.invalidDraft'));
        throw new Error('RECORDING_MEETING_CONTEXT_INVALID');
      }
    }
    return {
      templateSelection: {
        templateId: current.details.template.id,
        templateVersion: current.details.template.version,
        templateFileSha256: current.details.fileSha256,
      },
      meetingContextDraft: isCustomized ? draft : null,
    };
  }, [draft, isCustomized, loadInitial, loaded, t]);

  return {
    templates,
    details: loaded?.details ?? null,
    profile: loaded?.profile ?? null,
    draft,
    isCustomized,
    isLoading,
    error,
    selectTemplate,
    updateDraft,
    resetAdjustments,
    prepareRecordingMetadata,
  };
}
