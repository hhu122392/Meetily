import assert from 'node:assert/strict';
import test from 'node:test';
import { mossSystemStatusSchema, mossWorkspaceSchema } from '../../src/features/moss/schemas';
import {
  MOSS_COMMANDS,
  MossReviewService,
  normalizeMossApiError,
  type MossInvoker,
} from '../../src/features/moss/service';
import { mossSystemStatusFixture, mossWorkspaceFixture } from './fixtures';

test('maps every P4 operation to one explicit P3 command and wraps camel-case request data', async () => {
  const workspace = mossWorkspaceFixture();
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  const invoker: MossInvoker = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    return structuredClone(workspace) as T;
  };
  const service = new MossReviewService(invoker);

  await service.getWorkspace({ meetingId: 'meeting-1', selectedRunId: 'run-1' });
  await service.startRun({ meetingId: 'meeting-1' });
  await service.cancelRun({ meetingId: 'meeting-1', runId: 'run-1' });
  await service.saveSpeakerBinding({
    meetingId: 'meeting-1', runId: 'run-1', speakerLabel: 'S01', personId: 'person-ben', expectedCandidateRevision: 3,
  });
  await service.saveSegmentOverride({
    meetingId: 'meeting-1', runId: 'run-1', segmentId: 'candidate-2', personId: 'person-ben', expectedCandidateRevision: 3,
  });
  await service.setCorrectionState({
    meetingId: 'meeting-1', runId: 'run-1', correctionId: 'correction-1', applied: false, expectedCandidateRevision: 3,
  });
  await service.updateCandidateSegment({
    meetingId: 'meeting-1', runId: 'run-1', segmentId: 'candidate-1', text: 'edited', expectedCandidateRevision: 3,
  });
  await service.activateCandidate({
    meetingId: 'meeting-1', runId: 'run-1', expectedCandidateRevision: 3, expectedCurrentTranscriptSha256: 'd'.repeat(64),
  });
  await service.rollbackActivation({
    meetingId: 'meeting-1', activationId: 'activation-1', expectedCurrentTranscriptSha256: 'e'.repeat(64),
  });

  assert.deepEqual(calls.map((call) => call.command), [
    MOSS_COMMANDS.getWorkspace,
    MOSS_COMMANDS.startRun,
    MOSS_COMMANDS.cancelRun,
    MOSS_COMMANDS.saveSpeakerBinding,
    MOSS_COMMANDS.saveSegmentOverride,
    MOSS_COMMANDS.setCorrectionState,
    MOSS_COMMANDS.updateCandidateSegment,
    MOSS_COMMANDS.activateCandidate,
    MOSS_COMMANDS.rollbackActivation,
  ]);
  assert.deepEqual(calls[0].args, {
    request: { meetingId: 'meeting-1', selectedRunId: 'run-1' },
  });
  assert.deepEqual(calls[3].args, {
    request: {
      meetingId: 'meeting-1',
      runId: 'run-1',
      speakerLabel: 'S01',
      personId: 'person-ben',
      expectedCandidateRevision: 3,
    },
  });
});

test('system status is a separate read-only command with no fabricated request', async () => {
  const calls: Array<{ command: string; args?: Record<string, unknown> }> = [];
  const invoker: MossInvoker = async <T>(command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    return structuredClone(mossSystemStatusFixture) as T;
  };
  const service = new MossReviewService(invoker);
  const result = await service.getSystemStatus();
  assert.equal(result.version, '0.2.2');
  assert.deepEqual(calls, [{ command: MOSS_COMMANDS.getSystemStatus, args: undefined }]);
});

test('rejects a response that claims native hotwords exist', () => {
  const status = { ...mossSystemStatusFixture, supportsNativeHotwords: true };
  assert.equal(mossSystemStatusSchema.safeParse(status).success, false);
});

test('rejects invalid hashes and time-reversed candidate segments', () => {
  const workspace = mossWorkspaceFixture();
  workspace.review!.candidate.sha256 = 'not-a-hash';
  workspace.review!.candidate.segments[0].endMs = -1;
  assert.equal(mossWorkspaceSchema.safeParse(workspace).success, false);
});

