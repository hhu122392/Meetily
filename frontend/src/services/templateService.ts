import { invoke } from '@tauri-apps/api/core';
import type {
  CreateTemplateRequest,
  CancelTemplatePackImportRequest,
  CancelTemplatePackImportResponse,
  DefaultTemplatePreference,
  DeleteTemplateRequest,
  DeleteTemplateResponse,
  DeletedTemplateListItem,
  DuplicateTemplateRequest,
  ExportTemplateJsonRequest,
  ExportTemplateJsonResponse,
  ExportTemplatePackRequest,
  ExportTemplatePackResponse,
  ExecuteTemplatePackImportRequest,
  ExecuteTemplatePackImportResponse,
  GetTemplateRequest,
  ListTemplatesRequest,
  ListTemplatesResponse,
  MeetingTemplatePreferenceResponse,
  PlanTemplatePackImportRequest,
  PlanTemplatePackImportResponse,
  PreviewTemplateDocumentsResponse,
  PreviewTemplatePackExportRequest,
  PreviewTemplatePackExportResponse,
  PreviewTemplatePackImportRequest,
  PreviewTemplatePackImportResponse,
  RestoreTemplateRequest,
  SaveMeetingTemplatePreferenceRequest,
  SnapshotCleanupPreview,
  SnapshotCleanupResult,
  SnapshotRetentionPolicy,
  SummaryGenerationHistoryItem,
  ManualSummaryRevision,
  SummaryGenerationSnapshotDetails,
  TemplateApiError,
  TemplateDetails,
  TemplateImportJob,
  TemplateUsage,
  TemplatesDirectoryInfo,
  TemplateValidationMode,
  TemplateValidationResult,
  UpdateTemplateRequest,
} from '@/types/summary-template';

const FALLBACK_ERROR_CODE = 'TEMPLATE_IO_ERROR';
const FALLBACK_MESSAGE_KEY = 'templates.errors.io';

function isTemplateApiError(value: unknown): value is TemplateApiError {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<TemplateApiError>;
  return (
    typeof candidate.code === 'string' &&
    typeof candidate.messageKey === 'string' &&
    typeof candidate.retryable === 'boolean' &&
    typeof candidate.debugId === 'string'
  );
}

