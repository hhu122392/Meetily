import {
  SUMMARY_SOURCE_BINDING_SCHEMA_VERSION,
  type SummaryFreshness,
  type SummarySourceBinding,
  type SummaryStaleReason,
  type TranscriptVersionPointer,
} from '@/types/summary-source';

const SHA256 = /^[a-f0-9]{64}$/;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9_.-]{0,199}$/;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isPositiveSafeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) > 0;
}

function isIdentifier(value: unknown): value is string {
  return typeof value === 'string' && IDENTIFIER.test(value);
}

function isHash(value: unknown): value is string {
  return typeof value === 'string' && SHA256.test(value);
}

export function isSummarySourceBinding(value: unknown): value is SummarySourceBinding {
  if (!isRecord(value) || !isRecord(value.template)) return false;

  const source = value.transcriptSource;
  const mossRunId = value.mossRunId;
  const activatedAt = value.transcriptActivatedAt;
  const hasRecordedActivation =
    typeof activatedAt === 'string' && Number.isFinite(Date.parse(activatedAt));
  const sourceIsValid =
    ((source === 'whisper' ||
      source === 'sensevoice' ||
      source === 'parakeet' ||
      source === 'manual') &&
      mossRunId === null &&
      (activatedAt === null || hasRecordedActivation)) ||
    (source === 'moss' && isIdentifier(mossRunId) && hasRecordedActivation);

  return (
    value.schemaVersion === SUMMARY_SOURCE_BINDING_SCHEMA_VERSION &&
    isIdentifier(value.meetingId) &&
    isIdentifier(value.transcriptVersionId) &&
    isPositiveSafeInteger(value.transcriptVersion) &&
    sourceIsValid &&
    isHash(value.transcriptSha256) &&
    isIdentifier(value.speakerBindingSnapshotId) &&
    isPositiveSafeInteger(value.speakerBindingVersion) &&
    isHash(value.speakerBindingSha256) &&
    isIdentifier(value.template.templateId) &&
    isPositiveSafeInteger(value.template.templateVersion) &&
    isHash(value.template.templateFileSha256) &&
    isHash(value.template.templateSemanticSha256)
  );
}

/** Reads lineage only when every required field is valid. Missing or malformed
 * metadata returns null instead of inventing a source version. */
export function readSummarySourceBinding(payload: unknown): SummarySourceBinding | null {
  if (!isRecord(payload)) return null;

  const direct = payload.sourceBinding;
  if (isSummarySourceBinding(direct)) return direct;

  const snapshotDetails = payload.summarySourceBinding;
  if (isSummarySourceBinding(snapshotDetails)) return snapshotDetails;

  const snapshot = payload.template_snapshot;
  if (!isRecord(snapshot)) return null;
  return isSummarySourceBinding(snapshot.summarySourceBinding)
    ? snapshot.summarySourceBinding
    : null;
}

const STALE_REASONS = new Set([
  'transcript_version_changed',
  'transcript_content_changed',
  'speaker_bindings_changed',
  'template_changed',
  'source_binding_unavailable',
]);

export function readSummaryFreshness(payload: unknown): SummaryFreshness | null {
  if (!isRecord(payload) || !isRecord(payload.summaryFreshness)) return null;
  const value = payload.summaryFreshness;
  if (
    !['current', 'stale', 'unavailable'].includes(String(value.status)) ||
    !Array.isArray(value.reasons) ||
    !value.reasons.every((reason) => typeof reason === 'string' && STALE_REASONS.has(reason))
  ) {
    return null;
  }
  if (value.status === 'current' && value.reasons.length !== 0) return null;
  if (value.status !== 'current' && value.reasons.length === 0) return null;
  return value as unknown as SummaryFreshness;
}

/** P4 can pass all candidate rows here; only the single activated version is
 * eligible for summary generation. */
export function selectActivatedTranscriptVersion<T extends TranscriptVersionPointer>(
  meetingId: string,
  versions: readonly T[],
): T {
  const active = versions.filter(
    (version) => version.meetingId === meetingId && version.state === 'active',
  );
  if (active.length !== 1) {
    throw new Error(
      active.length === 0
        ? 'SUMMARY_SOURCE_NO_ACTIVE_VERSION'
        : 'SUMMARY_SOURCE_MULTIPLE_ACTIVE_VERSIONS',
    );
  }
  return active[0];
}

export function evaluateSummaryFreshness(
  generatedFrom: SummarySourceBinding,
  current: SummarySourceBinding,
): SummaryFreshness {
  if (!isSummarySourceBinding(generatedFrom) || !isSummarySourceBinding(current)) {
    throw new Error('SUMMARY_SOURCE_BINDING_INVALID');
  }
  if (generatedFrom.meetingId !== current.meetingId) {
    throw new Error('SUMMARY_SOURCE_MEETING_ID_INVALID');
  }

  const reasons: SummaryStaleReason[] = [];
  if (
    generatedFrom.transcriptVersionId !== current.transcriptVersionId ||
    generatedFrom.transcriptVersion !== current.transcriptVersion ||
    generatedFrom.transcriptSource !== current.transcriptSource ||
    generatedFrom.mossRunId !== current.mossRunId
  ) {
    reasons.push('transcript_version_changed');
  }
  if (generatedFrom.transcriptSha256 !== current.transcriptSha256) {
    reasons.push('transcript_content_changed');
  }
  if (
    generatedFrom.speakerBindingSnapshotId !== current.speakerBindingSnapshotId ||
    generatedFrom.speakerBindingVersion !== current.speakerBindingVersion ||
    generatedFrom.speakerBindingSha256 !== current.speakerBindingSha256
  ) {
    reasons.push('speaker_bindings_changed');
  }
  if (
    generatedFrom.template.templateId !== current.template.templateId ||
    generatedFrom.template.templateVersion !== current.template.templateVersion ||
    generatedFrom.template.templateFileSha256 !== current.template.templateFileSha256 ||
    generatedFrom.template.templateSemanticSha256 !== current.template.templateSemanticSha256
  ) {
    reasons.push('template_changed');
  }

  return { status: reasons.length === 0 ? 'current' : 'stale', reasons };
}
