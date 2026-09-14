import type { SummarySourceBinding } from './summary-source';

export type TemplateOrigin = 'builtin' | 'bundled' | 'custom';

export type TemplateSourceType =
  | 'builtin'
  | 'manual'
  | 'json_import'
  | 'docx_import'
  | 'doc_import'
  | 'builtin_copy'
  | 'duplicate'
  | 'legacy_migration';

export type TemplateFormat = 'paragraph' | 'list' | 'string';
export type EmptyBehavior = 'omit' | 'show_not_mentioned';

export interface TemplateSource {
  type: TemplateSourceType;
  originalFileName: string | null;
  originalFileSha256: string | null;
  importedAt: string | null;
  copiedFromTemplateId: string | null;
}

export interface TemplateSectionV2 {
  id: string;
  title: string;
  instruction: string;
  format: TemplateFormat;
  itemFormat: string | null;
  exampleItemFormat: string | null;
  required: boolean;
  emptyBehavior: EmptyBehavior;
}

/** Camel-case DTO used at the Tauri transport boundary. */
export interface TemplateV2 {
  schemaVersion: 2;
  id: string;
  name: string;
  description: string;
  version: number;
  locale: string | null;
  tags: string[];
  source: TemplateSource;
  createdAt: string;
  updatedAt: string;
  sections: TemplateSectionV2[];
  extensions: Record<string, unknown>;
}

/** Snake-case JSON shape persisted under the Meetily templates directory. */
export interface TemplateV2File {
  schema_version: 2;
  id: string;
  name: string;
  description: string;
  version: number;
  locale: string | null;
  tags: string[];
  source: {
    type: TemplateSourceType;
    original_file_name: string | null;
    original_file_sha256: string | null;
    imported_at: string | null;
    copied_from_template_id: string | null;
  };
  created_at: string;
  updated_at: string;
  sections: Array<{
    id: string;
    title: string;
    instruction: string;
    format: TemplateFormat;
    item_format: string | null;
    example_item_format: string | null;
    required: boolean;
    empty_behavior: EmptyBehavior;
  }>;
  extensions: Record<string, unknown>;
}

export const MEETING_CONTEXT_EXTENSION_KEY = 'meetily_meeting_context';

export interface MeetingContextPersonProfile {
  person_id: string;
  display_name: string;
  aliases: string[];
  department: string | null;
  role: string | null;
  enabled: boolean;
}

export interface MeetingContextTermProfile {
  term_id: string;
  canonical: string;
  aliases: string[];
  category: string | null;
  enabled: boolean;
}

/** Snake-case shape stored verbatim inside TemplateV2.extensions. */
export interface MeetingContextProfile {
  schema_version: 1;
  fixed_meeting_mechanism: string | null;
  people: MeetingContextPersonProfile[];
  terms: MeetingContextTermProfile[];
}

export interface MeetingContextProfileIssue {
  code: string;
  path: string;
}

export type RecordingAttendanceStatus = 'attending' | 'absent' | 'expected' | 'guest';

export interface RecordingTemplateSelection {
  templateId: string;
  templateVersion: number;
  templateFileSha256: string;
}

export interface RecordingPersonAttendanceOverride {
  personId: string;
  attendance: Exclude<RecordingAttendanceStatus, 'guest'>;
}

export interface RecordingGuestDraft {
  personId: string;
  displayName: string;
  aliases: string[];
  department: string | null;
  role: string | null;
}

export interface RecordingTermDraft {
  termId: string;
  canonical: string;
  aliases: string[];
  category: string | null;
}

export interface RecordingMeetingContextDraft {
  expectedProfileSha256: string;
  attendance: RecordingPersonAttendanceOverride[];
  hostPersonId: string | null;
  guests: RecordingGuestDraft[];
  additionalTerms: RecordingTermDraft[];
}

export interface PreparedRecordingMetadata {
  templateSelection: RecordingTemplateSelection | null;
  meetingContextDraft: RecordingMeetingContextDraft | null;
}

export interface TemplateFieldIssue {
  code: string;
  path: string;
  messageKey: string;
  params?: Record<string, string | number | boolean | null>;
}

export interface TemplateValidationResult {
  valid: boolean;
  errors: TemplateFieldIssue[];
  warnings: TemplateFieldIssue[];
  normalized: TemplateV2 | null;
}

export interface TemplateApiError {
  code: string;
  messageKey: string;
  params?: Record<string, string | number | boolean | null>;
  fieldErrors?: TemplateFieldIssue[];
  retryable: boolean;
  debugId: string;
}

