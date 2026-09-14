import assert from 'node:assert/strict';
import test from 'node:test';

import {
  allowedImportConflictPolicies,
  applyImportConflictPolicyToGroup,
  buildImportConflictPlan,
  clearCancelledImportValidation,
  filterSupportedImportPaths,
  hasUnfinishedImportItems,
  importDraftIssues,
  isImportDraftDirty,
  isImportItemSaveable,
  mapDocumentPreviewResult,
  reconcileImportConflictPolicies,
  replaceImportItemDraft,
  resetImportItemDraft,
  selectedFileName,
} from '../../src/components/templates/TemplateImportDialog';
import { editorDraftToTemplate } from '../../src/lib/template-editor';
import { shouldDelegateDropToTemplateImport } from '../../src/lib/file-drop-routing';
import type { DocumentImportPreview, TemplateListItem } from '../../src/types/summary-template';

const preview: DocumentImportPreview = {
  importId: 'import-1',
  fileName: '客户访谈.docx',
  sourceType: 'docx_import',
  fileSha256: 'a'.repeat(64),
  confidence: 'high',
  outline: [],
  warnings: [],
  draft: {
    schemaVersion: 2,
    id: 'customer_interview',
    name: '客户访谈',
    description: 'Imported locally',
    version: 1,
    locale: 'zh-CN',
    tags: ['imported'],
    source: {
      type: 'docx_import',
      originalFileName: '客户访谈.docx',
      originalFileSha256: 'a'.repeat(64),
      importedAt: '2026-08-23T00:00:00Z',
      copiedFromTemplateId: null,
    },
    createdAt: '2026-08-23T00:00:00Z',
    updatedAt: '2026-08-23T00:00:00Z',
    sections: [{
      id: 'summary',
      title: '总结',
      instruction: '提取结论',
      format: 'paragraph',
      itemFormat: null,
      exampleItemFormat: null,
      required: true,
      emptyBehavior: 'show_not_mentioned',
    }],
    extensions: {},
  },
};

function listedTemplate(id: string, origin: TemplateListItem['origin']): TemplateListItem {
  return {
    id,
    name: id,
    description: '',
    origin,
    schemaVersion: 2,
    version: 1,
    locale: null,
    tags: [],
    sectionCount: 1,
    sourceType: null,
    updatedAt: null,
    fileSha256: 'b'.repeat(64),
    semanticSha256: 'c'.repeat(64),
    isDefault: false,
    readOnly: origin !== 'custom',
    overridesBuiltin: false,
    valid: true,
    validationSummary: { errorCount: 0, warningCount: 0 },
  };
}

function readyItem(clientId: string, id: string) {
  const item = mapDocumentPreviewResult({ itemId: clientId, fileName: `${id}.json`, preview });
  const edited = replaceImportItemDraft(item, { ...item.editorDraft!, id });
  return {
    ...edited,
    validation: {
      valid: true,
      errors: [],
      warnings: [],
      normalized: editorDraftToTemplate(edited.editorDraft!),
    },
  };
}

test('shows only the selected basename for Windows and POSIX paths', () => {
  assert.equal(selectedFileName('C:\\Users\\private\\客户访谈.docx'), '客户访谈.docx');
  assert.equal(selectedFileName('/home/private/weekly.docx'), 'weekly.docx');
});

test('accepts JSON and Word imports case-insensitively while removing duplicate paths', () => {
  assert.deepEqual(filterSupportedImportPaths([
    'C:\\templates\\weekly.JSON',
    'C:\\templates\\weekly.json',
    '/tmp/interview.docx',
    '/tmp/legacy.DOC',
    '/tmp/readme.txt',
  ]), [
    'C:\\templates\\weekly.JSON',
    '/tmp/interview.docx',
    '/tmp/legacy.DOC',
  ]);
});

test('routes template files away from the global audio-drop handler only on the template library', () => {
  assert.equal(shouldDelegateDropToTemplateImport('/settings/templates', ['C:\\templates\\weekly.JSON']), true);
  assert.equal(shouldDelegateDropToTemplateImport('/settings/templates', ['/tmp/agenda.docx', '/tmp/call.mp3']), true);
  assert.equal(shouldDelegateDropToTemplateImport('/settings/templates', ['/tmp/call.mp3']), false);
  assert.equal(shouldDelegateDropToTemplateImport('/meeting-details', ['/tmp/agenda.docx']), false);
  assert.equal(shouldDelegateDropToTemplateImport('/settings/templates/editor', ['/tmp/agenda.doc']), false);
});

