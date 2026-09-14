export const MOSS_SCHEMA_VERSION = 1 as const;

export type MossAvailability = 'ready' | 'not_installed' | 'unhealthy' | 'unsupported';
export type MossHealth = 'healthy' | 'degraded' | 'failed' | 'unknown';

export interface MossSystemStatus {
  schemaVersion: typeof MOSS_SCHEMA_VERSION;
  availability: MossAvailability;
  installed: boolean;
  version: string | null;
  runtimeSha256: string | null;
  modelSha256: string | null;
  modelBytes: number | null;
  availableDiskBytes: number | null;
  deviceName: string | null;
  health: MossHealth;
  supportsNativeHotwords: false;
  detailCode: string | null;
  checkedAt: string;
}

export type MossRunState =
  | 'preparing'
  | 'running'
  | 'cancel_requested'
  | 'cancelled'
  | 'failed'
  | 'completed';

export type MossRunStage =
  | 'queued'
  | 'validating'
  | 'decoding'
  | 'loading_model'
  | 'transcribing'
  | 'parsing'
  | 'saving_candidate'
  | 'complete';

export interface MossRunProgress {
  stage: MossRunStage;
  percentage: number;
}

export interface MossRunSummary {
  runId: string;
  meetingId: string;
  state: MossRunState;
  progress: MossRunProgress | null;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  errorCode: string | null;
  canCancel: boolean;
  candidateRevision: number | null;
  candidateSha256: string | null;
  languageRequested: string | null;
  languageResolved: string | null;
  decodeParametersJson: string | null;
  decodeParametersSha256: string | null;
}

export interface MossParticipant {
  personId: string;
  displayName: string;
  attendance: 'attending' | 'expected' | 'guest';
  department: string | null;
  role: string | null;
}

export interface MossSpeakerBinding {
  speakerLabel: string;
  personId: string | null;
}

export type MossSpeakerResolution = 'anonymous' | 'bulk_binding' | 'segment_override';
export type MossTextSourceLayer =
  | 'current_transcript'
  | 'raw_moss'
  | 'context_correction'
  | 'audio_token_context'
  | 'human_edit';

export interface MossTranscriptSegment {
  segmentId: string;
  startMs: number;
  endMs: number;
  speakerLabel: string | null;
  text: string;
  resolvedPersonId: string | null;
  segmentOverridePersonId: string | null;
  speakerResolution: MossSpeakerResolution;
  textSourceLayer: MossTextSourceLayer;
}

export interface MossTranscriptVersion {
  source: 'whisper' | 'sensevoice' | 'parakeet' | 'moss' | 'manual';
  revision: number;
  sha256: string;
  segments: MossTranscriptSegment[];
}

export interface MossCandidateVersion extends MossTranscriptVersion {
  source: 'moss';
  runId: string;
  isActive: boolean;
  alignments: MossCandidateAlignment[];
  diagnostics: MossRunDiagnostics | null;
  audioTokenAlignment: MossAudioTokenAlignment | null;
}

export type MossAlignmentMethod =
  | 'moss_segment'
  | 'source_transcript_segment'
  | 'whisper_audio_token';

export interface MossCandidateAlignment {
  segmentId: string;
  rawSegmentIndex: number;
  rawStartMs: number;
  rawEndMs: number;
  rawTextSha256: string;
  alignmentMethod: MossAlignmentMethod;
  confidence: number | null;
  sourceAnchorIds: string[];
  sourceTranscriptSha256: string;
  audioTokenTrackSha256: string | null;
  firstAudioTokenIndex: number | null;
  lastAudioTokenIndex: number | null;
}

export interface MossAudioTokenAlignment {
  status: 'verified' | 'fallback';
  audioSha256: string;
  audioDurationMs: number | null;
  modelName: string | null;
  modelSha256: string | null;
  programSha256: string | null;
  parametersSha256: string | null;
  tokenTrackSha256: string | null;
  backend: string | null;
  globalMatchCoverage: number | null;
  tokenAlignedSegmentCount: number;
  fallbackRawSegmentCount: number;
  fallbackReason: string | null;
}

export interface MossRunDiagnostics {
  audioDurationMs: number;
  activityFrameMs: number;
  activityThresholdDbfs: number;
  firstActiveMs: number | null;
  lastActiveMs: number | null;
  modelLastTimestampMs: number;
  tailDeltaMs: number | null;
  alignedSegmentCount: number;
  fallbackSegmentCount: number;
  sourceAnchorCount: number;
  sourceHashVerified: boolean;
  sourceExpectedSha256: string;
  sourceActualSha256: string;
  fallbackReason: string | null;
}

export type MossCorrectionState = 'applied' | 'reverted';

export interface MossTermCorrection {
  correctionId: string;
  segmentId: string;
  originalText: string;
  correctedText: string;
  matchedAlias: string;
  canonical: string;
  ruleId: string;
  contextRevision: number;
  state: MossCorrectionState;
  sourceLayer: 'context_alias' | 'audio_token_context';
  machineSource: MossMachineCorrectionSource | null;
}

export interface MossMachineCorrectionSource {
  termId: string;
  contextSha256: string;
  tokenTrackSha256: string;
  modelSha256: string;
  firstTokenIndex: number;
  lastTokenIndex: number;
  confidence: number;
}

