import type {
  DeletedTemplateListItem,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';

export type TemplateLibraryView = 'all' | 'custom' | 'builtin' | 'trash';

export interface TemplateLibraryCounts {
  all: number;
  custom: number;
  builtin: number;
  trash: number;
}

const normalizeSearchText = (value: string) => value.trim().toLocaleLowerCase();

export function filterTemplates(
  templates: readonly TemplateListItem[],
  view: Exclude<TemplateLibraryView, 'trash'>,
  query: string,
): TemplateListItem[] {
  const normalizedQuery = normalizeSearchText(query);

  return templates.filter((template) => {
    const originMatches =
      view === 'all' ||
      (view === 'custom' && template.origin === 'custom') ||
      (view === 'builtin' && template.origin !== 'custom');

    if (!originMatches) return false;
    if (!normalizedQuery) return true;

    return [template.name, template.description, template.id, ...template.tags]
      .some((value) => normalizeSearchText(value).includes(normalizedQuery));
  });
}

export function filterDeletedTemplates(
  templates: readonly DeletedTemplateListItem[],
  query: string,
): DeletedTemplateListItem[] {
  const normalizedQuery = normalizeSearchText(query);
  if (!normalizedQuery) return [...templates];

  return templates.filter((template) =>
    [template.name, template.originalTemplateId]
      .some((value) => normalizeSearchText(value).includes(normalizedQuery)),
  );
}

export function countTemplateViews(
  templates: readonly TemplateListItem[],
  deletedTemplates: readonly DeletedTemplateListItem[],
): TemplateLibraryCounts {
  const custom = templates.filter((template) => template.origin === 'custom').length;
  return {
    all: templates.length,
    custom,
    builtin: templates.length - custom,
    trash: deletedTemplates.length,
  };
}

export function shouldApplyTemplateResponse(
  responseSequence: number,
  latestSequence: number,
): boolean {
  return responseSequence === latestSequence;
}

const RUNTIME_TEMPLATE_TRANSLATION_KEYS = {
  'templates.errors.alreadyExists': 'errors.alreadyExists',
  'templates.errors.conflict': 'errors.conflict',
  'templates.errors.directoryNotWritable': 'errors.directoryNotWritable',
  'templates.errors.directoryUnavailable': 'errors.directoryUnavailable',
  'templates.errors.diskFull': 'errors.diskFull',
  'templates.errors.invalidId': 'errors.invalidId',
  'templates.errors.inUse': 'errors.inUse',
  'templates.errors.invalid': 'errors.invalid',
  'templates.errors.isDefault': 'errors.isDefault',
  'templates.errors.notFound': 'errors.notFound',
  'templates.errors.pathRejected': 'errors.pathRejected',
  'templates.errors.readOnly': 'errors.readOnly',
  'templates.errors.io': 'errors.io',
  'templates.errors.snapshotFailed': 'errors.snapshotFailed',
  'templates.errors.archiveTooLarge': 'errors.archiveTooLarge',
  'templates.errors.cancelled': 'errors.cancelled',
  'templates.errors.docConversionFailed': 'errors.docConversionFailed',
  'templates.errors.docConverterMissing': 'errors.docConverterMissing',
  'templates.errors.docxInvalid': 'errors.docxInvalid',
  'templates.errors.importUnsupported': 'errors.importUnsupported',
  'templates.errors.packInvalid': 'errors.packInvalid',
  'templates.errors.packVersionUnsupported': 'errors.packVersionUnsupported',
  'templates.errors.packIntegrityFailed': 'errors.packIntegrityFailed',
  'templates.errors.packBudgetExceeded': 'errors.packBudgetExceeded',
  'templates.errors.packUnsafeEntry': 'errors.packUnsafeEntry',
  'templates.errors.packSensitiveContent': 'errors.packSensitiveContent',
  'templates.errors.packConflictUnresolved': 'errors.packConflictUnresolved',
  'templates.errors.packPlanStale': 'errors.packPlanStale',
  'templates.errors.packPreviewPlanNotFound': 'errors.packPreviewPlanNotFound',
  'templates.errors.packPreviewPlanExpired': 'errors.packPreviewPlanExpired',
  'templates.errors.packPreviewPlanConsumed': 'errors.packPreviewPlanConsumed',
  'templates.errors.packDecisionMissing': 'errors.packDecisionMissing',
  'templates.errors.packDecisionDuplicate': 'errors.packDecisionDuplicate',
  'templates.errors.packDecisionUnknownItem': 'errors.packDecisionUnknownItem',
  'templates.errors.packDecisionNotAllowed': 'errors.packDecisionNotAllowed',
  'templates.errors.packKeepBothIdExhausted': 'errors.packKeepBothIdExhausted',
  'templates.errors.packConflictChanged': 'errors.packConflictChanged',
  'templates.errors.packExportFailed': 'errors.packExportFailed',
  'templates.errors.packImportFailed': 'errors.packImportFailed',
  'templates.errors.packExecutionPlanNotFound': 'errors.packExecutionPlanNotFound',
  'templates.errors.packExecutionPlanExpired': 'errors.packExecutionPlanExpired',
  'templates.errors.packExecutionPlanConsumed': 'errors.packExecutionPlanConsumed',
  'templates.errors.packExecutionNotFound': 'errors.packExecutionNotFound',
  'templates.errors.packExecutionAlreadyActive': 'errors.packExecutionAlreadyActive',
  'templates.errors.packRecoveryFailed': 'errors.packRecoveryFailed',
  'templates.validation.blankValue': 'validation.blankValue',
  'templates.validation.deserializationFailed': 'validation.deserializationFailed',
  'templates.validation.duplicateSectionId': 'validation.duplicateSectionId',
  'templates.validation.invalidJson': 'validation.invalidJson',
  'templates.validation.legacyInvalid': 'validation.legacyInvalid',
  'templates.validation.localeUnset': 'validation.localeUnset',
  'templates.validation.nearSizeLimit': 'validation.nearSizeLimit',
  'templates.validation.schemaUnavailable': 'validation.schemaUnavailable',
  'templates.validation.schemaViolation': 'validation.schemaViolation',
  'templates.validation.serializationFailed': 'validation.serializationFailed',
  'templates.validation.tooLarge': 'validation.tooLarge',
  'templates.validation.transportFieldNaming': 'validation.transportFieldNaming',
  'templates.validation.updatedBeforeCreated': 'validation.updatedBeforeCreated',
  'templates.validation.versionWillBeNormalized': 'validation.versionWillBeNormalized',
  'templates.editor.validation.id': 'editor.validation.id',
  'templates.editor.validation.idImmutable': 'editor.validation.idImmutable',
  'templates.editor.validation.name': 'editor.validation.name',
  'templates.editor.validation.description': 'editor.validation.description',
  'templates.editor.validation.locale': 'editor.validation.locale',
  'templates.editor.validation.tags': 'editor.validation.tags',
  'templates.editor.validation.sections': 'editor.validation.sections',
  'templates.editor.validation.sectionId': 'editor.validation.sectionId',
  'templates.editor.validation.sectionIdDuplicate': 'editor.validation.sectionIdDuplicate',
  'templates.editor.validation.sectionTitle': 'editor.validation.sectionTitle',
  'templates.editor.validation.sectionInstruction': 'editor.validation.sectionInstruction',
  'templates.editor.errors.invalidLocation': 'editor.errors.invalidLocation',
} as const;

export type RuntimeTemplateTranslationKey =
  (typeof RUNTIME_TEMPLATE_TRANSLATION_KEYS)[keyof typeof RUNTIME_TEMPLATE_TRANSLATION_KEYS];

export function templateTranslationKey(messageKey: string): RuntimeTemplateTranslationKey {
  if (messageKey in RUNTIME_TEMPLATE_TRANSLATION_KEYS) {
    return RUNTIME_TEMPLATE_TRANSLATION_KEYS[
      messageKey as keyof typeof RUNTIME_TEMPLATE_TRANSLATION_KEYS
    ];
  }
  return 'errors.io';
}

export function templateErrorToastId(operation: string, error: TemplateApiError): string {
  return `template:${operation}:${error.code}:${error.debugId}`;
}

export function stopTemplateCardActionPropagation(
  event: Pick<Event, 'preventDefault' | 'stopPropagation'>,
): void {
  event.preventDefault();
  event.stopPropagation();
}
