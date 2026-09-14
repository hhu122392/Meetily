import assert from 'node:assert/strict';
import test from 'node:test';
import {
  createEmptyMeetingContextProfile,
  meetingContextProfileFromExtensions,
  meetingContextProfileSha256,
  parseMeetingContextProfile,
  removeMeetingContextProfileExtension,
  setMeetingContextProfileExtension,
  splitAliases,
  validateMeetingContextProfile,
  validateRecordingMeetingContextDraft,
} from '../../src/lib/meeting-context';
import {
  editorDraftToTemplate,
  isTemplateDraftDirty,
  templateToEditorDraft,
  validateTemplateDraft,
} from '../../src/lib/template-editor';
import {
  MEETING_CONTEXT_EXTENSION_KEY,
  type MeetingContextProfile,
  type TemplateV2,
} from '../../src/types/summary-template';

function profile(): MeetingContextProfile {
  return {
    schema_version: 1,
    fixed_meeting_mechanism: '每周三 14:00 召开牌照站周会',
    people: [{
      person_id: 'person_rayson',
      display_name: 'Rayson',
      aliases: ['瑞森', 'Reason'],
      department: '市场部',
      role: null,
      enabled: true,
    }],
    terms: [{
      term_id: 'term_m100',
      canonical: 'M100',
      aliases: ['产品一百'],
      category: 'product',
      enabled: true,
    }],
  };
}

function template(extensions: Record<string, unknown>): TemplateV2 {
  return {
    schemaVersion: 2,
    id: 'license_station_weekly',
    name: '牌照站周会',
    description: '牌照站固定周会模板',
    version: 1,
    locale: 'zh-CN',
    tags: [],
    source: {
      type: 'manual',
      originalFileName: null,
      originalFileSha256: null,
      importedAt: null,
      copiedFromTemplateId: null,
    },
    createdAt: '2026-08-26T00:00:00Z',
    updatedAt: '2026-08-26T00:00:00Z',
    sections: [{
      id: 'summary',
      title: '会议结论',
      instruction: '提取会议结论。',
      format: 'list',
      itemFormat: '- {{item}}',
      exampleItemFormat: null,
      required: true,
      emptyBehavior: 'show_not_mentioned',
    }],
    extensions,
  };
}

test('parses a valid profile and applies Rust-compatible defaults', () => {
  const parsed = parseMeetingContextProfile({
    schema_version: 1,
    people: [{ person_id: 'person_amu', display_name: 'Amu' }],
  });
  assert.deepEqual(parsed, {
    schema_version: 1,
    fixed_meeting_mechanism: null,
    people: [{
      person_id: 'person_amu',
      display_name: 'Amu',
      aliases: [],
      department: null,
      role: null,
      enabled: true,
    }],
    terms: [],
  });
  assert.equal(parseMeetingContextProfile({ schema_version: 2 }), null);
  assert.equal(parseMeetingContextProfile({ schema_version: 1, people: 'invalid' }), null);
});

test('setting and removing managed context preserves every unknown extension', () => {
  const original = {
    'vendor.example': { nested: { enabled: true } },
    another_extension: ['keep', 1],
  };
  const withContext = setMeetingContextProfileExtension(original, profile());
  assert.deepEqual(withContext['vendor.example'], original['vendor.example']);
  assert.deepEqual(withContext.another_extension, original.another_extension);
  assert.deepEqual(meetingContextProfileFromExtensions(withContext), profile());
  const removed = removeMeetingContextProfileExtension(withContext);
  assert.deepEqual(removed, original);
  assert.equal(MEETING_CONTEXT_EXTENSION_KEY in original, false);
});

test('editor round trip preserves unknown extensions and marks context edits dirty', () => {
  const initial = template({
    'vendor.example': { keep: true },
    [MEETING_CONTEXT_EXTENSION_KEY]: profile(),
  });
  const baseline = templateToEditorDraft(initial);
  const draft = templateToEditorDraft(initial);
  const current = meetingContextProfileFromExtensions(draft.extensions)!;
  current.people[0].aliases.push('Risa');
  draft.extensions = setMeetingContextProfileExtension(draft.extensions, current);

  assert.equal(isTemplateDraftDirty(draft, baseline), true);
  const restored = editorDraftToTemplate(draft);
  assert.deepEqual(restored.extensions['vendor.example'], { keep: true });
  assert.deepEqual(meetingContextProfileFromExtensions(restored.extensions)?.people[0].aliases, [
    '瑞森',
    'Reason',
    'Risa',
  ]);
});

test('validation rejects NFKC duplicates, alias conflicts and control characters', () => {
  const value = profile();
  value.people.push({
    person_id: 'PERSON_RAYSON',
    display_name: 'ＲＡＹＳＯＮ',
    aliases: ['瑞森', 'bad\nname'],
    department: null,
    role: null,
    enabled: true,
  });
  const codes = validateMeetingContextProfile(value).map((issue) => issue.code);
  assert.ok(codes.includes('DUPLICATE_PERSON_ID'));
  assert.ok(codes.includes('DUPLICATE_PERSON_NAME'));
  assert.ok(codes.includes('AMBIGUOUS_PERSON_ALIAS'));
  assert.ok(codes.includes('FORBIDDEN_CHARACTER'));
});

test('template local validation reports invalid managed extension but allows it to be absent', () => {
  const withoutContext = templateToEditorDraft(template({}));
  assert.equal(validateTemplateDraft(withoutContext).some((issue) => issue.path.includes('meetily_meeting_context')), false);

  const invalid = templateToEditorDraft(template({
    [MEETING_CONTEXT_EXTENSION_KEY]: { schema_version: 1, people: 'invalid' },
  }));
  assert.equal(
    validateTemplateDraft(invalid).some((issue) => issue.path === `/extensions/${MEETING_CONTEXT_EXTENSION_KEY}`),
    true,
  );
});

test('alias input accepts Chinese and ASCII separators without retaining blanks', () => {
  assert.deepEqual(splitAliases('瑞森，Reason、Risa; Raison\n'), ['瑞森', 'Reason', 'Risa', 'Raison']);
  assert.deepEqual(createEmptyMeetingContextProfile().people, []);
});

test('profile hash is stable after Rust-compatible whitespace normalization', async () => {
  const clean = profile();
  const padded = profile();
  padded.people[0].display_name = '  Rayson  ';
  padded.people[0].aliases = ['  瑞森 ', 'Reason  '];
  padded.people[0].role = '   ';
  assert.equal(await meetingContextProfileSha256(clean), await meetingContextProfileSha256(padded));
});

test('recording draft validation rejects an absent host and ambiguous guest alias', () => {
  const value = profile();
  const issues = validateRecordingMeetingContextDraft(value, {
    expectedProfileSha256: '0'.repeat(64),
    attendance: [{ personId: 'person_rayson', attendance: 'absent' }],
    hostPersonId: 'person_rayson',
    guests: [{
      personId: 'guest_other',
      displayName: 'Other',
      aliases: ['瑞森'],
      department: null,
      role: null,
    }],
    additionalTerms: [],
  });
  const codes = issues.map((issue) => issue.code);
  assert.ok(codes.includes('HOST_MARKED_ABSENT'));
  assert.ok(codes.includes('AMBIGUOUS_PERSON_ALIAS'));
});
