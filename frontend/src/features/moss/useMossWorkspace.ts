'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { mossReviewService, normalizeMossApiError, type MossReviewService } from './service';
import type {
  MossApiError,
  MossWorkspace,
  SaveMossSegmentOverrideRequest,
  SaveMossSpeakerBindingRequest,
  SetMossCorrectionStateRequest,
  UpdateMossCandidateSegmentRequest,
} from './types';
import { isMossRunActive, MossOperationGate } from './utils';

interface MossWorkspaceState {
  workspace: MossWorkspace | null;
  loading: boolean;
  error: MossApiError | null;
}

const RUNNING_POLL_INTERVAL_MS = 1500;
const CANCELLATION_POLL_INTERVAL_MS = 100;

export function useMossWorkspace({
  meetingId,
  enabled,
  active,
  service = mossReviewService,
}: {
  meetingId: string;
  enabled: boolean;
  active: boolean;
  service?: MossReviewService;
}) {
  const [state, setState] = useState<MossWorkspaceState>({
    workspace: null,
    loading: false,
    error: null,
  });
  const [pendingKeys, setPendingKeys] = useState<ReadonlySet<string>>(new Set());
  const requestSequence = useRef(0);
  const mounted = useRef(true);
  const gate = useRef(new MossOperationGate());
  const currentContext = useRef({ meetingId, enabled, active, service });
  currentContext.current = { meetingId, enabled, active, service };

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const applyWorkspace = useCallback((workspace: MossWorkspace) => {
    if (!mounted.current) return;
    setState({ workspace, loading: false, error: null });
  }, []);

  const requestWorkspace = useCallback(async (
    selectedRunId: string | null,
    showLoading = false,
  ) => {
    if (!enabled || !active) return null;
    const sequence = ++requestSequence.current;
    if (showLoading) {
      setState((current) => ({ ...current, loading: true, error: null }));
    }
    try {
      const workspace = await service.getWorkspace({
        meetingId,
        selectedRunId,
      });
      if (!mounted.current || sequence !== requestSequence.current) return null;
      applyWorkspace(workspace);
      return workspace;
    } catch (error) {
      if (!mounted.current || sequence !== requestSequence.current) return null;
      setState((current) => ({
        ...current,
        loading: false,
        error: normalizeMossApiError(error),
      }));
      return null;
    }
  }, [active, applyWorkspace, enabled, meetingId, service]);

  const refresh = useCallback(
    (showLoading = false) => requestWorkspace(state.workspace?.selectedRunId ?? null, showLoading),
    [requestWorkspace, state.workspace?.selectedRunId],
  );

  useEffect(() => {
    requestSequence.current += 1;
    setPendingKeys(new Set());
    if (!enabled || !active) {
      setState({ workspace: null, loading: false, error: null });
      return;
    }
    setState({ workspace: null, loading: true, error: null });
    void requestWorkspace(null, false);
  }, [active, enabled, requestWorkspace]);

  const hasRunningTask = useMemo(
    () => state.workspace?.runs.some(isMossRunActive) ?? false,
    [state.workspace?.runs],
  );
  const hasCancellationPending = useMemo(
    () => state.workspace?.runs.some((run) => run.state === 'cancel_requested') ?? false,
    [state.workspace?.runs],
  );

  useEffect(() => {
    if (!enabled || !active || !hasRunningTask) return;
    let disposed = false;
    let timer: number | null = null;
    const interval = hasCancellationPending
      ? CANCELLATION_POLL_INTERVAL_MS
      : RUNNING_POLL_INTERVAL_MS;
    const pollAndSchedule = async () => {
      await refresh(false);
      if (!disposed) scheduleNext();
    };
    const scheduleNext = () => {
      timer = window.setTimeout(() => {
        timer = null;
        void pollAndSchedule();
      }, interval);
    };

    // The cancellation mutation only proves that the helper received the
    // request. Keep the start gate locked, but immediately begin a fresh,
    // short polling cycle so the terminal response can unlock the UI within
    // the one-second feedback budget.
    if (hasCancellationPending) void pollAndSchedule();
    else scheduleNext();

    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [active, enabled, hasCancellationPending, hasRunningTask, refresh]);

  const runOperation = useCallback(async (
    key: string,
    operation: () => Promise<MossWorkspace>,
  ): Promise<MossWorkspace | null> => {
    const operationContext = { meetingId, service };
    const result = await gate.current.run(key, async () => {
      requestSequence.current += 1;
      if (mounted.current) {
        setPendingKeys((current) => new Set(current).add(key));
        setState((current) => ({ ...current, error: null }));
      }
      try {
        const workspace = await operation();
        const context = currentContext.current;
        if (
          !context.enabled
          || !context.active
          || context.meetingId !== operationContext.meetingId
          || context.service !== operationContext.service
        ) return null;
        // A poll may have started while this write was pending. The mutation
        // response is authoritative, so invalidate every older read first.
        requestSequence.current += 1;
        applyWorkspace(workspace);
        return workspace;
      } catch (error) {
        const context = currentContext.current;
        if (
          mounted.current
          && context.enabled
          && context.active
          && context.meetingId === operationContext.meetingId
          && context.service === operationContext.service
        ) {
          requestSequence.current += 1;
          setState((current) => ({ ...current, error: normalizeMossApiError(error) }));
        }
        return null;
      } finally {
        const context = currentContext.current;
        if (
          mounted.current
          && context.enabled
          && context.active
          && context.meetingId === operationContext.meetingId
          && context.service === operationContext.service
        ) {
          setPendingKeys((current) => {
            const next = new Set(current);
            next.delete(key);
            return next;
          });
        }
      }
    });
    return result ?? null;
  }, [applyWorkspace, meetingId, service]);

  const review = state.workspace?.review ?? null;
  const expectedCandidateRevision = review?.candidate.revision;
  const candidateIsEditable = Boolean(review && !review.candidate.isActive);

  return {
    ...state,
    review,
    pendingKeys,
    hasRunningTask,
    refresh: () => refresh(true),
    selectRun: (runId: string) => runOperation(
      'select-run',
      () => service.getWorkspace({ meetingId, selectedRunId: runId }),
    ),
    startRun: () => runOperation('start-run', () => service.startRun({ meetingId })),
    cancelRun: (runId: string) => runOperation(
      `cancel-run:${runId}`,
      () => service.cancelRun({ meetingId, runId }),
    ),
    saveSpeakerBinding: (request: Omit<SaveMossSpeakerBindingRequest, 'meetingId' | 'expectedCandidateRevision'>) => (
      expectedCandidateRevision === undefined || !candidateIsEditable
        ? Promise.resolve(null)
        : runOperation(
          `speaker-binding:${request.speakerLabel}`,
          () => service.saveSpeakerBinding({
            ...request,
            meetingId,
            expectedCandidateRevision,
          }),
        )
    ),
    saveSegmentOverride: (request: Omit<SaveMossSegmentOverrideRequest, 'meetingId' | 'expectedCandidateRevision'>) => (
      expectedCandidateRevision === undefined || !candidateIsEditable
        ? Promise.resolve(null)
        : runOperation(
          `segment-override:${request.segmentId}`,
          () => service.saveSegmentOverride({
            ...request,
            meetingId,
            expectedCandidateRevision,
          }),
        )
    ),
    setCorrectionState: (request: Omit<SetMossCorrectionStateRequest, 'meetingId' | 'expectedCandidateRevision'>) => (
      expectedCandidateRevision === undefined || !candidateIsEditable
        ? Promise.resolve(null)
        : runOperation(
          `correction:${request.correctionId}`,
          () => service.setCorrectionState({
            ...request,
            meetingId,
            expectedCandidateRevision,
          }),
        )
    ),
    updateCandidateSegment: (request: Omit<UpdateMossCandidateSegmentRequest, 'meetingId' | 'expectedCandidateRevision'>) => (
      expectedCandidateRevision === undefined || !candidateIsEditable
        ? Promise.resolve(null)
        : runOperation(
          `candidate-segment:${request.segmentId}`,
          () => service.updateCandidateSegment({
            ...request,
            meetingId,
            expectedCandidateRevision,
          }),
        )
    ),
    activateCandidate: () => review?.activation.canActivate
      ? runOperation('activate-candidate', () => service.activateCandidate({
        meetingId,
        runId: review.candidate.runId,
        expectedCandidateRevision: review.candidate.revision,
        expectedCurrentTranscriptSha256: review.current.sha256,
      }))
      : Promise.resolve(null),
    rollbackActivation: () => review?.activation.canRollback && review.activation.activeActivationId
      ? runOperation('rollback-activation', () => service.rollbackActivation({
        meetingId,
        activationId: review.activation.activeActivationId!,
        expectedCurrentTranscriptSha256: review.activation.currentTranscriptSha256,
      }))
      : Promise.resolve(null),
  };
}