test('native MOSS decode proof rejects missing, automatic, non-Chinese, and mismatched hash fields', () => {
  const missing = mossWorkspaceFixture();
  delete (missing.runs[0] as unknown as Record<string, unknown>).languageResolved;
  assert.equal(mossWorkspaceSchema.safeParse(missing).success, false);

  const automatic = mossWorkspaceFixture();
  (automatic.runs[0] as unknown as Record<string, unknown>).languageRequested = 'auto';
  assert.equal(mossWorkspaceSchema.safeParse(automatic).success, false);

  const nonChinese = mossWorkspaceFixture();
  (nonChinese.runs[0] as unknown as Record<string, unknown>).languageResolved = 'en-US';
  assert.equal(mossWorkspaceSchema.safeParse(nonChinese).success, false);

  const mismatchedHash = mossWorkspaceFixture();
  (mismatchedHash.runs[0] as unknown as Record<string, unknown>).decodeParametersSha256 = '0'.repeat(64);
  assert.equal(mossWorkspaceSchema.safeParse(mismatchedHash).success, false);
});

test('rejects alignment evidence or audio-tail diagnostics that contradict the candidate', () => {
  const wrongSegment = mossWorkspaceFixture();
  wrongSegment.review!.candidate.alignments[0].segmentId = 'candidate-other';
  assert.equal(mossWorkspaceSchema.safeParse(wrongSegment).success, false);

  const fakeSourceAlignment = mossWorkspaceFixture();
  fakeSourceAlignment.review!.candidate.alignments[1].sourceAnchorIds = [];
  assert.equal(mossWorkspaceSchema.safeParse(fakeSourceAlignment).success, false);

  const expandedSourceTime = mossWorkspaceFixture();
  expandedSourceTime.review!.candidate.alignments[1].rawStartMs = 1600;
  assert.equal(mossWorkspaceSchema.safeParse(expandedSourceTime).success, false);

  const wrongTail = mossWorkspaceFixture();
  wrongTail.review!.candidate.diagnostics!.tailDeltaMs = 999;
  assert.equal(mossWorkspaceSchema.safeParse(wrongTail).success, false);

  const wrongCounts = mossWorkspaceFixture();
  wrongCounts.review!.candidate.diagnostics!.alignedSegmentCount = 99;
  assert.equal(mossWorkspaceSchema.safeParse(wrongCounts).success, false);
});

test('accepts one fully bound R5 audio-token candidate and rejects every broken binding', () => {
  const verified = mossWorkspaceFixture();
  const trackSha256 = '9'.repeat(64);
  const modelSha256 = '8'.repeat(64);
  const tokenAlignment = verified.review!.candidate.alignments[1];
  tokenAlignment.alignmentMethod = 'whisper_audio_token';
  tokenAlignment.confidence = 0.91;
  tokenAlignment.sourceAnchorIds = ['whisper-token-000010-000012'];
  tokenAlignment.audioTokenTrackSha256 = trackSha256;
  tokenAlignment.firstAudioTokenIndex = 10;
  tokenAlignment.lastAudioTokenIndex = 12;
  verified.review!.candidate.segments[1].textSourceLayer = 'audio_token_context';
  verified.review!.candidate.audioTokenAlignment = {
    status: 'verified',
    audioSha256: '7'.repeat(64),
    audioDurationMs: 5000,
    modelName: 'large-v3-turbo-q5_0',
    modelSha256,
    programSha256: '6'.repeat(64),
    parametersSha256: '5'.repeat(64),
    tokenTrackSha256: trackSha256,
    backend: 'cpu',
    globalMatchCoverage: 0.75,
    tokenAlignedSegmentCount: 1,
    fallbackRawSegmentCount: 2,
    fallbackReason: 'PARTIAL_AUDIO_TOKEN_FALLBACK',
  };
  verified.review!.corrections = [{
    correctionId: 'correction-r5',
    segmentId: 'candidate-2',
    originalText: '谷歌',
    correctedText: 'Google',
    matchedAlias: '谷歌',
    canonical: 'Google',
    ruleId: 'R5_AUDIO_TOKEN_CONTEXT:term-google',
    contextRevision: 2,
    state: 'applied',
    sourceLayer: 'audio_token_context',
    machineSource: {
      termId: 'term-google',
      contextSha256: '4'.repeat(64),
      tokenTrackSha256: trackSha256,
      modelSha256,
      firstTokenIndex: 11,
      lastTokenIndex: 11,
      confidence: 0.9,
    },
  }];
  assert.equal(mossWorkspaceSchema.safeParse(verified).success, true);

  const wrongTrack = structuredClone(verified);
  wrongTrack.review!.candidate.alignments[1].audioTokenTrackSha256 = '3'.repeat(64);
  assert.equal(mossWorkspaceSchema.safeParse(wrongTrack).success, false);

  const lowCoverage = structuredClone(verified);
  lowCoverage.review!.candidate.audioTokenAlignment!.globalMatchCoverage = 0.49;
  assert.equal(mossWorkspaceSchema.safeParse(lowCoverage).success, false);

  const missingProgramBinding = structuredClone(verified);
  missingProgramBinding.review!.candidate.audioTokenAlignment!.programSha256 = null;
  assert.equal(mossWorkspaceSchema.safeParse(missingProgramBinding).success, false);

  const correctionOutsideBoundary = structuredClone(verified);
  correctionOutsideBoundary.review!.corrections[0].machineSource!.firstTokenIndex = 13;
  correctionOutsideBoundary.review!.corrections[0].machineSource!.lastTokenIndex = 13;
  assert.equal(mossWorkspaceSchema.safeParse(correctionOutsideBoundary).success, false);

  const fallback = mossWorkspaceFixture();
  fallback.review!.candidate.audioTokenAlignment = {
    status: 'fallback',
    audioSha256: '7'.repeat(64),
    audioDurationMs: null,
    modelName: null,
    modelSha256: null,
    programSha256: null,
    parametersSha256: null,
    tokenTrackSha256: null,
    backend: null,
    globalMatchCoverage: null,
    tokenAlignedSegmentCount: 0,
    fallbackRawSegmentCount: 3,
    fallbackReason: 'AUDIO_TOKEN_MODEL_MISSING',
  };
  assert.equal(mossWorkspaceSchema.safeParse(fallback).success, true);
});

