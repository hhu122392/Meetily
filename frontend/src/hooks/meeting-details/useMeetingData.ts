import { useState, useCallback, useRef, useEffect } from 'react';
import { Transcript, Summary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { CurrentMeeting, useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import type {
  SummaryFieldTrace,
  TranscriptEvidenceBinding,
} from '@/types/summary-source';

interface UseMeetingDataProps {
  meeting: any;
  summaryData: Summary | null;
}

interface SummaryFactValidationResponse {
  status: 'passed' | 'needs_review';
  warningCount: number;
  warnings: Array<{
    code: string;
    messageKey: string;
  }>;
  aliasesNormalized: boolean;
  meetingContextId?: string | null;
  meetingContextSha256?: string | null;
  summaryContextSha256?: string | null;
  fieldTraces?: SummaryFieldTrace[];
  sourceEvidence?: TranscriptEvidenceBinding | null;
}

interface SaveMeetingSummaryResponse {
  message: string;
  factValidation: SummaryFactValidationResponse;
  summary: Summary;
}

interface CurrentSummaryResponse {
  data?: Summary | null;
}

export function useMeetingData({ meeting, summaryData }: UseMeetingDataProps) {
  const { t } = useTranslation('meetings');
  // State
  // Use prop directly since summary generation fetches transcripts independently
  const transcripts = meeting.transcripts;
  const [meetingTitle, setMeetingTitle] = useState(meeting.title || '+ New Call');
  const [isEditingTitle, setIsEditingTitle] = useState(false);
  const [isTitleDirty, setIsTitleDirty] = useState(false);
  const [aiSummary, setAiSummary] = useState<Summary | null>(summaryData);
  const [isSaving, setIsSaving] = useState(false);
  const [, setIsSummaryDirty] = useState(false);
  const [, setError] = useState<string>('');

  // Ref for BlockNoteSummaryView
  const blockNoteSummaryRef = useRef<BlockNoteSummaryViewRef>(null);

  // Sidebar context
  const { setCurrentMeeting, setMeetings, meetings: sidebarMeetings } = useSidebar();

  // Sync aiSummary state when summaryData prop changes (fixes display of fetched summaries)
  useEffect(() => {
    console.log('[useMeetingData] Syncing summary data from prop:', summaryData ? 'present' : 'null');
    setAiSummary(summaryData);
  }, [summaryData]); // Only trigger when parent prop changes, not when aiSummary changes

  // P2-18：侧边栏改名后，详情页标题立即同步（原来要切走再切回才刷新）
  useEffect(() => {
    const handleMeetingTitleUpdated = (event: Event) => {
      const detail = (event as CustomEvent<{ meetingId?: string; title?: string }>).detail;
      if (!detail?.meetingId || detail.meetingId !== meeting.id || typeof detail.title !== 'string') {
        return;
      }
      setMeetingTitle(detail.title);
      setIsTitleDirty(false);
    };

    window.addEventListener('meeting-title-updated', handleMeetingTitleUpdated);
    return () => window.removeEventListener('meeting-title-updated', handleMeetingTitleUpdated);
  }, [meeting.id]);

  // Handlers
  const handleTitleChange = useCallback((newTitle: string) => {
    setMeetingTitle(newTitle);
    setIsTitleDirty(true);
  }, []);

  const handleSummaryChange = useCallback((newSummary: Summary) => {
    setAiSummary((currentSummary) => {
      if (!currentSummary) return newSummary;
      const current = currentSummary as Record<string, unknown>;
      const next = { ...(newSummary as Record<string, unknown>) };
      for (const key of ['template_snapshot', 'factValidation', 'summaryFreshness']) {
        if (!(key in next) && key in current) next[key] = current[key];
      }
      return next as Summary;
    });
  }, []);

  const handleSaveMeetingTitle = useCallback(async () => {
    try {
      await invokeTauri('api_save_meeting_title', {
        meetingId: meeting.id,
        title: meetingTitle,
      });

      console.log('Save meeting title success');
      setIsTitleDirty(false);

      // Update meetings with new title
      const updatedMeetings = sidebarMeetings.map((m: CurrentMeeting) =>
        m.id === meeting.id ? { id: m.id, title: meetingTitle } : m
      );
      setMeetings(updatedMeetings);
      setCurrentMeeting({ id: meeting.id, title: meetingTitle });
      return true;
    } catch (error) {
      console.error('Failed to save meeting title:', error);
      if (error instanceof Error) {
        setError(error.message);
      } else {
        setError('Failed to save meeting title: Unknown error');
      }
      return false;
    }
  }, [meeting.id, meetingTitle, sidebarMeetings, setMeetings, setCurrentMeeting]);

  const handleFinishTitleEditing = useCallback(async () => {
    setIsEditingTitle(false);
    if (!isTitleDirty) return true;

    const saved = await handleSaveMeetingTitle();
    if (!saved) {
      toast.error(t('errors.failedToSaveChanges'));
    }
    return saved;
  }, [handleSaveMeetingTitle, isTitleDirty, t]);

  const handleSaveSummary = useCallback(async (summary: Summary | { markdown?: string; summary_json?: any[] }) => {
    console.log('📄 handleSaveSummary called with:', {
      hasMarkdown: 'markdown' in summary,
      hasSummaryJson: 'summary_json' in summary,
      summaryKeys: Object.keys(summary)
    });

    try {
      let formattedSummary: any;

      // Check if it's the new BlockNote format
      if ('markdown' in summary || 'summary_json' in summary) {
        console.log('📄 Saving new format (markdown/blocknote)');
        formattedSummary = summary;
      } else {
        console.log('📄 Saving legacy format');
        formattedSummary = {
          MeetingName: meetingTitle,
          MeetingNotes: {
            sections: Object.entries(summary).map(([, section]) => ({
              title: section.title,
              blocks: section.blocks
            }))
          }
        };
      }

      const response = await invokeTauri<SaveMeetingSummaryResponse>('api_save_meeting_summary', {
        meetingId: meeting.id,
        summary: formattedSummary,
      });

      // The native response is the canonical saved summary. It contains the
      // freshly validated Markdown and intentionally omits independently
      // supplied BlockNote JSON so the UI cannot render content different from
      // what the native layer checked.
      setAiSummary(response.summary);

      console.log('✅ Save meeting summary success');
    } catch (error) {
      console.error('❌ Failed to save meeting summary:', error);
      if (error instanceof Error) {
        setError(error.message);
      } else {
        setError('Failed to save meeting summary: Unknown error');
      }
      throw error;
    }
  }, [meeting.id, meetingTitle]);

  const refreshSummaryFromCurrentEvidence = useCallback(async () => {
    try {
      const response = await invokeTauri<CurrentSummaryResponse>('api_get_summary', {
        meetingId: meeting.id,
      });
      if (response.data) {
        setAiSummary(response.data);
      }
    } catch (error) {
      console.error('Failed to refresh summary fact validation:', error);
      setAiSummary((currentSummary) => currentSummary ? ({
        ...currentSummary,
        factValidation: {
          status: 'needs_review',
          warningCount: 1,
          warnings: [{
            code: 'summary_validation_unavailable',
            messageKey: 'summary:factValidation.validationUnavailable',
          }],
          aliasesNormalized: false,
        },
      } as unknown as Summary) : currentSummary);
    }
  }, [meeting.id]);

  const saveAllChanges = useCallback(async () => {
    // P2-25：没有改动时不要报"更改已成功保存"（实际什么都没写）
    const hasTitleChanges = isTitleDirty;
    const hasSummaryChanges = Boolean(blockNoteSummaryRef.current?.isDirty);
    if (!hasTitleChanges && !hasSummaryChanges) {
      toast.info(t('messages.noChangesToSave'));
      return;
    }

    setIsSaving(true);
    try {
      // Save meeting title only if changed
      if (hasTitleChanges) {
        await handleSaveMeetingTitle();
      }

      // Save BlockNote editor changes if dirty
      if (hasSummaryChanges) {
        console.log('💾 Saving BlockNote editor changes...');
        await blockNoteSummaryRef.current?.saveSummary();
      }

      toast.success(t('messages.changesSavedSuccessfully'));
    } catch (error) {
      console.error('Failed to save changes:', error);
      toast.error(t('errors.failedToSaveChanges'));
      throw error;
    } finally {
      setIsSaving(false);
    }
  }, [isTitleDirty, handleSaveMeetingTitle, t]);

  return {
    // State
    transcripts,
    meetingTitle,
    isEditingTitle,
    isTitleDirty,
    aiSummary,
    isSaving,
    blockNoteSummaryRef,

    // Setters
    setMeetingTitle,
    setIsEditingTitle,
    setAiSummary,
    setIsSummaryDirty,

    // Handlers
    handleTitleChange,
    handleFinishTitleEditing,
    handleSummaryChange,
    handleSaveSummary,
    refreshSummaryFromCurrentEvidence,
    handleSaveMeetingTitle,
    saveAllChanges,
  };
}