function fallbackDebugId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `template-client-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

export function normalizeTemplateApiError(error: unknown): TemplateApiError {
  if (isTemplateApiError(error)) return error;
  return {
    code: FALLBACK_ERROR_CODE,
    messageKey: FALLBACK_MESSAGE_KEY,
    retryable: true,
    debugId: fallbackDebugId(),
  };
}

async function invokeTemplate<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw normalizeTemplateApiError(error);
  }
}

export class TemplateService {
  getDirectory(): Promise<TemplatesDirectoryInfo> {
    return invokeTemplate('api_get_templates_directory');
  }

  openDirectory(): Promise<void> {
    return invokeTemplate('api_open_templates_directory');
  }

  previewDocuments(paths: string[]): Promise<PreviewTemplateDocumentsResponse> {
    return invokeTemplate('api_preview_template_documents', {
      request: { paths },
    });
  }

  previewImports(
    paths: string[],
    jobId?: string,
    itemIds?: string[],
  ): Promise<PreviewTemplateDocumentsResponse> {
    return invokeTemplate('api_preview_template_imports', {
      request: { paths, jobId, itemIds },
    });
  }

  getImportJob(jobId: string): Promise<TemplateImportJob> {
    return invokeTemplate('api_get_template_import_job', {
      request: { jobId },
    });
  }

  cancelImportJob(jobId: string): Promise<TemplateImportJob> {
    return invokeTemplate('api_cancel_template_import_job', {
      request: { jobId },
    });
  }

  cancelImportItem(jobId: string, itemId: string): Promise<TemplateImportJob> {
    return invokeTemplate('api_cancel_template_import_item', {
      request: { jobId, itemId },
    });
  }

  exportJson(request: ExportTemplateJsonRequest): Promise<ExportTemplateJsonResponse> {
    return invokeTemplate('api_export_template_json', { request });
  }

  previewPackExport(
    request: PreviewTemplatePackExportRequest,
  ): Promise<PreviewTemplatePackExportResponse> {
    return invokeTemplate('api_preview_template_pack_export', { request });
  }

  exportPack(request: ExportTemplatePackRequest): Promise<ExportTemplatePackResponse> {
    return invokeTemplate('api_export_template_pack', { request });
  }

  previewPackImport(
    request: PreviewTemplatePackImportRequest,
  ): Promise<PreviewTemplatePackImportResponse> {
    return invokeTemplate('api_preview_template_pack_import', { request });
  }

  planPackImport(
    request: PlanTemplatePackImportRequest,
  ): Promise<PlanTemplatePackImportResponse> {
    return invokeTemplate('api_plan_template_pack_import', { request });
  }

  executePackImport(
    request: ExecuteTemplatePackImportRequest,
  ): Promise<ExecuteTemplatePackImportResponse> {
    return invokeTemplate('api_execute_template_pack_import', { request });
  }

  cancelPackImport(
    request: CancelTemplatePackImportRequest,
  ): Promise<CancelTemplatePackImportResponse> {
    return invokeTemplate('api_cancel_template_pack_import', { request });
  }

  list(request: ListTemplatesRequest = {}): Promise<ListTemplatesResponse> {
    return invokeTemplate('api_list_templates_v2', { request });
  }

  get(request: GetTemplateRequest): Promise<TemplateDetails> {
    return invokeTemplate('api_get_template_v2', { request });
  }

  validate(template: unknown, mode: TemplateValidationMode): Promise<TemplateValidationResult> {
    return invokeTemplate('api_validate_template_v2', {
      request: { template, mode },
    });
  }

  create(request: CreateTemplateRequest): Promise<TemplateDetails> {
    return invokeTemplate('api_create_template', { request });
  }

  update(request: UpdateTemplateRequest): Promise<TemplateDetails> {
    return invokeTemplate('api_update_template', { request });
  }

  duplicate(request: DuplicateTemplateRequest): Promise<TemplateDetails> {
    return invokeTemplate('api_duplicate_template', { request });
  }

  getUsage(templateId: string): Promise<TemplateUsage> {
    return invokeTemplate('api_get_template_usage', {
      request: { templateId },
    });
  }

  delete(request: DeleteTemplateRequest): Promise<DeleteTemplateResponse> {
    return invokeTemplate('api_delete_template', { request });
  }

  listDeleted(): Promise<DeletedTemplateListItem[]> {
    return invokeTemplate('api_list_deleted_templates');
  }

  restore(request: RestoreTemplateRequest): Promise<TemplateDetails> {
    return invokeTemplate('api_restore_template', { request });
  }

  purge(trashId: string): Promise<void> {
    return invokeTemplate('api_purge_template', {
      request: { trashId },
    });
  }

  getDefault(): Promise<DefaultTemplatePreference> {
    return invokeTemplate('api_get_default_template');
  }

  setDefault(templateId: string | null): Promise<DefaultTemplatePreference> {
    return invokeTemplate('api_set_default_template', {
      request: { templateId },
    });
  }

  getMeetingPreference(meetingId: string): Promise<MeetingTemplatePreferenceResponse> {
    return invokeTemplate('api_get_meeting_template_preference', {
      request: { meetingId },
    });
  }

  saveMeetingPreference(
    request: SaveMeetingTemplatePreferenceRequest,
  ): Promise<MeetingTemplatePreferenceResponse> {
    return invokeTemplate('api_save_meeting_template_preference', { request });
  }

  listSummaryGenerationHistory(meetingId: string): Promise<SummaryGenerationHistoryItem[]> {
    return invokeTemplate('api_list_summary_generation_history', { meetingId });
  }

  listManualSummaryRevisions(meetingId: string): Promise<ManualSummaryRevision[]> {
    return invokeTemplate('api_list_manual_summary_revisions', { meetingId });
  }

  restoreManualSummaryRevision(
    meetingId: string,
    revisionId: string,
  ): Promise<Record<string, unknown>> {
    return invokeTemplate('api_restore_manual_summary_revision', { meetingId, revisionId });
  }

  getSummaryGenerationSnapshot(
    meetingId: string,
    generationId: string,
  ): Promise<SummaryGenerationSnapshotDetails> {
    return invokeTemplate('api_get_summary_generation_snapshot', {
      request: { meetingId, generationId },
    });
  }

  previewSnapshotCleanup(
    meetingId: string,
    policy: SnapshotRetentionPolicy,
  ): Promise<SnapshotCleanupPreview> {
    return invokeTemplate('api_preview_template_snapshot_cleanup', {
      request: { meetingId, policy },
    });
  }

  executeSnapshotCleanup(
    meetingId: string,
    policy: SnapshotRetentionPolicy,
    previewToken: string,
    expectedGenerationIds: string[],
  ): Promise<SnapshotCleanupResult> {
    return invokeTemplate('api_execute_template_snapshot_cleanup', {
      request: { meetingId, policy, previewToken, expectedGenerationIds },
    });
  }
}

export const templateService = new TemplateService();