test('rejects cross-meeting and internally inconsistent workspace data', async () => {
  const wrongMeeting = mossWorkspaceFixture();
  wrongMeeting.meetingId = 'meeting-other';
  wrongMeeting.runs.forEach((run) => { run.meetingId = 'meeting-other'; });
  wrongMeeting.review!.meetingId = 'meeting-other';
  wrongMeeting.review!.run.meetingId = 'meeting-other';
  assert.equal(mossWorkspaceSchema.safeParse(wrongMeeting).success, true);
  const service = new MossReviewService(async <T>() => structuredClone(wrongMeeting) as T);
  await assert.rejects(
    service.getWorkspace({ meetingId: 'meeting-1' }),
    (error: unknown) => (error as { code?: string }).code === 'MOSS_RESPONSE_INVALID',
  );

  const mismatchedRun = mossWorkspaceFixture();
  mismatchedRun.review!.candidate.runId = 'run-other';
  assert.equal(mossWorkspaceSchema.safeParse(mismatchedRun).success, false);

  const mismatchedActivation = mossWorkspaceFixture();
  mismatchedActivation.review!.candidate.isActive = true;
  assert.equal(mossWorkspaceSchema.safeParse(mismatchedActivation).success, false);
});

test('turns malformed backend data into a controlled contract error', async () => {
  const service = new MossReviewService(async <T>() => ({ schemaVersion: 1 }) as T);
  await assert.rejects(
    service.getWorkspace({ meetingId: 'meeting-1' }),
    (error: unknown) => {
      assert.deepEqual((error as { code: string }).code, 'MOSS_RESPONSE_INVALID');
      assert.doesNotMatch(JSON.stringify(error), /schemaVersion.*issues|Zod/i);
      return true;
    },
  );
});

test('preserves only a complete whitelisted error and never exposes raw Tauri text', () => {
  const structured = normalizeMossApiError({
    code: 'MOSS_ACTIVATION_CONFLICT',
    retryable: true,
    debugId: 'server-reference-123',
    message: 'private transcript text',
  });
  assert.deepEqual(structured, {
    code: 'MOSS_ACTIVATION_CONFLICT',
    retryable: true,
    debugId: 'server-reference-123',
  });

  const fallback = normalizeMossApiError(
    'command missing at D:\\MeetilyData\\private\\meeting.wav',
  );
  assert.equal(fallback.code, 'MOSS_FRONTEND_INTEGRATION_UNAVAILABLE');
  assert.equal(fallback.retryable, false);
  assert.doesNotMatch(JSON.stringify(fallback), /MeetilyData|meeting\.wav|command missing/i);
});
