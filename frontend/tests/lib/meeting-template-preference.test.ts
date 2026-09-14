import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

test('meeting details loads and persists a meeting-scoped template preference', () => {
  const page = read('src/app/meeting-details/page-content.tsx');
  const hook = read('src/hooks/meeting-details/useTemplates.ts');
  const service = read('src/services/templateService.ts');

  assert.match(page, /useTemplates\(meeting\.id\)/);
  assert.match(service, /api_get_meeting_template_preference/);
  assert.match(service, /api_save_meeting_template_preference/);
  assert.match(hook, /savePreference\('meeting_override', templateId, templateName\)/);
  assert.match(hook, /savePreference\('inherit', null\)/);
});

test('serialized saves and version guards make the last user intent win', () => {
  const hook = read('src/hooks/meeting-details/useTemplates.ts');

  assert.match(hook, /saveQueueRef\.current\s*=\s*operation\.then/);
  assert.match(hook, /saveVersion !== saveVersionRef\.current/);
  assert.match(hook, /activeMeetingRef\.current !== requestMeetingId/);
  assert.match(hook, /setPreferenceResponse\(previous\)/);
});

test('every summary generation entry is locked while template state is unresolved', () => {
  const controls = read('src/components/MeetingDetails/SummaryGeneratorButtonGroup.tsx');
  const panel = read('src/components/MeetingDetails/SummaryPanel.tsx');

  assert.match(
    controls,
    /disabled=\{isCheckingModels \|\| isModelConfigLoading \|\| isTemplateLoading \|\| isTemplateSaving \|\| Boolean\(templateIssue \|\| templateError\)\}/,
  );
  assert.match(controls, /if \(isTemplateLoading \|\| isTemplateSaving \|\| templateIssue \|\| templateError\) return/);
  assert.match(panel, /isDisabled=\{isTemplateLoading \|\| isTemplateSaving \|\| Boolean\(templateIssue \|\| templateError\)\}/);
});

test('English and Simplified Chinese template preference keys stay identical', () => {
  const english = JSON.parse(read('src/i18n/locales/en/summary.json'));
  const chinese = JSON.parse(read('src/i18n/locales/zh-CN/summary.json'));

  assert.deepEqual(
    Object.keys(chinese.templatePreference).sort(),
    Object.keys(english.templatePreference).sort(),
  );
  for (const [key, value] of Object.entries(chinese.templatePreference)) {
    assert.equal(typeof value, 'string', key);
    assert.notEqual((value as string).trim(), '', key);
  }
});
