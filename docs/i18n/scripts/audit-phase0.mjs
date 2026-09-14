import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

const repoRoot = path.resolve(process.argv[2] || '.');
const i18nRoot = path.join(repoRoot, 'docs', 'i18n');
const baselineRoot = path.join(i18nRoot, 'baseline');
const auditRoot = path.join(i18nRoot, 'audit', 'phase-0-baseline');
fs.mkdirSync(auditRoot, { recursive: true });

function readJson(relativePath) {
  return JSON.parse(fs.readFileSync(path.join(i18nRoot, relativePath), 'utf8'));
}

function writeJson(filePath, value) {
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

const candidates = readJson('baseline/source-text-candidates.json');
const catalog = readJson('baseline/en.catalog.generated.json');
const disposition = readJson('baseline/disposition.json');
const sourceMap = readJson('baseline/source-map.json');
const nativeMatrix = readJson('baseline/native-visibility-matrix.json');
const templateInventory = readJson('baseline/template-content-inventory.json');
const placeholderMigration = readJson('baseline/placeholder-migration.json');
const placeholders = readJson('baseline/placeholders.json');
const doNotTranslate = readJson('baseline/do-not-translate.json');
const glossary = readJson('baseline/glossary.en-zh-CN.json');
const manifest = readJson('baseline/phase0.manifest.json');
const buildResultPath = path.join(auditRoot, 'frontend-build-result.json');
const buildResult = fs.existsSync(buildResultPath)
  ? JSON.parse(fs.readFileSync(buildResultPath, 'utf8'))
  : { command: 'pnpm run build', exitCode: null, status: 'NOT_RUN' };
const frontendPackage = JSON.parse(fs.readFileSync(path.join(repoRoot, 'frontend', 'package.json'), 'utf8'));
const testRunner = frontendPackage.scripts?.test
  ? { status: 'AVAILABLE', command: 'pnpm test', configuredScript: frontendPackage.scripts.test }
  : { status: 'N/A', command: null, reason: 'frontend/package.json does not define a test script; Next production build still performs TypeScript validation.' };

const localeDir = path.join(baselineRoot, 'locales', 'en');
const localeFiles = fs.readdirSync(localeDir).filter((name) => name.endsWith('.json')).sort();
const formalValues = new Map();

function walkLocale(node, prefix) {
  if (typeof node === 'string') {
    if (formalValues.has(prefix)) throw new Error(`Duplicate formal key: ${prefix}`);
    formalValues.set(prefix, node);
    return;
  }
  if (!node || typeof node !== 'object' || Array.isArray(node)) throw new Error(`Invalid locale node at ${prefix}`);
  for (const [key, value] of Object.entries(node)) walkLocale(value, prefix ? `${prefix}.${key}` : key);
}

for (const localeFile of localeFiles) {
  const namespace = path.basename(localeFile, '.json');
  walkLocale(JSON.parse(fs.readFileSync(path.join(localeDir, localeFile), 'utf8')), namespace);
}

const expectedStatusCounts = {
  translate: 696,
  manual_review: 62,
  manual_visibility_review: 447,
  excluded_legacy_source: 45,
  separate_content_localization: 104,
};
const catalogByStatus = Object.fromEntries(
  Object.keys(expectedStatusCounts).map((status) => [status, catalog.entries.filter((entry) => entry.status === status).length]),
);

const checks = [];
function check(id, description, actual, expected, comparator = (a, b) => a === b) {
  const pass = comparator(actual, expected);
  checks.push({ id, description, status: pass ? 'PASS' : 'FAIL', actual, expected });
  return pass;
}

check('P0-001', 'Raw source occurrence count is frozen.', candidates.summary.total, 1549);
check('P0-002', 'Deduplicated catalog count is frozen.', catalog.entries.length, 1354);
for (const [status, expected] of Object.entries(expectedStatusCounts)) {
  check(`P0-STATUS-${status}`, `Catalog status ${status} has the expected count.`, catalogByStatus[status], expected);
}

const catalogIds = new Set(catalog.entries.map((entry) => entry.id));
const dispositionIds = disposition.entries.map((entry) => entry.id);
check('P0-003', 'Every catalog entry has exactly one disposition.', dispositionIds.length, catalogIds.size);
check('P0-004', 'Disposition IDs are unique.', new Set(dispositionIds).size, dispositionIds.length);
check('P0-005', 'No catalog ID is missing from disposition.', dispositionIds.filter((id) => !catalogIds.has(id)).length, 0);

const confirmedMappings = sourceMap.entries.filter((entry) => entry.originalStatus === 'translate');
const manualDispositions = disposition.entries.filter((entry) => entry.originalStatus === 'manual_review');
check('P0-006', 'All confirmed frontend entries map to formal keys.', confirmedMappings.length, 696);
check('P0-007', 'All manual frontend review entries have a final decision.', manualDispositions.length, 62);
check('P0-008', 'No manual frontend disposition remains unresolved.', manualDispositions.filter((entry) => /unresolved|pending|unknown/i.test(entry.disposition)).length, 0);
check('P0-009', 'Every formal source mapping retains at least one source location.', sourceMap.entries.filter((entry) => !entry.sources?.length).length, 0);
check('P0-010', 'Every included source mapping has at least one formal key.', sourceMap.entries.filter((entry) => !entry.finalKeys?.length).length, 0);

const missingFormalKeys = sourceMap.entries.flatMap((entry) => entry.finalKeys).filter((key) => !formalValues.has(key));
const unreferencedFormalKeys = [...formalValues.keys()].filter(
  (key) => !sourceMap.entries.some((entry) => entry.finalKeys.includes(key)),
);
check('P0-011', 'All mapped keys exist in the formal English resources.', missingFormalKeys.length, 0);
check('P0-012', 'All formal English keys are referenced by the source map.', unreferencedFormalKeys.length, 0);
check('P0-013', 'Formal English key count matches the manifest.', formalValues.size, manifest.counts.formalTranslationKeys);
check('P0-014', 'Exactly 12 approved business namespaces are generated.', localeFiles.length, 12);

const approvedNamespaces = new Set([
  'analytics', 'common', 'import', 'meetings', 'models', 'navigation',
  'onboarding', 'recording', 'settings', 'summary', 'transcription', 'updates',
]);
const invalidNamespaces = localeFiles.map((name) => path.basename(name, '.json')).filter((name) => !approvedNamespaces.has(name));
check('P0-015', 'Only approved business namespaces are present.', invalidNamespaces.length, 0);

const invalidKeys = [...formalValues.keys()].filter(
  (key) => !/^[a-z][A-Za-z0-9]*(\.[a-z][A-Za-z0-9_]*?)+$/.test(key),
);
const implementationKeys = [...formalValues.keys()].filter(
  (key) => /(^|\.)(components?|hooks?|contexts?|modal|provider)(\.|$)/i.test(key),
);
const collisionKeys = [...formalValues.keys()].filter((key) => /Variant[0-9a-f]{7}$/.test(key));
check('P0-016', 'All formal keys satisfy the semantic key syntax.', invalidKeys.length, 0);
check('P0-017', 'Formal keys do not expose React implementation layers.', implementationKeys.length, 0);
check('P0-018', 'No hash-based collision keys remain.', collisionKeys.length, 0);

const complexPlaceholders = [...formalValues.entries()].filter(([, value]) =>
  /\{\{[^}]*(\.|\?|\(|\)|:|\|)[^}]*\}\}/.test(value),
);
const singleBracePlaceholders = [...formalValues.entries()].filter(([, value]) =>
  /(?<!\{)\{[A-Za-z_][^{}]*\}(?!\})/.test(value),
);
const invalidPlaceholderNames = [...formalValues.entries()].filter(([, value]) =>
  [...value.matchAll(/\{\{([^}]+)\}\}/g)].some((match) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(match[1])),
);
check('P0-019', 'No complex expressions remain inside placeholders.', complexPlaceholders.length, 0);
check('P0-020', 'No legacy single-brace placeholders remain.', singleBracePlaceholders.length, 0);
check('P0-021', 'All placeholder names are simple identifiers.', invalidPlaceholderNames.length, 0);
check('P0-022', 'All detected placeholder-bearing keys are inventoried.', placeholders.entries.length, [...formalValues.values()].filter((value) => /\{\{/.test(value)).length);
check('P0-023', 'All original confirmed complex expressions plus additional review discoveries have explicit migration plans.', placeholderMigration.complexSourceExpressions.length, 24);

const cjkInEnglish = [...formalValues.entries()].filter(([, value]) => /[\u3400-\u9fff]/.test(value));
check('P0-024', 'The frozen English bundle contains no Chinese translation text.', cjkInEnglish.length, 0);

const activeNative = nativeMatrix.entries.filter((entry) => entry.originalStatus === 'manual_visibility_review');
const legacyNative = nativeMatrix.entries.filter((entry) => entry.originalStatus === 'excluded_legacy_source');
const unresolvedNative = activeNative.filter((entry) =>
  !['runtime_user_visible', 'runtime_frontend_boundary'].includes(entry.visibility)
  || !entry.surface
  || !entry.phase3Action
  || /unresolved|pending|unknown/i.test(entry.phase3Action),
);
check('P0-025', 'All active Rust/Tauri candidates are classified.', activeNative.length, 447);
check('P0-026', 'No active Rust/Tauri candidate remains unresolved.', unresolvedNative.length, 0);
check('P0-027', 'All explicitly legacy Rust entries remain isolated.', legacyNative.length, 45);

const incompleteTemplateEntries = templateInventory.entries.filter((entry) => !entry.disposition || !entry.sources?.length);
check('P0-028', 'All template/AI content candidates are inventoried.', templateInventory.entries.length, 104);
check('P0-029', 'No template/AI content entry remains unclassified.', incompleteTemplateEntries.length, 0);

check('P0-030', 'Do-not-translate list contains a meaningful protected-term baseline.', doNotTranslate.exactTerms.length >= 20, true);
check('P0-031', 'Approved bilingual glossary contains at least 40 entries.', glossary.entries.length >= 40, true);
check('P0-032', 'Every glossary entry has English, zh-CN, a note, and approval.', glossary.entries.filter((entry) => !entry.en || !entry.zhCN || !entry.note || entry.approved !== true).length, 0);

const gitCommit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: repoRoot, encoding: 'utf8' }).trim();
const gitStatus = execFileSync('git', ['status', '--porcelain'], { cwd: repoRoot, encoding: 'utf8' })
  .split(/\r?\n/)
  .filter(Boolean);
