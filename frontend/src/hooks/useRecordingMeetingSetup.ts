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
  const loadSequenceRef = useRef(0);
  const loadStateRef = useRef<{ status: 'loading' | 'ready' | 'failed'; loaded: LoadedMeetingSetup | null }>({ status: 'loading', loaded: null });

  const applyLoaded = useCallback((next: LoadedMeetingSetup) => {
    loadStateRef.current = { status: 'ready', loaded: next };
    setLoaded(next);
    setDraft(next.draft);
    setIsCustomized(false);
    setError(null);
  }, []);

  const loadInitial = useCallback((): Promise<LoadedMeetingSetup | null> => {
    if (initialLoadRef.current) return initialLoadRef.current;
    const sequence = ++loadSequenceRef.current;
    loadStateRef.current.status = 'loading';
    initialLoadRef.current = (async () => {
      try {
        const [list, preference] = await Promise.all([
          templateService.list(),
          templateService.getDefault(),
        ]);
        setTemplates(list.templates.filter((template) => template.valid));
        const next = await loadSetup(preference.resolvedTemplateId);
        if (sequence === loadSequenceRef.current) {
          applyLoaded(next);
        }
        return next;
      } catch (loadError) {
        console.error('Failed to prepare recording meeting context:', loadError);
        if (sequence === loadSequenceRef.current) {
          setError('RECORDING_MEETING_SETUP_UNAVAILABLE');
          loadStateRef.current.status = 'failed';
        }
        return null;
      } finally {
        if (sequence === loadSequenceRef.current) setIsLoading(false);
      }
    })();
    return initialLoadRef.current;
  }, [applyLoaded]);

  useEffect(() => {
    void loadInitial();
  }, [loadInitial]);

  const selectTemplate = useCallback(async (templateId: string) => {
    const sequence = ++loadSequenceRef.current;
    loadStateRef.current.status = 'loading';
    setIsLoading(true);
    try {
      const next = await loadSetup(templateId);
      if (sequence === loadSequenceRef.current) {
        applyLoaded(next);
      }
    } catch (loadError) {
      console.error('Failed to load selected recording template:', loadError);
      if (sequence === loadSequenceRef.current) {
        setError('RECORDING_TEMPLATE_LOAD_FAILED');
        loadStateRef.current.status = 'failed';
      }
    } finally {
      if (sequence === loadSequenceRef.current) setIsLoading(false);
    }
  }, [applyLoaded]);

  const updateDraft = useCallback((updater: (current: RecordingMeetingContextDraft) => RecordingMeetingContextDraft) => {
    setDraft((current) => current ? updater(current) : current);
    setIsCustomized(true);
  }, []);

  const resetAdjustments = useCallback(async () => {
    if (!loaded) return;
    await selectTemplate(loaded.details.template.id);
  }, [loaded, selectTemplate]);

  const prepareRecordingMetadata = useCallback(async (): Promise<PreparedRecordingMetadata> => {
    const initial = loaded ? null : loadInitial();
    const sequence = loadSequenceRef.current;
    if (initial) await initial;
    const current = loadStateRef.current.loaded;
    if (sequence !== loadSequenceRef.current || loadStateRef.current.status === 'loading') {
      toast.error(t('recordingSetup.loading'));
      throw new Error('RECORDING_MEETING_SETUP_LOADING');
    }
    if (!current || loadStateRef.current.status === 'failed') {
      toast.error(t('recordingSetup.loadWarning'));
      throw new Error('RECORDING_MEETING_SETUP_UNAVAILABLE');
    }
    const effectiveDraft = loaded === current ? draft : current.draft;
    if (current.profile) {
      const issues = effectiveDraft ? validateRecordingMeetingContextDraft(current.profile, effectiveDraft) : [];
      if (!effectiveDraft || issues.length > 0
        || effectiveDraft.expectedProfileSha256 !== current.draft?.expectedProfileSha256) {
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
      meetingContextDraft: effectiveDraft,
    };
  }, [draft, loadInitial, loaded, t]);

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