export interface TemplateListItem {
  id: string;
  name: string;
  description: string;
  origin: TemplateOrigin;
  schemaVersion: 1 | 2;
  version: number;
  locale: string | null;
  tags: string[];
  sectionCount: number;
  sourceType: TemplateSourceType | null;
  updatedAt: string | null;
  fileSha256: string;
  semanticSha256: string | null;
  isDefault: boolean;
  readOnly: boolean;
  overridesBuiltin: boolean;
  valid: boolean;
  validationSummary: {
    errorCount: number;
    warningCount: number;
  };
}

export interface TemplateDetails {
  template: TemplateV2;
  origin: TemplateOrigin;
  schemaVersionOnDisk: 1 | 2;
  fileSha256: string;
  semanticSha256: string;
  isDefault: boolean;
  overridesBuiltin: boolean;
  readOnly: boolean;
}

export type TemplateListOrigin = TemplateOrigin | 'all';
export type TemplateValidationMode = 'create' | 'update' | 'import' | 'preview';
export type CreateTemplateConflictPolicy = 'error' | 'keep_both' | 'override_builtin';
export type RestoreTemplateConflictPolicy = 'error' | 'keep_both' | 'replace_custom';

export interface TemplateRepositoryDiagnostic {
  fileName: string;
  code: string;
  messageKey: string;
}

export interface DeletedTemplateListItem {
  trashId: string;
  originalTemplateId: string;
  deletedAt: string;
  name: string;
  fileSha256: string;
  valid: boolean;
  errorCode: string | null;
}

export interface TemplatesDirectoryInfo {
  path: string;
  exists: boolean;
  writable: boolean;
  customTemplateCount: number;
}

export interface ListTemplatesRequest {
  origin?: TemplateListOrigin;
  includeInvalid?: boolean;
  includeTrash?: boolean;
  query?: string;
  /** Display locale for built-in content only; it never changes generation language. */
  contentLocale?: string;
}

export interface ListTemplatesResponse {
  templates: TemplateListItem[];
  diagnostics: TemplateRepositoryDiagnostic[];
  deletedTemplates: DeletedTemplateListItem[];
  defaultTemplateId: string | null;
}

export interface GetTemplateRequest {
  templateId: string;
  origin?: TemplateOrigin;
  contentLocale?: string;
}

export interface CreateTemplateRequest {
  template: TemplateV2;
  conflictPolicy: CreateTemplateConflictPolicy;
}

export interface UpdateTemplateRequest {
  templateId: string;
  expectedVersion: number;
  expectedFileSha256: string;
  template: TemplateV2;
}

export interface DuplicateTemplateRequest {
  templateId: string;
  origin?: TemplateOrigin;
  newName: string;
  requestedId?: string;
}

export interface TemplateUsage {
  currentMeetingPreferenceCount: number;
  historicalSnapshotCount: number;
  isDefault: boolean;
}

export interface DeleteTemplateRequest {
  templateId: string;
  expectedFileSha256: string;
  replacementDefaultTemplateId?: string | null;
}

export interface DeleteTemplateResponse {
  trashId: string;
  deletedAt: string;
  fileSha256: string;
  usage: TemplateUsage;
}

export interface RestoreTemplateRequest {
  trashId: string;
  conflictPolicy: RestoreTemplateConflictPolicy;
}

export interface DefaultTemplatePreference {
  templateId: string | null;
  resolvedTemplateId: string;
  resolutionSource: 'user_default' | 'builtin_fallback';
}

export type MeetingTemplateMode = 'inherit' | 'meeting_override';
export type MeetingTemplateStorage = 'metadata' | 'local_fallback';
export type MeetingTemplateResolutionSource =
  | 'meeting_override'
  | 'global_default'
  | 'builtin_fallback';

export interface MeetingTemplatePreference {
  schemaVersion: 1;
  mode: MeetingTemplateMode;
  templateId: string | null;
  templateVersion: number | null;
  templateFileSha256: string | null;
  selectedAt: string;
}

export interface ResolvedMeetingTemplate {
  templateId: string;
  name: string;
  version: number;
  fileSha256: string;
  origin: TemplateOrigin;
  source: MeetingTemplateResolutionSource;
}

export interface MeetingTemplateIssue {
  code: 'SELECTED_TEMPLATE_MISSING' | 'SELECTED_TEMPLATE_INVALID';
  selectedTemplateId: string;
}

export interface MeetingTemplatePreferenceResponse {
  preference: MeetingTemplatePreference;
  storage: MeetingTemplateStorage;
  resolved: ResolvedMeetingTemplate;
  issue?: MeetingTemplateIssue;
}

export interface SaveMeetingTemplatePreferenceRequest {
  meetingId: string;
  preference: {
    mode: MeetingTemplateMode;
    templateId: string | null;
  };
}

