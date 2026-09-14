import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

const root = path.resolve(__dirname, '../..');
const read = (relativePath: string) => fs.readFileSync(path.join(root, relativePath), 'utf8');
const readJson = <T>(relativePath: string): T => JSON.parse(read(relativePath)) as T;

interface BuiltinTemplate {
  schema_version: number;
  id: string;
  name: string;
  description: string;
  version: number;
  locale: string;
  source: { type: string };
  sections: Array<{
    id: string;
    title: string;
    instruction: string;
    format: string;
    required: boolean;
    empty_behavior: string;
  }>;
}

const templateFiles = fs
  .readdirSync(path.join(root, 'src-tauri/templates/en'))
  .filter((file) => file.endsWith('.json'))
  .sort();

test('all six built-in templates have complete English and Simplified Chinese V2 resources', () => {
  assert.equal(templateFiles.length, 6);
  assert.deepEqual(
    fs.readdirSync(path.join(root, 'src-tauri/templates/zh-CN')).filter((file) => file.endsWith('.json')).sort(),
    templateFiles,
  );

  for (const file of templateFiles) {
    const en = readJson<BuiltinTemplate>(`src-tauri/templates/en/${file}`);
    const zh = readJson<BuiltinTemplate>(`src-tauri/templates/zh-CN/${file}`);
    assert.equal(en.schema_version, 2);
    assert.equal(zh.schema_version, 2);
    assert.equal(en.id, file.replace(/\.json$/, ''));
    assert.equal(zh.id, en.id);
    assert.equal(en.version, zh.version);
    assert.equal(en.locale, 'en');
    assert.equal(zh.locale, 'zh-CN');
    assert.equal(en.source.type, 'builtin');
    assert.equal(zh.source.type, 'builtin');
    assert.match(zh.name + zh.description, /[\u3400-\u9fff]/u);
    assert.ok(!/TODO|TBD|待翻|待译|TRANSLATE/i.test(JSON.stringify(zh)));
  }
});

test('localized variants preserve stable section identity and business structure', () => {
  for (const file of templateFiles) {
    const en = readJson<BuiltinTemplate>(`src-tauri/templates/en/${file}`);
    const zh = readJson<BuiltinTemplate>(`src-tauri/templates/zh-CN/${file}`);
    assert.deepEqual(
      zh.sections.map(({ id, format, required, empty_behavior }) => ({ id, format, required, empty_behavior })),
      en.sections.map(({ id, format, required, empty_behavior }) => ({ id, format, required, empty_behavior })),
    );
    assert.notEqual(JSON.stringify(en.sections), JSON.stringify(zh.sections));
  }
});

test('the 104 frozen content candidates all resolve to English and Chinese fields', () => {
  const coverage = readJson<{
    summary: { total: number; resolved: number; unresolved: number };
    entries: Array<{ resolved: boolean; mappings: Array<{ chinese: string | null }> }>;
  }>('../docs/i18n/phase-4/template-content-coverage.json');
  assert.deepEqual(coverage.summary, { total: 104, resolved: 104, unresolved: 0 });
  assert.ok(coverage.entries.every((entry) => entry.resolved));
  assert.ok(coverage.entries.every((entry) => entry.mappings.every((mapping) => mapping.chinese)));
});

test('generation content locale is driven by summary language and does not read UI locale', () => {
  const resolver = read('src-tauri/src/summary/template_commands_v2.rs');
  const localeModule = read('src-tauri/src/summary/templates/content_locale.rs');
  const uiHook = read('src/hooks/meeting-details/useTemplates.ts');
  assert.match(resolver, /content_locale_for_summary_language\(summary_language, None\)/);
  assert.match(resolver, /get_for_content_locale/);
  assert.doesNotMatch(resolver, /ui_locale|uiLocale/);
  assert.match(
    localeModule,
    /pub fn content_locale_for_summary_language\(\s*summary_language: Option<&str>,\s*detected_transcript_language: Option<&str>,/,
  );
  assert.match(uiHook, /contentLocale: displayContentLocale/);
  assert.match(uiHook, /templateService\.saveMeetingPreference/);
});

test('custom templates and historical snapshots remain authoritative across locale changes', () => {
  const repository = read('src-tauri/src/summary/templates/repository.rs');
  const snapshot = read('src-tauri/src/summary/template_snapshot.rs');
  const resolver = read('src-tauri/src/summary/template_commands_v2.rs');
  assert.match(repository, /TemplateOrigin::Custom => self\.read_custom_file\(template_id\)/);
  assert.match(repository, /locale_resolution_never_overwrites_or_substitutes_custom_template_content/);
  assert.match(resolver, /ResolvedGenerationTemplate::from_snapshot/);
  assert.match(snapshot, /semantic_sha256/);
  assert.match(snapshot, /HistoricalSnapshot/);
});