const runtimeChanges = gitStatus.filter((line) => {
  const changedPath = line.slice(3).replaceAll('\\', '/');
  return !changedPath.startsWith('docs/i18n/');
});
check('P0-033', 'Phase 0 does not modify runtime application source.', runtimeChanges.length, 0);
check('P0-034', 'Frontend production build passes.', buildResult.exitCode, 0);

const failures = checks.filter((item) => item.status === 'FAIL');
const result = failures.length === 0 ? 'PASS' : 'FAIL';
const report = {
  schemaVersion: 1,
  phase: '15.3 / Phase 0 - Freeze English baseline',
  scope: 'AUTOMATED_STATIC_BASELINE_ONLY',
  fullPhaseGate: 'PENDING_RUNTIME_AUDIT',
  auditedAt: new Date().toISOString(),
  gitCommit,
  result,
  severitySummary: {
    P0: 0,
    P1: failures.length,
    P2: 0,
    P3: 0,
  },
  summary: {
    checks: checks.length,
    passed: checks.filter((item) => item.status === 'PASS').length,
    failed: failures.length,
    rawOccurrences: candidates.summary.total,
    catalogEntries: catalog.entries.length,
    formalEnglishKeys: formalValues.size,
    namespaces: localeFiles.length,
    sourceMappings: sourceMap.entries.length,
    activeNativeClassifications: activeNative.length,
    templateContentEntries: templateInventory.entries.length,
    placeholderMigrationPlans: placeholderMigration.complexSourceExpressions.length,
  },
  buildResult,
  testRunner,
  runtimeChanges,
  checks,
};