export type DocumentImportConfidence = 'high' | 'medium' | 'low';
export type DocumentOutlineKind = 'heading' | 'paragraph' | 'list_item' | 'table';

export interface DocumentOutlineNode {
  kind: DocumentOutlineKind;
  level: number | null;
  text: string;
  rows?: string[][];
}

export interface DocumentImportWarning {
  code: string;
  messageKey: string;
  params?: Record<string, string | number | boolean | null>;
}

export interface DocumentImportPreview {
  importId: string;
  fileName: string;
  sourceType: Extract<TemplateSourceType, 'json_import' | 'docx_import' | 'doc_import'>;
  fileSha256: string;
  confidence: DocumentImportConfidence;
  outline: DocumentOutlineNode[];
  warnings: DocumentImportWarning[];
  draft: TemplateV2;
}

export type TemplateImportJobStatus =
  | 'accepted'
  | 'running'
  | 'cancel_requested'
  | 'cancelled'
  | 'completed';

export type TemplateImportItemStatus =
  | 'pending'
  | 'running'
  | 'cancel_requested'
  | 'cancelled'
  | 'completed'
  | 'failed';

export interface TemplateImportJobItem {
  itemId: string;
  status: TemplateImportItemStatus;
}

export interface TemplateImportJob {
  jobId: string;
  status: TemplateImportJobStatus;
  items: TemplateImportJobItem[];
}

export interface PreviewTemplateDocumentItem {
  itemId: string;
  fileName: string;
  /** Present on the cancellable v2 job API; omitted by older/mock transports. */
  status?: TemplateImportItemStatus;
  preview?: DocumentImportPreview;
  error?: TemplateApiError;
}

export interface PreviewTemplateDocumentsResponse {
  jobId?: string;
  /** Present on the cancellable v2 job API; omitted by older/mock transports. */
  status?: TemplateImportJobStatus;
  items: PreviewTemplateDocumentItem[];
}

export interface ExportTemplateJsonRequest {
  templateId: string;
  origin?: TemplateOrigin;
  contentLocale?: string;
  destinationPath: string;
}

export interface ExportTemplateJsonResponse {
  fileName: string;
  bytes: number;
  fileSha256: string;
}

export interface PortablePackTemplateFingerprint {
  id: string;
  version: number;
  fileSha256: string;
  semanticSha256: string;
}

export interface PortablePackExportTemplate extends PortablePackTemplateFingerprint {
  name: string;
  byteSize: number;
  overridesBuiltin: boolean;
}

export interface PortablePackWarning {
  code: string;
  messageKey: string;
}

export interface PreviewTemplatePackExportRequest {
  templateIds: string[];
}

export interface PreviewTemplatePackExportResponse {
  planToken: string;
  templates: PortablePackExportTemplate[];
  templateCount: number;
  estimatedUncompressedBytes: number;
  warnings: PortablePackWarning[];
}

export interface ExportTemplatePackRequest {
  planToken: string;
  destinationPath: string;
  overwrite: boolean;
}

export interface PortablePackAuditSummary {
  packageId: string;
  packageSchemaVersion: 1;
  packageFileName: string;
  archiveSha256: string;
  templateCount: number;
  createdAt: string;
  applicationVersion: string;
}

export interface ExportTemplatePackResponse {
  package: PortablePackAuditSummary;
  byteSize: number;
  exportedTemplates: PortablePackTemplateFingerprint[];
}

export type PortablePackConflictKind =
  | 'none'
  | 'custom'
  | 'readonly'
  | 'duplicate_in_package';

export type PortablePackConflictStrategy =
  | 'skip'
  | 'keep_both'
  | 'replace_custom';

export interface PreviewTemplatePackImportRequest {
  sourcePath: string;
}

export interface PortablePackImportTemplate extends PortablePackTemplateFingerprint {
  name: string;
  byteSize: number;
}

export interface PortablePackImportItem {
  itemId: string;
  template: PortablePackImportTemplate;
  conflictKind: PortablePackConflictKind;
  allowedStrategies: PortablePackConflictStrategy[];
  existing?: PortablePackTemplateFingerprint;
}

export interface PreviewTemplatePackImportResponse {
  planToken: string;
  package: PortablePackAuditSummary;
  items: PortablePackImportItem[];
  totalUncompressedBytes: number;
  warnings: PortablePackWarning[];
}

export interface PortablePackImportDecision {
  itemId: string;
  strategy: PortablePackConflictStrategy;
}

export interface PlanTemplatePackImportRequest {
  previewPlanToken: string;
  decisions: PortablePackImportDecision[];
}

