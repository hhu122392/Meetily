#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const templateRoot = path.join(root, "frontend/src-tauri/templates");
const phaseDirectory = path.join(root, "docs/i18n/phase-4");
const auditDirectory = path.join(root, "docs/i18n/audit/phase-4-content");

const sha256 = (value) =>
  crypto.createHash("sha256").update(value).digest("hex").toUpperCase();
const normalize = (value) => value.replace(/\s+/g, " ").trim();

function contentFields(template) {
  const fields = [
    { path: "/name", value: template.name },
    { path: "/description", value: template.description },
  ];
  template.sections.forEach((section, index) => {
    for (const key of ["title", "instruction", "item_format", "example_item_format"]) {
      if (typeof section[key] === "string") {
        fields.push({ path: `/sections/${index}/${key}`, value: section[key] });
      }
    }
  });
  return fields;
}

async function readTemplate(locale, file) {
  const relativePath = `frontend/src-tauri/templates/${locale}/${file}`;
  const content = await fs.readFile(path.join(root, relativePath), "utf8");
  return { relativePath, content, value: JSON.parse(content) };
}

const englishFiles = (await fs.readdir(path.join(templateRoot, "en")))
  .filter((file) => file.endsWith(".json"))
  .sort();
const chineseFiles = (await fs.readdir(path.join(templateRoot, "zh-CN")))
  .filter((file) => file.endsWith(".json"))
  .sort();
const schema = JSON.parse(
  await fs.readFile(path.join(root, "frontend/src-tauri/schemas/template-v2.schema.json"), "utf8"),
);
const requiredRootFields = schema.required;
const pairs = [];
for (const file of englishFiles) {
  const en = await readTemplate("en", file);
  const zh = await readTemplate("zh-CN", file);
  const missingRequired = {
    en: requiredRootFields.filter((field) => !(field in en.value)),
    zhCN: requiredRootFields.filter((field) => !(field in zh.value)),
  };
  const structure = (template) =>
    template.sections.map((section) => ({
      id: section.id,
      format: section.format,
      required: section.required,
      empty_behavior: section.empty_behavior,
      hasItemFormat: section.item_format !== null,
      hasExampleItemFormat: section.example_item_format !== null,
    }));
  const zhFields = contentFields(zh.value);
  pairs.push({
    id: en.value.id,
    version: en.value.version,
    files: { en: en.relativePath, zhCN: zh.relativePath },
    hashes: { en: sha256(en.content), zhCN: sha256(zh.content) },
    locales: { en: en.value.locale, zhCN: zh.value.locale },
    sourceTypes: { en: en.value.source?.type, zhCN: zh.value.source?.type },
    sections: { en: structure(en.value), zhCN: structure(zh.value) },
    names: { en: en.value.name, zhCN: zh.value.name },
    missingRequired,
    chineseContent: {
      fields: zhFields.length,
      blanks: zhFields.filter((field) => field.value.trim().length === 0).map((field) => field.path),
      pendingMarkers: zhFields
        .filter((field) => /TODO|TBD|待翻|待译|TRANSLATE/i.test(field.value))
        .map((field) => field.path),
      hanCharacterFields: zhFields.filter((field) => /[\u3400-\u9fff]/u.test(field.value)).length,
    },
  });
}

const inventory = JSON.parse(
  await fs.readFile(path.join(root, "docs/i18n/baseline/template-content-inventory.json"), "utf8"),
);
const coverageEntries = [];
for (const candidate of inventory.entries) {
  const mappings = [];
  for (const source of candidate.sources) {
    const file = path.basename(source.file);
    const en = await readTemplate("en", file);
    const zh = await readTemplate("zh-CN", file);
    const target = contentFields(en.value).find(
      (field) => normalize(field.value) === normalize(candidate.en),
    );
    if (target) {
      const localized = contentFields(zh.value).find((field) => field.path === target.path);
      mappings.push({
        source: source.file,
        englishTarget: `${en.relativePath}#${target.path}`,
        chineseTarget: localized ? `${zh.relativePath}#${localized.path}` : null,
        chinese: localized?.value ?? null,
      });
    }
  }
  coverageEntries.push({
    id: candidate.id,
    en: candidate.en,
    disposition: candidate.disposition,
    mappings,
    resolved: mappings.length > 0 && mappings.every((mapping) => mapping.chinese),
  });
}

const ids = pairs.map((pair) => pair.id);
const config = await fs.readFile(path.join(root, "frontend/src-tauri/tauri.conf.json"), "utf8");
const generationResolver = await fs.readFile(
  path.join(root, "frontend/src-tauri/src/summary/template_commands_v2.rs"),
  "utf8",
);
const repository = await fs.readFile(
  path.join(root, "frontend/src-tauri/src/summary/templates/repository.rs"),
  "utf8",
);
const processor = await fs.readFile(
  path.join(root, "frontend/src-tauri/src/summary/processor.rs"),
  "utf8",
);

