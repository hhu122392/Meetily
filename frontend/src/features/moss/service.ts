import { invoke } from '@tauri-apps/api/core';
import type { ZodType } from 'zod';
import { mossSystemStatusSchema, mossWorkspaceSchema } from './schemas';
import type {
  ActivateMossCandidateRequest,
  MossApiError,
  MossApiErrorCode,
  MossRunRequest,
  MossSystemStatus,
  MossWorkspace,
  MossWorkspaceRequest,
  RollbackMossActivationRequest,
  SaveMossSegmentOverrideRequest,
  SaveMossSpeakerBindingRequest,
  SetMossCorrectionStateRequest,
  StartMossRunRequest,
  UpdateMossCandidateSegmentRequest,
} from './types';

export const MOSS_COMMANDS = {
  getSystemStatus: 'api_moss_get_system_status',
  getWorkspace: 'api_moss_get_workspace',
  startRun: 'api_moss_start_run',
  cancelRun: 'api_moss_cancel_run',
  saveSpeakerBinding: 'api_moss_save_speaker_binding',
  saveSegmentOverride: 'api_moss_save_segment_override',
  setCorrectionState: 'api_moss_set_correction_state',
  updateCandidateSegment: 'api_moss_update_candidate_segment',
  activateCandidate: 'api_moss_activate_candidate',
  rollbackActivation: 'api_moss_rollback_activation',
} as const;

const KNOWN_ERROR_CODES = new Set<MossApiErrorCode>([
  'MOSS_FRONTEND_INTEGRATION_UNAVAILABLE',
  'MOSS_RESPONSE_INVALID',
  'MOSS_FEATURE_DISABLED',
  'MOSS_NOT_INSTALLED',
  'MOSS_UNHEALTHY',
  'MOSS_RUN_ALREADY_ACTIVE',
  'MOSS_RUN_NOT_FOUND',
  'MOSS_CANDIDATE_NOT_READY',
  'MOSS_CANDIDATE_CONFLICT',
  'MOSS_CANDIDATE_STALE',
  'MOSS_INVALID_BINDING',
  'MOSS_INVALID_OVERRIDE',
  'MOSS_CORRECTION_NOT_FOUND',
  'MOSS_ACTIVATION_CONFLICT',
  'MOSS_ROLLBACK_CONFLICT',
  'MOSS_CANCEL_FAILED',
  'MOSS_QWEN_BUSY',
  'MOSS_OPERATION_FAILED',
]);

export type MossInvoker = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

function createDebugId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `moss-client-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function safeDebugId(value: unknown): string {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{8,96}$/.test(value)
    ? value
    : createDebugId();
}

function invalidResponseError(): MossApiError {
  return {
    code: 'MOSS_RESPONSE_INVALID',
    retryable: false,
    debugId: createDebugId(),
  };
}

export function normalizeMossApiError(error: unknown): MossApiError {
  if (error && typeof error === 'object') {
    const candidate = error as { code?: unknown; retryable?: unknown; debugId?: unknown };
    if (
      typeof candidate.code === 'string'
      && KNOWN_ERROR_CODES.has(candidate.code as MossApiErrorCode)
      && typeof candidate.retryable === 'boolean'
    ) {
      return {
        code: candidate.code as MossApiErrorCode,
        retryable: candidate.retryable,
        debugId: safeDebugId(candidate.debugId),
      };
    }
  }

  // A missing command or an older/mismatched desktop binary can still reject
  // with unstructured text. Never expose or parse that text.
  return {
    code: 'MOSS_FRONTEND_INTEGRATION_UNAVAILABLE',
    retryable: false,
    debugId: createDebugId(),
  };
}

async function invokeMoss<T>(
  invoker: MossInvoker,
  command: string,
  schema: ZodType<T>,
  request?: object,
): Promise<T> {
  let payload: unknown;
  try {
    payload = await invoker<unknown>(command, request ? { request } : undefined);
  } catch (error) {
    throw normalizeMossApiError(error);
  }

  const parsed = schema.safeParse(payload);
  if (!parsed.success) {
    throw invalidResponseError();
  }
  return parsed.data;
}

function verifyWorkspaceMeeting(workspace: MossWorkspace, expectedMeetingId: string): MossWorkspace {
  if (workspace.meetingId !== expectedMeetingId) throw invalidResponseError();
  return workspace;
}

export class MossReviewService {
  constructor(private readonly invoker: MossInvoker = invoke) {}

  getSystemStatus(): Promise<MossSystemStatus> {
    return invokeMoss(this.invoker, MOSS_COMMANDS.getSystemStatus, mossSystemStatusSchema);
  }

  async getWorkspace(request: MossWorkspaceRequest): Promise<MossWorkspace> {
    const workspace = await invokeMoss(
      this.invoker,
      MOSS_COMMANDS.getWorkspace,
      mossWorkspaceSchema,
      { meetingId: request.meetingId, selectedRunId: request.selectedRunId ?? null },
    );
    return verifyWorkspaceMeeting(workspace, request.meetingId);
  }

  startRun(request: StartMossRunRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.startRun, request);
  }

  cancelRun(request: MossRunRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.cancelRun, request);
  }

  saveSpeakerBinding(request: SaveMossSpeakerBindingRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.saveSpeakerBinding, request);
  }

  saveSegmentOverride(request: SaveMossSegmentOverrideRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.saveSegmentOverride, request);
  }

  setCorrectionState(request: SetMossCorrectionStateRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.setCorrectionState, request);
  }

  updateCandidateSegment(request: UpdateMossCandidateSegmentRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.updateCandidateSegment, request);
  }

  activateCandidate(request: ActivateMossCandidateRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.activateCandidate, request);
  }

  rollbackActivation(request: RollbackMossActivationRequest): Promise<MossWorkspace> {
    return this.mutate(MOSS_COMMANDS.rollbackActivation, request);
  }

  private async mutate(
    command: string,
    request: object & { meetingId: string },
  ): Promise<MossWorkspace> {
    const workspace = await invokeMoss(this.invoker, command, mossWorkspaceSchema, request);
    return verifyWorkspaceMeeting(workspace, request.meetingId);
  }
}

export const mossReviewService = new MossReviewService();
