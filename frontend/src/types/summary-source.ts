export const SUMMARY_SOURCE_BINDING_SCHEMA_VERSION = 1 as const;

export type TranscriptSourceKind =
  | 'whisper'
  | 'sensevoice'
  | 'parakeet'
  | 'moss'
  | 'manual';
export type TranscriptVersionState = 'candidate' | 'active' | 'failed';

export interface SummaryTemplateBinding {
  templateId: string;
  templateVersion: number;
  templateFileSha256: string;
  templateSemanticSha256: string;
}

export interface SummarySourceBinding {
  schemaVersion: typeof SUMMARY_SOURCE_BINDING_SCHEMA_VERSION;
  meetingId: string;
  transcriptVersionId: string;
  transcriptVersion: number;
  transcriptSource: TranscriptSourceKind;
  mossRunId: string | null;
  transcriptActivatedAt: string | null;
  transcriptSha256: string;
  speakerBindingSnapshotId: string;
  speakerBindingVersion: number;
  speakerBindingSha256: string;
  template: SummaryTemplateBinding;
}

export type TranscriptEvidenceBinding = Omit<SummarySourceBinding, 'template'>;

export interface TranscriptVersionPointer {
  meetingId: string;
  transcriptVersionId: string;
  transcriptVersion: number;
  sourceKind: TranscriptSourceKind;
  mossRunId: string | null;
  state: TranscriptVersionState;
}

export type SummaryStaleReason =
  | 'transcript_version_changed'
  | 'transcript_content_changed'
  | 'speaker_bindings_changed'
  | 'template_changed'
  | 'source_binding_unavailable';

export interface SummaryFreshness {
  status: 'current' | 'stale' | 'unavailable';
  reasons: SummaryStaleReason[];
}

export type SummaryTraceField = 'owner' | 'time' | 'dependency';
export type SummaryTraceStatus = 'supported' | 'needs_review';

export interface SummaryEvidenceReference {
  segmentId: string;
  startMs: number | null;
  endMs: number | null;
  excerptSha256: string;
}

export interface SummaryFieldTrace {
  field: SummaryTraceField;
  value: string;
  markdownLine: number;
  markdownColumn: number | null;
  status: SummaryTraceStatus;
  evidence: SummaryEvidenceReference[];
  task?: string;
  relatedEvidence?: SummaryEvidenceReference[];
}