export type MossActivationBlocker =
  | 'CURRENT_TRANSCRIPT_CHANGED'
  | 'CANDIDATE_STALE'
  | 'CANDIDATE_INCOMPLETE';

export interface MossActivationState {
  activeRunId: string | null;
  activeActivationId: string | null;
  currentTranscriptSha256: string;
  canActivate: boolean;
  activateBlocker: MossActivationBlocker | null;
  canRollback: boolean;
}

export interface MossCandidateReview {
  schemaVersion: typeof MOSS_SCHEMA_VERSION;
  meetingId: string;
  run: MossRunSummary;
  current: MossTranscriptVersion;
  candidate: MossCandidateVersion;
  participants: MossParticipant[];
  anonymousSpeakers: string[];
  bindings: MossSpeakerBinding[];
  corrections: MossTermCorrection[];
  activation: MossActivationState;
}

export interface MossWorkspace {
  schemaVersion: typeof MOSS_SCHEMA_VERSION;
  meetingId: string;
  system: MossSystemStatus;
  runs: MossRunSummary[];
  selectedRunId: string | null;
  review: MossCandidateReview | null;
}

export type MossApiErrorCode =
  | 'MOSS_FRONTEND_INTEGRATION_UNAVAILABLE'
  | 'MOSS_RESPONSE_INVALID'
  | 'MOSS_FEATURE_DISABLED'
  | 'MOSS_NOT_INSTALLED'
  | 'MOSS_UNHEALTHY'
  | 'MOSS_RUN_ALREADY_ACTIVE'
  | 'MOSS_RUN_NOT_FOUND'
  | 'MOSS_CANDIDATE_NOT_READY'
  | 'MOSS_CANDIDATE_CONFLICT'
  | 'MOSS_CANDIDATE_STALE'
  | 'MOSS_INVALID_BINDING'
  | 'MOSS_INVALID_OVERRIDE'
  | 'MOSS_CORRECTION_NOT_FOUND'
  | 'MOSS_ACTIVATION_CONFLICT'
  | 'MOSS_ROLLBACK_CONFLICT'
  | 'MOSS_CANCEL_FAILED'
  | 'MOSS_QWEN_BUSY'
  | 'MOSS_OPERATION_FAILED';

export interface MossApiError {
  code: MossApiErrorCode;
  retryable: boolean;
  debugId: string;
}

export interface MossWorkspaceRequest {
  meetingId: string;
  selectedRunId?: string | null;
}

export interface MossRunRequest {
  meetingId: string;
  runId: string;
}

export interface StartMossRunRequest {
  meetingId: string;
}

export interface SaveMossSpeakerBindingRequest extends MossRunRequest {
  speakerLabel: string;
  personId: string | null;
  expectedCandidateRevision: number;
}

export interface SaveMossSegmentOverrideRequest extends MossRunRequest {
  segmentId: string;
  personId: string | null;
  expectedCandidateRevision: number;
}

export interface SetMossCorrectionStateRequest extends MossRunRequest {
  correctionId: string;
  applied: boolean;
  expectedCandidateRevision: number;
}

export interface UpdateMossCandidateSegmentRequest extends MossRunRequest {
  segmentId: string;
  text: string;
  expectedCandidateRevision: number;
}

export interface ActivateMossCandidateRequest extends MossRunRequest {
  expectedCandidateRevision: number;
  expectedCurrentTranscriptSha256: string;
}

export interface RollbackMossActivationRequest {
  meetingId: string;
  activationId: string;
  expectedCurrentTranscriptSha256: string;
}

export const MOSS_ERROR_I18N_KEYS = {
  MOSS_FRONTEND_INTEGRATION_UNAVAILABLE: 'errors.integrationUnavailable',
  MOSS_RESPONSE_INVALID: 'errors.invalidResponse',
  MOSS_FEATURE_DISABLED: 'errors.featureDisabled',
  MOSS_NOT_INSTALLED: 'errors.notInstalled',
  MOSS_UNHEALTHY: 'errors.unhealthy',
  MOSS_RUN_ALREADY_ACTIVE: 'errors.runAlreadyActive',
  MOSS_RUN_NOT_FOUND: 'errors.runNotFound',
  MOSS_CANDIDATE_NOT_READY: 'errors.candidateNotReady',
  MOSS_CANDIDATE_CONFLICT: 'errors.candidateConflict',
  MOSS_CANDIDATE_STALE: 'errors.candidateStale',
  MOSS_INVALID_BINDING: 'errors.invalidBinding',
  MOSS_INVALID_OVERRIDE: 'errors.invalidOverride',
  MOSS_CORRECTION_NOT_FOUND: 'errors.correctionNotFound',
  MOSS_ACTIVATION_CONFLICT: 'errors.activationConflict',
  MOSS_ROLLBACK_CONFLICT: 'errors.rollbackConflict',
  MOSS_CANCEL_FAILED: 'errors.cancelFailed',
  MOSS_QWEN_BUSY: 'errors.qwenBusy',
  MOSS_OPERATION_FAILED: 'errors.operationFailed',
} as const satisfies Record<MossApiErrorCode, string>;