writeJson(path.join(auditRoot, 'phase0-audit-report.json'), report);

const markdown = `# Meetily i18n 阶段 0 自动静态审计报告

- 阶段：15.3 阶段 0——冻结英文基线
- Git Commit：\`${gitCommit}\`
- 审计时间：${report.auditedAt}
- 自动静态审计结论：**${result}**
- 完整阶段门禁：**PENDING_RUNTIME_AUDIT**
- 检查项：${report.summary.checks}
- 通过：${report.summary.passed}
- 失败：${report.summary.failed}
- P0 / P1 / P2 / P3：${report.severitySummary.P0} / ${report.severitySummary.P1} / ${report.severitySummary.P2} / ${report.severitySummary.P3}

## 冻结结果

| 指标 | 结果 |
|---|---:|
| 原始文本出现位置 | ${report.summary.rawOccurrences} |
| 去重目录记录 | ${report.summary.catalogEntries} |
| 正式英文翻译键 | ${report.summary.formalEnglishKeys} |
| 业务 Namespace | ${report.summary.namespaces} |
| 正式源码映射 | ${report.summary.sourceMappings} |
| 活跃 Rust/Tauri 分类 | ${report.summary.activeNativeClassifications} |
| 模板/AI 内容记录 | ${report.summary.templateContentEntries} |
| 显式占位符迁移计划 | ${report.summary.placeholderMigrationPlans} |

## 构建与源码边界

- 前端构建命令：\`${buildResult.command}\`
- 前端构建退出码：\`${buildResult.exitCode}\`
- 项目统一测试脚本：\`${testRunner.status}\`${testRunner.reason ? `（${testRunner.reason}）` : ''}
- 运行时应用源码变更数：\`${runtimeChanges.length}\`
- 阶段 0 只新增文档、基线、审计脚本和生成物，不修改运行时 UI。

## 审计明细

| ID | 检查项 | 结果 | 实际 | 期望 |
|---|---|---|---:|---:|
${checks.map((item) => `| ${item.id} | ${item.description} | ${item.status} | ${String(item.actual).replaceAll('|', '\\|')} | ${String(item.expected).replaceAll('|', '\\|')} |`).join('\n')}

## 自动静态审计边界

${result === 'PASS'
  ? '全部自动静态检查通过。该结论不覆盖英文 UI 冒烟、编辑状态、事件冒泡、Rust/Tauri 错误传播或端到端运行时测试；完整阶段 0 仍不得放行。'
  : `存在 ${failures.length} 个失败检查，阶段 0 不得放行。请查看 JSON 报告中的失败项。`}
`;

fs.writeFileSync(path.join(auditRoot, 'phase0-audit-report.md'), markdown, 'utf8');
console.log(JSON.stringify({ result, ...report.summary, failures }, null, 2));
if (failures.length) process.exit(1);
