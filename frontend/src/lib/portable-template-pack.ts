import type {
  PortablePackConflictKind,
  PortablePackConflictStrategy,
  PortablePackImportDecision,
  PortablePackImportItem,
  PortablePackImportPlanSummary,
  PreviewTemplatePackImportResponse,
  TemplateListItem,
} from '@/types/summary-template';

export type PortableImportDecisionMap = Readonly<Record<string, PortablePackConflictStrategy | undefined>>;

const RESTART_PREVIEW_ERROR_CODES = new Set([
  'TEMPLATE_PACK_PLAN_STALE',
  'TEMPLATE_PACK_PREVIEW_PLAN_NOT_FOUND',
  'TEMPLATE_PACK_PREVIEW_PLAN_EXPIRED',
  'TEMPLATE_PACK_PREVIEW_PLAN_CONSUMED',
  'TEMPLATE_PACK_CONFLICT_CHANGED',
  'TEMPLATE_PACK_EXECUTION_PLAN_NOT_FOUND',
  'TEMPLATE_PACK_EXECUTION_PLAN_EXPIRED',
  'TEMPLATE_PACK_EXECUTION_PLAN_CONSUMED',
]);

export function portablePackFileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) || 'templates.meetily-template-pack';
}

export function ensurePortablePackExtension(path: string): string {
  return path.toLocaleLowerCase().endsWith('.meetily-template-pack')
    ? path
    : `${path}.meetily-template-pack`;
}

export function portableExportCandidates(
  templates: readonly TemplateListItem[],
): TemplateListItem[] {
  return templates
    .filter((template) => template.origin === 'custom' && template.valid && !template.readOnly)
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
}

export function reconcilePortableImportDecisions(
  items: readonly PortablePackImportItem[],
  decisions: PortableImportDecisionMap,
): Record<string, PortablePackConflictStrategy> {
  const next: Record<string, PortablePackConflictStrategy> = {};
  for (const item of items) {
    const decision = decisions[item.itemId];
    if (decision && item.allowedStrategies.includes(decision)) {
      next[item.itemId] = decision;
    }
  }
  return next;
}

export function unresolvedPortableImportItems(
  items: readonly PortablePackImportItem[],
  decisions: PortableImportDecisionMap,
): PortablePackImportItem[] {
  return items.filter((item) => (
    item.conflictKind !== 'none'
    && !item.allowedStrategies.includes(decisions[item.itemId] as PortablePackConflictStrategy)
  ));
}

export function buildPortableImportDecisions(
  items: readonly PortablePackImportItem[],
  decisions: PortableImportDecisionMap,
): PortablePackImportDecision[] {
  return items.flatMap((item) => {
    if (item.conflictKind === 'none') return [];
    const strategy = decisions[item.itemId];
    return strategy && item.allowedStrategies.includes(strategy)
      ? [{ itemId: item.itemId, strategy }]
      : [];
  });
}

export function applyPortableStrategyToConflictKind(
  items: readonly PortablePackImportItem[],
  decisions: PortableImportDecisionMap,
  conflictKind: PortablePackConflictKind,
  strategy: PortablePackConflictStrategy,
): Record<string, PortablePackConflictStrategy> {
  const next = reconcilePortableImportDecisions(items, decisions);
  for (const item of items) {
    if (item.conflictKind === conflictKind && item.allowedStrategies.includes(strategy)) {
      next[item.itemId] = strategy;
    }
  }
  return next;
}

export function portableConflictCounts(
  preview: PreviewTemplatePackImportResponse,
): Record<PortablePackConflictKind, number> {
  return preview.items.reduce<Record<PortablePackConflictKind, number>>((counts, item) => {
    counts[item.conflictKind] += 1;
    return counts;
  }, { none: 0, custom: 0, readonly: 0, duplicate_in_package: 0 });
}

export function emptyPortablePlanSummary(): PortablePackImportPlanSummary {
  return { createCount: 0, replaceCount: 0, skipCount: 0, transformedCount: 0 };
}

export function shouldRestartPortablePreview(errorCode: string): boolean {
  return RESTART_PREVIEW_ERROR_CODES.has(errorCode);
}

export function createPortableExecutionId(
  uuidFactory: () => string = () => crypto.randomUUID(),
): string {
  const executionId = uuidFactory();
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(executionId)) {
    throw new Error('A valid UUID execution ID is required');
  }
  return executionId;
}

export function formatPortableBytes(bytes: number, locale: string): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KiB', 'MiB', 'GiB'];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / (1024 ** unitIndex);
  return `${new Intl.NumberFormat(locale, {
    maximumFractionDigits: unitIndex === 0 ? 0 : 1,
  }).format(value)} ${units[unitIndex]}`;
}
