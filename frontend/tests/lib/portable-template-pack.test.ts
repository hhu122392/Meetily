import assert from 'node:assert/strict';
import test from 'node:test';

import { templateTranslationKey } from '../../src/lib/template-library';
import {
  applyPortableStrategyToConflictKind,
  buildPortableImportDecisions,
  createPortableExecutionId,
  ensurePortablePackExtension,
  formatPortableBytes,
  portableConflictCounts,
  portableExportCandidates,
  portablePackFileName,
  reconcilePortableImportDecisions,
  shouldRestartPortablePreview,
  unresolvedPortableImportItems,
} from '../../src/lib/portable-template-pack';
import type {
  PortablePackImportItem,
  PreviewTemplatePackImportResponse,
  TemplateListItem,
} from '../../src/types/summary-template';

function listedTemplate(
  id: string,
  origin: TemplateListItem['origin'],
  options: Partial<TemplateListItem> = {},
): TemplateListItem {
  return {
    id,
    name: options.name ?? id,
    description: '',
    origin,
    schemaVersion: 2,
    version: 1,
    locale: null,
    tags: [],
    sectionCount: 1,
    sourceType: null,
    updatedAt: null,
    fileSha256: 'a'.repeat(64),
    semanticSha256: 'b'.repeat(64),
    isDefault: false,
    readOnly: origin !== 'custom',
    overridesBuiltin: false,
    valid: true,
    validationSummary: { errorCount: 0, warningCount: 0 },
    ...options,
  };
}

function importItem(
  itemId: string,
  conflictKind: PortablePackImportItem['conflictKind'],
): PortablePackImportItem {
  const allowedStrategies = conflictKind === 'custom'
    ? ['skip', 'keep_both', 'replace_custom'] as const
    : conflictKind === 'readonly' || conflictKind === 'duplicate_in_package'
      ? ['skip', 'keep_both'] as const
      : [] as const;
  return {
    itemId,
    template: {
      id: `template_${itemId}`,
      name: `Template ${itemId}`,
      version: 1,
      byteSize: 128,
      fileSha256: 'c'.repeat(64),
      semanticSha256: 'd'.repeat(64),
    },
    conflictKind,
    allowedStrategies: [...allowedStrategies],
    existing: conflictKind === 'none' ? undefined : {
      id: `template_${itemId}`,
      version: 2,
      fileSha256: 'e'.repeat(64),
      semanticSha256: 'f'.repeat(64),
    },
  };
}

const preview: PreviewTemplatePackImportResponse = {
  planToken: '123e4567-e89b-42d3-a456-426614174000',
  package: {
    packageId: 'portable-package',
    packageSchemaVersion: 1,
    packageFileName: 'portable.meetily-template-pack',
    archiveSha256: '0'.repeat(64),
    templateCount: 4,
    createdAt: '2026-08-24T00:00:00Z',
    applicationVersion: '0.4.0',
  },
  items: [
    importItem('none', 'none'),
    importItem('custom', 'custom'),
    importItem('readonly', 'readonly'),
    importItem('duplicate', 'duplicate_in_package'),
  ],
  totalUncompressedBytes: 512,
  warnings: [],
};

test('shows only a portable package basename and never a local source path', () => {
  assert.equal(
    portablePackFileName('C:\\Users\\private\\团队模板.meetily-template-pack'),
    '团队模板.meetily-template-pack',
  );
  assert.equal(
    portablePackFileName('/home/private/team.meetily-template-pack'),
    'team.meetily-template-pack',
  );
  assert.doesNotMatch(portablePackFileName('C:\\Users\\private\\team.meetily-template-pack'), /Users|private/i);
});

test('adds the portable extension exactly once, case-insensitively', () => {
  assert.equal(
    ensurePortablePackExtension('D:\\exports\\team'),
    'D:\\exports\\team.meetily-template-pack',
  );
  assert.equal(
    ensurePortablePackExtension('D:\\exports\\team.MEETILY-TEMPLATE-PACK'),
    'D:\\exports\\team.MEETILY-TEMPLATE-PACK',
  );
});

test('exports only valid writable custom templates in stable name and ID order', () => {
  const candidates = portableExportCandidates([
    listedTemplate('builtin', 'builtin'),
    listedTemplate('invalid', 'custom', { valid: false }),
    listedTemplate('readonly_custom', 'custom', { readOnly: true }),
    listedTemplate('zeta', 'custom', { name: 'B' }),
    listedTemplate('alpha_2', 'custom', { name: 'A' }),
    listedTemplate('alpha_1', 'custom', { name: 'A' }),
  ]);
  assert.deepEqual(candidates.map((item) => item.id), ['alpha_1', 'alpha_2', 'zeta']);
});

