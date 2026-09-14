import { z } from 'zod';

const sha256 = z.string().regex(/^[a-f0-9]{64}$/i);
const nullableSha256 = sha256.nullable();
const nonNegativeInteger = z.number().int().nonnegative();
const isoTimestamp = z.string().datetime({ offset: true });

export const mossSystemStatusSchema = z.object({
  schemaVersion: z.literal(1),
  availability: z.enum(['ready', 'not_installed', 'unhealthy', 'unsupported']),
  installed: z.boolean(),
  version: z.string().min(1).nullable(),
  runtimeSha256: nullableSha256,
  modelSha256: nullableSha256,
  modelBytes: nonNegativeInteger.nullable(),
  availableDiskBytes: nonNegativeInteger.nullable(),
  deviceName: z.string().min(1).nullable(),
  health: z.enum(['healthy', 'degraded', 'failed', 'unknown']),
  supportsNativeHotwords: z.literal(false),
  detailCode: z.string().min(1).nullable(),
  checkedAt: isoTimestamp,
});

const mossRunProgressSchema = z.object({
  stage: z.enum([
    'queued',
    'validating',
    'decoding',
    'loading_model',
    'transcribing',
    'parsing',
    'saving_candidate',
    'complete',
  ]),
  percentage: z.number().min(0).max(100),
});

export const mossRunSummarySchema = z.object({
  runId: z.string().min(1),
  meetingId: z.string().min(1),
  state: z.enum(['preparing', 'running', 'cancel_requested', 'cancelled', 'failed', 'completed']),
  progress: mossRunProgressSchema.nullable(),
  createdAt: isoTimestamp,
  updatedAt: isoTimestamp,
  completedAt: isoTimestamp.nullable(),
  errorCode: z.string().min(1).nullable(),
  canCancel: z.boolean(),
  candidateRevision: nonNegativeInteger.nullable(),
  candidateSha256: nullableSha256,
  languageRequested: z.literal('zh-CN').nullable(),
  languageResolved: z.literal('zh-CN').nullable(),
  decodeParametersJson: z.literal('{"language":"zh","timestamps":"segment","diarize":"on"}').nullable(),
  decodeParametersSha256: z.literal('1b8ee2dde060ce156ff43cd9b08a19f54060018bab428373b7094ee887e0d47e').nullable(),
});

const mossParticipantSchema = z.object({
  personId: z.string().min(1),
  displayName: z.string().min(1),
  attendance: z.enum(['attending', 'expected', 'guest']),
  department: z.string().min(1).nullable(),
  role: z.string().min(1).nullable(),
});

const mossSpeakerBindingSchema = z.object({
  speakerLabel: z.string().regex(/^S\d{2,4}$/),
  personId: z.string().min(1).nullable(),
});

const mossTranscriptSegmentSchema = z.object({
  segmentId: z.string().min(1),
  startMs: nonNegativeInteger,
  endMs: nonNegativeInteger,
  speakerLabel: z.string().min(1).nullable(),
  text: z.string(),
  resolvedPersonId: z.string().min(1).nullable(),
  segmentOverridePersonId: z.string().min(1).nullable(),
  speakerResolution: z.enum(['anonymous', 'bulk_binding', 'segment_override']),
  textSourceLayer: z.enum([
    'current_transcript',
    'raw_moss',
    'context_correction',
    'audio_token_context',
    'human_edit',
  ]),
}).refine((segment) => segment.endMs >= segment.startMs, {
  message: 'segment end must not precede start',
});

const mossTranscriptVersionSchema = z.object({
  source: z.enum(['whisper', 'sensevoice', 'parakeet', 'moss', 'manual']),
  revision: nonNegativeInteger,
  sha256,
  segments: z.array(mossTranscriptSegmentSchema),
});