const checks = {
  sixEnglishTemplates: englishFiles.length === 6,
  sixChineseTemplates:
    chineseFiles.length === 6 && JSON.stringify(englishFiles) === JSON.stringify(chineseFiles),
  requiredFieldsPresent: pairs.every(
    (pair) => pair.missingRequired.en.length === 0 && pair.missingRequired.zhCN.length === 0,
  ),
  stableIdsAndVersions: pairs.every(
    (pair) => pair.id === path.basename(pair.files.en, ".json") && pair.version >= 1,
  ),
  idsGloballyUnique: new Set(ids).size === ids.length,
  localeMetadataCorrect: pairs.every(
    (pair) => pair.locales.en === "en" && pair.locales.zhCN === "zh-CN",
  ),
  builtinSourceType: pairs.every(
    (pair) => pair.sourceTypes.en === "builtin" && pair.sourceTypes.zhCN === "builtin",
  ),
  schemaDeclaresBuiltinSource:
    schema.$defs?.source?.properties?.type?.enum?.includes("builtin") === true,
  structureParity: pairs.every(
    (pair) => JSON.stringify(pair.sections.en) === JSON.stringify(pair.sections.zhCN),
  ),
  localizedContentDiffers: pairs.every((pair) => pair.hashes.en !== pair.hashes.zhCN),
  chineseComplete: pairs.every(
    (pair) =>
      pair.chineseContent.blanks.length === 0 &&
      pair.chineseContent.pendingMarkers.length === 0 &&
      pair.chineseContent.hanCharacterFields >= 2,
  ),
  candidateCountFrozenAt104: coverageEntries.length === 104,
  candidateCoverageComplete: coverageEntries.every((entry) => entry.resolved),
  localizedResourcesBundledOnly:
    config.includes('"templates/en/*.json"') &&
    config.includes('"templates/zh-CN/*.json"') &&
    !config.includes('"templates/*.json"'),
  summaryLanguageDrivesContentLocale:
    generationResolver.includes("content_locale_for_summary_language(summary_language, None)") &&
    generationResolver.includes("get_for_content_locale"),
  customTemplatesRemainOriginAuthoritative:
    repository.includes("TemplateOrigin::Custom => self.read_custom_file(template_id)") &&
    repository.includes("locale_resolution_never_overwrites_or_substitutes_custom_template_content"),
  localizedBuiltinPrecedesLegacyBundled:
    repository.indexOf("if defaults::get_builtin_template(template_id).is_some()") >= 0 &&
    repository.indexOf("if defaults::get_builtin_template(template_id).is_some()") <
      repository.indexOf("if let Some(root) = self.bundled_root.as_ref()"),
  promptLanguagePrecedenceExplicit:
    processor.includes("TEMPLATE_LANGUAGE_PRECEDENCE_INSTRUCTION") &&
    processor.includes("never overrides the requested summary language"),
};

const manifest = {
  schemaVersion: 1,
  phase: "15.7-stage-4-template-and-ai-content-i18n",
  generatedAt: new Date().toISOString(),
  supportedLocales: ["en", "zh-CN"],
  fallbackLocale: "en",
  templates: pairs,
};
const coverage = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  source: "docs/i18n/baseline/template-content-inventory.json",
  summary: {
    total: coverageEntries.length,
    resolved: coverageEntries.filter((entry) => entry.resolved).length,
    unresolved: coverageEntries.filter((entry) => !entry.resolved).length,
  },
  entries: coverageEntries,
};
const report = {
  phase: manifest.phase,
  generatedAt: new Date().toISOString(),
  passed: Object.values(checks).every(Boolean),
  checks,
  totals: {
    templates: pairs.length,
    localizedResources: pairs.length * 2,
    contentCandidates: coverageEntries.length,
    unresolvedCandidates: coverage.summary.unresolved,
  },
};

await fs.mkdir(phaseDirectory, { recursive: true });
await fs.mkdir(auditDirectory, { recursive: true });
await fs.writeFile(
  path.join(phaseDirectory, "builtin-template-manifest.json"),
  `${JSON.stringify(manifest, null, 2)}\n`,
);
await fs.writeFile(
  path.join(phaseDirectory, "template-content-coverage.json"),
  `${JSON.stringify(coverage, null, 2)}\n`,
);
await fs.writeFile(
  path.join(auditDirectory, "static-audit.json"),
  `${JSON.stringify(report, null, 2)}\n`,
);

process.stdout.write(
  `${JSON.stringify({
    passed: report.passed,
    checks: Object.keys(checks).length,
    failed: Object.entries(checks).filter(([, passed]) => !passed).map(([name]) => name),
    totals: report.totals,
  })}\n`,
);
if (!report.passed) process.exitCode = 1;