test('warns before closing only while import work remains unfinished', () => {
  assert.equal(hasUnfinishedImportItems(['ready', 'failed']), true);
  assert.equal(hasUnfinishedImportItems(['processing']), true);
  assert.equal(hasUnfinishedImportItems(['cancelling']), true);
  assert.equal(hasUnfinishedImportItems(['saving', 'saved']), true);
  assert.equal(hasUnfinishedImportItems(['saved', 'skipped', 'cancelled']), false);
});

test('maps each successful preview independently into a review-required queue item', () => {
  const item = mapDocumentPreviewResult({
    itemId: 'queue-1',
    fileName: '客户访谈.docx',
    preview,
  });
  assert.equal(item.status, 'ready');
  assert.equal(item.preview?.draft.schemaVersion, 2);
  assert.equal(item.editorDraft?.name, '客户访谈');
  assert.equal(item.reviewConfirmed, true);
  assert.equal(isImportItemSaveable(item), true);
  assert.equal(item.error, undefined);
});

test('keeps per-file edits isolated and can restore the exact imported baseline', () => {
  const first = mapDocumentPreviewResult({ itemId: 'queue-a', fileName: preview.fileName, preview });
  const second = mapDocumentPreviewResult({ itemId: 'queue-b', fileName: preview.fileName, preview });
  assert.ok(first.editorDraft);
  const edited = replaceImportItemDraft(first, {
    ...first.editorDraft,
    id: 'customer_interview_cn',
    name: '客户访谈（中文）',
    sections: first.editorDraft.sections.map((section, index) => index === 0
      ? { ...section, instruction: '提取结论、负责人和截止时间' }
      : section),
  });

  assert.equal(isImportDraftDirty(edited), true);
  assert.equal(edited.draftRevision, 1);
  assert.equal(editorDraftToTemplate(edited.editorDraft!).id, 'customer_interview_cn');
  assert.equal(second.editorDraft?.id, 'customer_interview');
  assert.equal(second.preview?.draft.name, '客户访谈');

  const restored = resetImportItemDraft(edited);
  assert.equal(isImportDraftDirty(restored), false);
  assert.deepEqual(editorDraftToTemplate(restored.editorDraft!), preview.draft);
  assert.equal(restored.draftRevision, 2);
});

test('blocks saving while local or native validation is incomplete or invalid', () => {
  const item = mapDocumentPreviewResult({ itemId: 'queue-validation', fileName: preview.fileName, preview });
  assert.ok(item.editorDraft);
  const invalid = replaceImportItemDraft(item, { ...item.editorDraft, id: 'X' });
  assert.equal(importDraftIssues(invalid).some((issue) => issue.path === '/id'), true);
  assert.equal(isImportItemSaveable(invalid), false);

  const nativeInvalid = {
    ...item,
    validation: {
      valid: false,
      errors: [{ code: 'SCHEMA_VIOLATION', path: '/name', messageKey: 'templates.validation.schemaViolation' }],
      warnings: [],
      normalized: null,
    },
  };
  assert.equal(isImportItemSaveable(nativeInvalid), false);
});

test('requires explicit confirmation before a low-confidence Word draft can be saved', () => {
  const lowConfidencePreview: DocumentImportPreview = { ...preview, confidence: 'low' };
  const item = mapDocumentPreviewResult({
    itemId: 'queue-low-confidence',
    fileName: lowConfidencePreview.fileName,
    preview: lowConfidencePreview,
  });
  assert.equal(item.reviewConfirmed, false);
  assert.equal(isImportItemSaveable(item), false);
  assert.equal(isImportItemSaveable({ ...item, reviewConfirmed: true }), true);
});