const mossCandidateVersionSchema = mossTranscriptVersionSchema.extend({
  source: z.literal('moss'),
  runId: z.string().min(1),
  isActive: z.boolean(),
  alignments: z.array(z.object({
    segmentId: z.string().min(1),
    rawSegmentIndex: nonNegativeInteger,
    rawStartMs: nonNegativeInteger,
    rawEndMs: nonNegativeInteger,
    rawTextSha256: sha256,
    alignmentMethod: z.enum(['moss_segment', 'source_transcript_segment', 'whisper_audio_token']),
    confidence: z.number().min(0).max(1).nullable(),
    sourceAnchorIds: z.array(z.string().min(1)),
    sourceTranscriptSha256: sha256,
    audioTokenTrackSha256: sha256.nullable(),
    firstAudioTokenIndex: nonNegativeInteger.nullable(),
    lastAudioTokenIndex: nonNegativeInteger.nullable(),
  }).superRefine((alignment, context) => {
    if (alignment.rawEndMs < alignment.rawStartMs) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['rawEndMs'], message: 'raw segment end must not precede start' });
    }
    const audioFields = [
      alignment.audioTokenTrackSha256,
      alignment.firstAudioTokenIndex,
      alignment.lastAudioTokenIndex,
    ];
    if (alignment.alignmentMethod === 'moss_segment') {
      if (alignment.confidence !== null || alignment.sourceAnchorIds.length !== 0 || audioFields.some((value) => value !== null)) {
        context.addIssue({ code: z.ZodIssueCode.custom, message: 'raw MOSS fallback cannot claim an alignment source' });
      }
    } else if (alignment.alignmentMethod === 'source_transcript_segment') {
      if (alignment.confidence === null || alignment.sourceAnchorIds.length === 0 || audioFields.some((value) => value !== null)) {
        context.addIssue({ code: z.ZodIssueCode.custom, message: 'source transcript alignment evidence is incomplete' });
      }
    } else if (
      alignment.confidence === null
      || alignment.sourceAnchorIds.length !== 1
      || audioFields.some((value) => value === null)
      || (alignment.firstAudioTokenIndex ?? 0) > (alignment.lastAudioTokenIndex ?? 0)
    ) {
      context.addIssue({ code: z.ZodIssueCode.custom, message: 'audio-token alignment evidence is incomplete' });
    }
  })),
  diagnostics: z.object({
    audioDurationMs: z.number().int().positive(),
    activityFrameMs: z.literal(20),
    activityThresholdDbfs: z.literal(-50),
    firstActiveMs: nonNegativeInteger.nullable(),
    lastActiveMs: nonNegativeInteger.nullable(),
    modelLastTimestampMs: nonNegativeInteger,
    tailDeltaMs: z.number().int().nullable(),
    alignedSegmentCount: nonNegativeInteger,
    fallbackSegmentCount: nonNegativeInteger,
    sourceAnchorCount: nonNegativeInteger,
    sourceHashVerified: z.boolean(),
    sourceExpectedSha256: sha256,
    sourceActualSha256: sha256,
    fallbackReason: z.string().regex(/^[A-Z0-9_]+$/).nullable(),
  }).superRefine((diagnostics, context) => {
    const hasFirst = diagnostics.firstActiveMs !== null;
    const hasLast = diagnostics.lastActiveMs !== null;
    if (hasFirst !== hasLast) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['lastActiveMs'], message: 'activity bounds must both be present or absent' });
    }
    if (
      diagnostics.firstActiveMs !== null
      && diagnostics.lastActiveMs !== null
      && (
        diagnostics.firstActiveMs > diagnostics.lastActiveMs
        || diagnostics.lastActiveMs > diagnostics.audioDurationMs
      )
    ) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['lastActiveMs'], message: 'activity bounds are invalid' });
    }
    const expectedTail = diagnostics.lastActiveMs === null
      ? null
      : diagnostics.modelLastTimestampMs - diagnostics.lastActiveMs;
    if (diagnostics.tailDeltaMs !== expectedTail) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['tailDeltaMs'], message: 'tail delta does not match measured activity' });
    }
    if (
      diagnostics.sourceHashVerified
      && diagnostics.sourceExpectedSha256 !== diagnostics.sourceActualSha256
    ) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['sourceActualSha256'], message: 'verified source hashes must match' });
    }
    if ((diagnostics.fallbackSegmentCount > 0) !== (diagnostics.fallbackReason !== null)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['fallbackReason'], message: 'fallback reason must match fallback count' });
    }
  }).nullable(),
  audioTokenAlignment: z.object({
    status: z.enum(['verified', 'fallback']),
    audioSha256: sha256,
    audioDurationMs: z.number().int().positive().nullable(),
    modelName: z.string().min(1).nullable(),
    modelSha256: nullableSha256,
    programSha256: nullableSha256,
    parametersSha256: nullableSha256,
    tokenTrackSha256: nullableSha256,
    backend: z.string().min(1).nullable(),
    globalMatchCoverage: z.number().min(0).max(1).nullable(),
    tokenAlignedSegmentCount: nonNegativeInteger,
    fallbackRawSegmentCount: nonNegativeInteger,
    fallbackReason: z.string().regex(/^[A-Z0-9_]+$/).nullable(),
  }).superRefine((value, context) => {
    const verifiedFields = [
      value.audioDurationMs,
      value.modelName,
      value.modelSha256,
      value.programSha256,
      value.parametersSha256,
      value.tokenTrackSha256,
      value.backend,
      value.globalMatchCoverage,
    ];
    if (value.status === 'verified' && verifiedFields.some((field) => field === null)) {
      context.addIssue({ code: z.ZodIssueCode.custom, message: 'verified audio-token alignment is missing a binding field' });
    }
    if (value.status === 'fallback' && verifiedFields.some((field) => field !== null)) {
      context.addIssue({ code: z.ZodIssueCode.custom, message: 'fallback audio-token alignment cannot claim a verified track' });
    }
    if ((value.fallbackRawSegmentCount > 0) !== (value.fallbackReason !== null)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['fallbackReason'], message: 'audio-token fallback reason must match fallback count' });
    }
  }).nullable(),
});