export type PortablePackImportOperationKind =
  | 'create'
  | 'skip'
  | 'keep_both'
  | 'replace_custom';

export interface PortablePackPlannedImportOperation {
  itemId: string;
  operation: PortablePackImportOperationKind;
  source: PortablePackImportTemplate;
  target?: PortablePackImportTemplate;
  expectedExisting?: PortablePackTemplateFingerprint;
}

export interface PortablePackImportPlanSummary {
  createCount: number;
  replaceCount: number;
  skipCount: number;
  transformedCount: number;
}

export interface PlanTemplatePackImportResponse {
  executionPlanToken: string;
  expiresAt: string;
  package: PortablePackAuditSummary;
  operations: PortablePackPlannedImportOperation[];
  summary: PortablePackImportPlanSummary;
  warnings: PortablePackWarning[];
}

export interface ExecuteTemplatePackImportRequest {
  executionPlanToken: string;
  executionId: string;
}

export interface CancelTemplatePackImportRequest {
  executionId: string;
}

export interface PortablePackImportExecutionItem {
  itemId: string;
  operation: PortablePackImportOperationKind;
  target?: PortablePackImportTemplate;
}

export interface PortablePackRecoverySummary {
  rolledBackTransactions: number;
  finalizedTransactions: number;
  cleanedStagingDirectories: number;
}

export interface ExecuteTemplatePackImportResponse {
  executionId: string;
  package: PortablePackAuditSummary;
  summary: PortablePackImportPlanSummary;
  results: PortablePackImportExecutionItem[];
  recovery: PortablePackRecoverySummary;
}

export interface CancelTemplatePackImportResponse {
  executionId: string;
  cancellationRequested: boolean;
}

export type SummaryGenerationStatus =
  | 'pending'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'superseded'
  | 'legacy';

export type SummarySnapshotState = 'available' | 'missing' | 'corrupt' | 'quarantined';

export interface SummaryGenerationHistoryItem {
  generationId: string;
  status: SummaryGenerationStatus;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  templateId: string;
  templateVersion: number;
  resolutionSource: MeetingTemplateResolutionSource | 'historical_snapshot';
  modelProvider: string;
  modelName: string;
  summaryLanguage: string | null;
  sourceBindingSchemaVersion: number | null;
  transcriptSource: 'whisper' | 'sensevoice' | 'parakeet' | 'moss' | 'manual' | null;
  transcriptVersionId: string | null;
  transcriptVersion: number | null;
  mossRunId: string | null;
  transcriptActivatedAt: string | null;
  transcriptSha256: string | null;
  speakerBindingSnapshotId: string | null;
  speakerBindingVersion: number | null;
  speakerBindingSha256: string | null;
  errorCategory: string | null;
  snapshotState: SummarySnapshotState;
  isCurrentSummary: boolean;
  isActiveGeneration: boolean;
  canRetryWithSnapshot: boolean;
}

export interface ManualSummaryRevision {
  revisionId: string;
  createdAt: string;
  sourceGenerationId: string | null;
  markdown: string | null;
  summary: Record<string, unknown>;
  isCurrent: boolean;
}

export interface SummaryGenerationSnapshotDetails {
  generationId: string;
  capturedAt: string;
  template: TemplateV2;
  fileSha256: string;
  semanticSha256: string;
  summaryLanguage: string | null;
  modelProvider: string;
  modelName: string;
  meetingContextId: string | null;
  meetingContextSha256: string | null;
  summaryContextSha256: string | null;
  summarySourceBinding: SummarySourceBinding | null;
}

export interface SnapshotRetentionPolicy {
  retainLatest: number;
  retainDays: number;
  maxTotalBytes: number;
}

export interface SnapshotInventoryItem {
  generationId: string;
  byteSize: number;
  capturedAt: string;
  fileState: Extract<SummarySnapshotState, 'available' | 'corrupt'>;
  historyStatus: SummaryGenerationStatus | null;
  templateId: string | null;
  templateVersion: number | null;
  protectedReasons: string[];
  cleanupReasons: string[];
  cleanupCandidate: boolean;
}

export interface SnapshotCleanupPreview {
  meetingId: string;
  policy: SnapshotRetentionPolicy;
  previewToken: string;
  totalFileCount: number;
  totalBytes: number;
  candidateFileCount: number;
  candidateBytes: number;
  protectedFileCount: number;
  corruptFileCount: number;
  orphanFileCount: number;
  items: SnapshotInventoryItem[];
}

export interface SnapshotCleanupResult {
  cleanupBatchId: string;
  quarantinedGenerationIds: string[];
  quarantinedFileCount: number;
  quarantinedBytes: number;
  skippedGenerationIds: string[];
  planChanged: boolean;
}