test('classifies custom, read-only, and later in-batch ID conflicts before any save starts', () => {
  const items = [
    readyItem('custom-1', 'existing_custom'),
    readyItem('readonly-1', 'standard_meeting'),
    readyItem('batch-primary', 'duplicate_in_batch'),
    readyItem('batch-later', 'duplicate_in_batch'),
  ];
  const plan = buildImportConflictPlan(items, [
    listedTemplate('existing_custom', 'custom'),
    listedTemplate('standard_meeting', 'builtin'),
  ]);

  assert.equal(plan['custom-1'].group, 'custom');
  assert.equal(plan['readonly-1'].group, 'read_only');
  assert.equal(plan['batch-primary'], undefined);
  assert.deepEqual(plan['batch-later'], {
    group: 'batch',
    templateId: 'duplicate_in_batch',
    primaryClientId: 'batch-primary',
    existingOrigin: undefined,
  });
});

test('requires an allowed per-item conflict decision and never reuses an incompatible policy', () => {
  const item = readyItem('custom-policy', 'existing_custom');
  const plan = buildImportConflictPlan([item], [listedTemplate('existing_custom', 'custom')]);
  const conflict = plan[item.clientId];

  assert.deepEqual(allowedImportConflictPolicies(conflict), ['keep_both', 'skip', 'replace_custom']);
  assert.equal(isImportItemSaveable(item, conflict), false);
  assert.equal(isImportItemSaveable({ ...item, conflictPolicy: 'replace_custom' }, conflict), true);
  assert.equal(isImportItemSaveable({ ...item, conflictPolicy: 'override_builtin' }, conflict), false);

  const reconciled = reconcileImportConflictPolicies(
    [{ ...item, conflictPolicy: 'override_builtin' }],
    plan,
  );
  assert.equal(reconciled[0].conflictPolicy, undefined);
});

test('applies a strategy only to conflicts of the same type', () => {
  const customA = readyItem('custom-a', 'custom_a');
  const customB = readyItem('custom-b', 'custom_b');
  const readOnly = readyItem('readonly-a', 'standard_meeting');
  const items = [customA, customB, readOnly];
  const plan = buildImportConflictPlan(items, [
    listedTemplate('custom_a', 'custom'),
    listedTemplate('custom_b', 'custom'),
    listedTemplate('standard_meeting', 'builtin'),
  ]);

  const applied = applyImportConflictPolicyToGroup(items, plan, 'custom', 'replace_custom');
  assert.equal(applied[0].conflictPolicy, 'replace_custom');
  assert.equal(applied[1].conflictPolicy, 'replace_custom');
  assert.equal(applied[2].conflictPolicy, undefined);

  const incompatible = applyImportConflictPolicyToGroup(items, plan, 'custom', 'override_builtin');
  assert.equal(incompatible.every((item) => item.conflictPolicy === undefined), true);
});

test('clears a cancelled selected-item validation without touching another revision', () => {
  const item = readyItem('validation-switch', 'validation_switch');
  const pending = { ...item, validation: undefined, validating: true, draftRevision: 7 };
  assert.equal(clearCancelledImportValidation(pending, item.clientId, 7).validating, false);
  assert.equal(clearCancelledImportValidation(pending, item.clientId, 6).validating, true);
  assert.equal(clearCancelledImportValidation({ ...pending, validation: item.validation }, item.clientId, 7).validating, false);
});

test('preserves structured per-file errors without retaining a selected full path', () => {
  const item = mapDocumentPreviewResult({
    itemId: 'queue-2',
    fileName: 'damaged.docx',
    error: {
      code: 'TEMPLATE_DOCX_INVALID',
      messageKey: 'templates.errors.docxInvalid',
      retryable: false,
      debugId: 'debug-2',
    },
  });
  assert.equal(item.status, 'failed');
  assert.equal(item.fileName, 'damaged.docx');
  assert.equal(item.error?.code, 'TEMPLATE_DOCX_INVALID');
  assert.equal(item.editorDraft, undefined);
  assert.doesNotMatch(JSON.stringify(item), /Users|private/i);
});

test('maps a backend-confirmed cancellation to a terminal item without exposing its error panel', () => {
  const item = mapDocumentPreviewResult({
    itemId: 'cancelled-item',
    fileName: 'large-template.docx',
    status: 'cancelled',
    error: {
      code: 'TEMPLATE_CANCELLED',
      messageKey: 'templates.errors.cancelled',
      retryable: false,
      debugId: 'cancelled-debug',
    },
  });
  assert.equal(item.status, 'cancelled');
  assert.equal(item.error, undefined);
  assert.equal(item.preview, undefined);
  assert.equal(hasUnfinishedImportItems([item.status]), false);
});