const mossTermCorrectionSchema = z.object({
  correctionId: z.string().min(1),
  segmentId: z.string().min(1),
  originalText: z.string(),
  correctedText: z.string(),
  matchedAlias: z.string().min(1),
  canonical: z.string().min(1),
  ruleId: z.string().min(1),
  contextRevision: nonNegativeInteger,
  state: z.enum(['applied', 'reverted']),
  sourceLayer: z.enum(['context_alias', 'audio_token_context']),
  machineSource: z.object({
    termId: z.string().min(1),
    contextSha256: sha256,
    tokenTrackSha256: sha256,
    modelSha256: sha256,
    firstTokenIndex: nonNegativeInteger,
    lastTokenIndex: nonNegativeInteger,
    confidence: z.number().min(0).max(1),
  }).refine((value) => value.lastTokenIndex >= value.firstTokenIndex, {
    message: 'machine correction token range is reversed',
  }).nullable(),
}).superRefine((correction, context) => {
  if ((correction.sourceLayer === 'audio_token_context') !== (correction.machineSource !== null)) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['machineSource'], message: 'correction source layer is inconsistent' });
  }
});

const mossActivationStateSchema = z.object({
  activeRunId: z.string().min(1).nullable(),
  activeActivationId: z.string().min(1).nullable(),
  currentTranscriptSha256: sha256,
  canActivate: z.boolean(),
  activateBlocker: z.enum([
    'CURRENT_TRANSCRIPT_CHANGED',
    'CANDIDATE_STALE',
    'CANDIDATE_INCOMPLETE',
  ]).nullable(),
  canRollback: z.boolean(),
});