test('requires an explicit compatible decision for every conflict and none for auto-create', () => {
  assert.deepEqual(
    unresolvedPortableImportItems(preview.items, {}).map((item) => item.itemId),
    ['custom', 'readonly', 'duplicate'],
  );
  const decisions = {
    none: 'skip',
    custom: 'replace_custom',
    readonly: 'keep_both',
    duplicate: 'skip',
  } as const;
  assert.deepEqual(unresolvedPortableImportItems(preview.items, decisions), []);
  assert.deepEqual(buildPortableImportDecisions(preview.items, decisions), [
    { itemId: 'custom', strategy: 'replace_custom' },
    { itemId: 'readonly', strategy: 'keep_both' },
    { itemId: 'duplicate', strategy: 'skip' },
  ]);
});

test('drops stale or incompatible decisions after a repeated preview', () => {
  assert.deepEqual(reconcilePortableImportDecisions(preview.items, {
    custom: 'replace_custom',
    readonly: 'replace_custom',
    missing: 'skip',
  }), {
    custom: 'replace_custom',
  });
});

test('bulk application never crosses conflict kinds or backend allowed strategies', () => {
  const applied = applyPortableStrategyToConflictKind(preview.items, {}, 'custom', 'replace_custom');
  assert.deepEqual(applied, { custom: 'replace_custom' });
  const incompatible = applyPortableStrategyToConflictKind(
    preview.items,
    applied,
    'readonly',
    'replace_custom',
  );
  assert.deepEqual(incompatible, { custom: 'replace_custom' });
  const skippedReadonly = applyPortableStrategyToConflictKind(
    preview.items,
    incompatible,
    'readonly',
    'skip',
  );
  assert.deepEqual(skippedReadonly, { custom: 'replace_custom', readonly: 'skip' });
});

test('counts every import classification without folding read-only into custom', () => {
  assert.deepEqual(portableConflictCounts(preview), {
    none: 1,
    custom: 1,
    readonly: 1,
    duplicate_in_package: 1,
  });
});

test('forces a fresh preview after stale, consumed, expired or conflict-change errors', () => {
  for (const code of [
    'TEMPLATE_PACK_PLAN_STALE',
    'TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED',
    'TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED',
    'TEMPLATE_PACK_CONFLICT_CHANGED',
    'TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED',
    'TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED',
  ]) {
    assert.equal(shouldRestartPortablePreview(code), true, code);
  }
  assert.equal(shouldRestartPortablePreview('TEMPLATE_PACK_INVALID'), false);
});

test('accepts only a UUID-shaped execution ID so duplicate execution IDs are never synthesized', () => {
  assert.equal(
    createPortableExecutionId(() => '123e4567-e89b-42d3-a456-426614174000'),
    '123e4567-e89b-42d3-a456-426614174000',
  );
  assert.throws(() => createPortableExecutionId(() => 'execution-1'), /valid UUID/);
});

test('formats package byte totals without leaking paths or using translated text in control flow', () => {
  assert.equal(formatPortableBytes(0, 'en-US'), '0 B');
  assert.equal(formatPortableBytes(1536, 'en-US'), '1.5 KiB');
  assert.equal(formatPortableBytes(1024 * 1024, 'zh-CN'), '1 MiB');
});

test('maps every portable backend error through the controlled translation whitelist', () => {
  const mappings = {
    'templates.errors.packInvalid': 'errors.packInvalid',
    'templates.errors.packPlanStale': 'errors.packPlanStale',
    'templates.errors.packConflictChanged': 'errors.packConflictChanged',
    'templates.errors.packExecutionPlanExpired': 'errors.packExecutionPlanExpired',
    'templates.errors.packExecutionAlreadyActive': 'errors.packExecutionAlreadyActive',
    'templates.errors.packRecoveryFailed': 'errors.packRecoveryFailed',
  } as const;
  for (const [messageKey, expected] of Object.entries(mappings)) {
    assert.equal(templateTranslationKey(messageKey), expected);
  }
  assert.equal(templateTranslationKey('templates.portable.injected.runtime.key'), 'errors.io');
});