export const mossCandidateReviewSchema = z.object({
  schemaVersion: z.literal(1),
  meetingId: z.string().min(1),
  run: mossRunSummarySchema,
  current: mossTranscriptVersionSchema,
  candidate: mossCandidateVersionSchema,
  participants: z.array(mossParticipantSchema),
  anonymousSpeakers: z.array(z.string().regex(/^S\d{2,4}$/)),
  bindings: z.array(mossSpeakerBindingSchema),
  corrections: z.array(mossTermCorrectionSchema),
  activation: mossActivationStateSchema,
});

export const mossWorkspaceSchema = z.object({
  schemaVersion: z.literal(1),
  meetingId: z.string().min(1),
  system: mossSystemStatusSchema,
  runs: z.array(mossRunSummarySchema),
  selectedRunId: z.string().min(1).nullable(),
  review: mossCandidateReviewSchema.nullable(),
}).superRefine((workspace, context) => {
  const runIds = new Set(workspace.runs.map((run) => run.runId));
  if (runIds.size !== workspace.runs.length) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['runs'], message: 'run ids must be unique' });
  }
  workspace.runs.forEach((run, index) => {
    if (run.meetingId !== workspace.meetingId) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['runs', index, 'meetingId'], message: 'run meeting mismatch' });
    }
  });
  if (workspace.selectedRunId && !runIds.has(workspace.selectedRunId)) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['selectedRunId'], message: 'selected run is missing' });
  }

  const review = workspace.review;
  if (!review) return;
  if (review.meetingId !== workspace.meetingId) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'meetingId'], message: 'review meeting mismatch' });
  }
  if (review.run.runId !== review.candidate.runId || !runIds.has(review.run.runId)) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'run'], message: 'review run mismatch' });
  }
  if (workspace.selectedRunId !== review.run.runId) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review'], message: 'review must match selected run' });
  }
  if (
    review.run.candidateRevision !== review.candidate.revision
    || review.run.candidateSha256 !== review.candidate.sha256
  ) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate'], message: 'candidate summary mismatch' });
  }
  if (review.activation.currentTranscriptSha256 !== review.current.sha256) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'activation'], message: 'current transcript hash mismatch' });
  }
  if (review.candidate.isActive !== (review.activation.activeRunId === review.candidate.runId)) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'activation', 'activeRunId'], message: 'active candidate state mismatch' });
  }
  if (review.candidate.isActive && review.activation.canActivate) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'activation', 'canActivate'], message: 'active candidate cannot be activated twice' });
  }
  if (review.activation.canRollback && !review.activation.activeActivationId) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'activation', 'activeActivationId'], message: 'rollback requires an activation id' });
  }

  const participantIds = new Set(review.participants.map((participant) => participant.personId));
  if (participantIds.size !== review.participants.length) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'participants'], message: 'participant ids must be unique' });
  }
  const bindingLabels = new Set<string>();
  review.bindings.forEach((binding, index) => {
    if (bindingLabels.has(binding.speakerLabel)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'bindings', index], message: 'speaker binding must be unique' });
    }
    bindingLabels.add(binding.speakerLabel);
    if (binding.personId && !participantIds.has(binding.personId)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'bindings', index, 'personId'], message: 'binding participant is missing' });
    }
  });

  const candidateSegmentIds = new Set<string>();
  review.candidate.segments.forEach((segment, index) => {
    if (candidateSegmentIds.has(segment.segmentId)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'segments', index], message: 'candidate segment ids must be unique' });
    }
    candidateSegmentIds.add(segment.segmentId);
    if (segment.speakerLabel && !/^S\d{2,4}$/.test(segment.speakerLabel)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'segments', index, 'speakerLabel'], message: 'candidate speaker must remain anonymous' });
    }
    if (segment.resolvedPersonId && !participantIds.has(segment.resolvedPersonId)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'segments', index, 'resolvedPersonId'], message: 'resolved participant is missing' });
    }
    if (segment.segmentOverridePersonId && !participantIds.has(segment.segmentOverridePersonId)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'segments', index, 'segmentOverridePersonId'], message: 'override participant is missing' });
    }
  });
  if (review.candidate.alignments.length !== review.candidate.segments.length) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments'], message: 'every candidate segment requires alignment evidence' });
  }
  const alignmentSegmentIds = new Set<string>();
  const referencedSourceAnchorIds = new Set<string>();
  let actualAlignedSegments = 0;
  let actualSourceAlignedSegments = 0;
  let actualAudioTokenSegments = 0;
  let actualFallbackSegments = 0;
  review.candidate.alignments.forEach((alignment, index) => {
    if (
      alignmentSegmentIds.has(alignment.segmentId)
      || review.candidate.segments[index]?.segmentId !== alignment.segmentId
    ) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', index, 'segmentId'], message: 'alignment must match one candidate segment in order' });
    }
    alignmentSegmentIds.add(alignment.segmentId);
    const segment = review.candidate.segments[index];
    if (alignment.alignmentMethod === 'moss_segment') {
      actualFallbackSegments += 1;
      if (
        segment
        && (segment.startMs !== alignment.rawStartMs || segment.endMs !== alignment.rawEndMs)
      ) {
        context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', index], message: 'raw fallback time must match the MOSS segment' });
      }
    } else {
      actualAlignedSegments += 1;
      if (alignment.alignmentMethod === 'source_transcript_segment') {
        actualSourceAlignedSegments += 1;
        alignment.sourceAnchorIds.forEach((anchorId) => referencedSourceAnchorIds.add(anchorId));
      } else {
        actualAudioTokenSegments += 1;
        if (
          alignment.audioTokenTrackSha256
          !== review.candidate.audioTokenAlignment?.tokenTrackSha256
        ) {
          context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', index, 'audioTokenTrackSha256'], message: 'audio-token track hash mismatch' });
        }
      }
      if (
        segment
        && (
          segment.startMs < alignment.rawStartMs
          || segment.endMs > alignment.rawEndMs
          || segment.endMs <= segment.startMs
        )
      ) {
        context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', index], message: 'aligned time must stay inside the MOSS segment' });
      }
    }
    const diagnostics = review.candidate.diagnostics;
    if (diagnostics && alignment.sourceTranscriptSha256 !== diagnostics.sourceExpectedSha256) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', index, 'sourceTranscriptSha256'], message: 'alignment source hash mismatch' });
    }
  });
  let alignmentPosition = 0;
  let expectedRawSegmentIndex = 0;
  let actualAudioFallbackRawSegments = 0;
  while (alignmentPosition < review.candidate.alignments.length) {
    const first = review.candidate.alignments[alignmentPosition];
    let groupEnd = alignmentPosition + 1;
    while (
      groupEnd < review.candidate.alignments.length
      && review.candidate.alignments[groupEnd].rawSegmentIndex === first.rawSegmentIndex
    ) {
      groupEnd += 1;
    }
    const group = review.candidate.alignments.slice(alignmentPosition, groupEnd);
    const outputGroup = review.candidate.segments.slice(alignmentPosition, groupEnd);
    const groupShapeValid = first.rawSegmentIndex === expectedRawSegmentIndex
      && group.every((alignment) => (
        alignment.rawStartMs === first.rawStartMs
        && alignment.rawEndMs === first.rawEndMs
        && alignment.rawTextSha256 === first.rawTextSha256
        && alignment.alignmentMethod === first.alignmentMethod
      ));
    if (!groupShapeValid) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', alignmentPosition], message: 'raw segment provenance group is invalid' });
    }
    if (first.alignmentMethod === 'source_transcript_segment' || first.alignmentMethod === 'whisper_audio_token') {
      const anchorIds = group.map((alignment) => alignment.sourceAnchorIds[0]);
      const partitionValid = first.rawEndMs > first.rawStartMs
        && outputGroup[0]?.startMs === first.rawStartMs
        && outputGroup.at(-1)?.endMs === first.rawEndMs
        && outputGroup.every((segment) => (
          segment.startMs >= first.rawStartMs
          && segment.endMs <= first.rawEndMs
          && segment.endMs > segment.startMs
        ))
        && outputGroup.slice(1).every((segment, index) => (
          outputGroup[index].endMs === segment.startMs
        ))
        && group.every((alignment) => alignment.sourceAnchorIds.length === 1)
        && new Set(anchorIds).size === anchorIds.length;
      if (!partitionValid) {
        context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', alignmentPosition], message: 'source alignment must partition the original MOSS time range' });
      }
    } else if (group.length !== 1) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'alignments', alignmentPosition], message: 'raw fallback must have one output segment' });
    }
    if (first.alignmentMethod !== 'whisper_audio_token') {
      actualAudioFallbackRawSegments += 1;
    }
    expectedRawSegmentIndex += 1;
    alignmentPosition = groupEnd;
  }
  const diagnostics = review.candidate.diagnostics;
  if (
    diagnostics
    && diagnostics.alignedSegmentCount + diagnostics.fallbackSegmentCount
      !== review.candidate.segments.length
  ) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'diagnostics'], message: 'diagnostic segment counts must match candidate' });
  }
  if (
    diagnostics
    && (
      diagnostics.alignedSegmentCount !== actualAlignedSegments
      || diagnostics.fallbackSegmentCount !== actualFallbackSegments
      || referencedSourceAnchorIds.size > diagnostics.sourceAnchorCount
      || (!diagnostics.sourceHashVerified && actualSourceAlignedSegments > 0)
    )
  ) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'diagnostics'], message: 'diagnostic counts must match alignment methods' });
  }
  const audioTokenAlignment = review.candidate.audioTokenAlignment;
  if (audioTokenAlignment) {
    if (
      audioTokenAlignment.tokenAlignedSegmentCount !== actualAudioTokenSegments
      || audioTokenAlignment.fallbackRawSegmentCount !== actualAudioFallbackRawSegments
      || (audioTokenAlignment.status === 'fallback' && actualAudioTokenSegments !== 0)
      || (
        actualAudioTokenSegments > 0
        && (
          audioTokenAlignment.status !== 'verified'
          || audioTokenAlignment.globalMatchCoverage === null
          || audioTokenAlignment.globalMatchCoverage < 0.5
        )
      )
    ) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'audioTokenAlignment'], message: 'audio-token summary does not match candidate provenance' });
    }
  } else if (actualAudioTokenSegments > 0) {
    context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'candidate', 'audioTokenAlignment'], message: 'audio-token segments require a run binding' });
  }
  review.corrections.forEach((correction, index) => {
    if (!candidateSegmentIds.has(correction.segmentId)) {
      context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'corrections', index, 'segmentId'], message: 'correction segment is missing' });
    }
    if (correction.machineSource) {
      const alignment = review.candidate.alignments.find(
        (candidateAlignment) => candidateAlignment.segmentId === correction.segmentId,
      );
      const firstBoundaryToken = alignment?.firstAudioTokenIndex;
      const lastBoundaryToken = alignment?.lastAudioTokenIndex;
      if (
        audioTokenAlignment?.status !== 'verified'
        || alignment?.alignmentMethod !== 'whisper_audio_token'
        || correction.machineSource.tokenTrackSha256 !== audioTokenAlignment.tokenTrackSha256
        || correction.machineSource.modelSha256 !== audioTokenAlignment.modelSha256
        || firstBoundaryToken === null
        || firstBoundaryToken === undefined
        || lastBoundaryToken === null
        || lastBoundaryToken === undefined
        || correction.machineSource.firstTokenIndex < firstBoundaryToken
        || correction.machineSource.lastTokenIndex > lastBoundaryToken
      ) {
        context.addIssue({ code: z.ZodIssueCode.custom, path: ['review', 'corrections', index, 'machineSource'], message: 'machine correction source is not bound to this audio-token segment' });
      }
    }
  });
});
