import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import {
  addTemplateSection,
  createSaveAsCopyDraft,
  createTemplateDraft,
  duplicateTemplateSection,
  editorDraftToTemplate,
  formatEditorJson,
  isTemplateDraftDirty,
  moveTemplateSection,
  parseEditorJson,
  removeTemplateSection,
  slugifySectionId,
  slugifyTemplateId,
  updateTemplateSection,
  validateTemplateDraft,
  type TemplateEditorDraft,
} from '../../src/lib/template-editor';
import { useTemplateEditor } from '../../src/hooks/useTemplateEditor';
import type { TemplateService } from '../../src/services/templateService';
import type {
  TemplateDetails,
  TemplateValidationResult,
  TemplateV2,
} from '../../src/types/summary-template';

function validDraft(): TemplateEditorDraft {
  let draft = createTemplateDraft('2026-08-23T00:00:00Z');
  draft = {
    ...draft,
    id: 'customer_review',
    name: 'Customer Review',
    description: 'Review customer requirements and actions.',
  };
  return updateTemplateSection(draft, draft.sections[0].draftKey, {
    title: 'Summary',
    instruction: 'Summarize the meeting.',
  });
}

function details(template: TemplateV2): TemplateDetails {
  return {
    template,
    origin: 'custom',
    schemaVersionOnDisk: 2,
    fileSha256: 'a'.repeat(64),
    semanticSha256: 'b'.repeat(64),
    isDefault: false,
    overridesBuiltin: false,
    readOnly: false,
  };
}

function validResult(template: TemplateV2): TemplateValidationResult {
  return { valid: true, errors: [], warnings: [], normalized: template };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

test('creates a safe v2 draft and reports every required blank field', () => {
  const draft = createTemplateDraft('2026-08-23T00:00:00Z');
  assert.equal(draft.schemaVersion, 2);
  assert.equal(draft.sections.length, 1);
  assert.equal(draft.source.type, 'manual');
  assert.deepEqual(
    validateTemplateDraft(draft).map((issue) => issue.path),
    ['/name', '/description', '/sections/0/title', '/sections/0/instruction'],
  );
});

test('generates filesystem-safe deterministic slugs and uses a safe fallback for Chinese-only names', () => {
  assert.equal(slugifyTemplateId('Weekly Sales Review'), 'weekly_sales_review');
  assert.equal(slugifyTemplateId('  A/B \\ C  '), 'a_b_c');
  assert.equal(slugifyTemplateId('客户需求评审', 'abc123'), 'template_abc123');
  assert.equal(slugifySectionId('Action Items'), 'action_items');
  assert.equal(slugifySectionId('行动项', 'xyz'), 'section_xyz');
});

test('adds, duplicates, removes and reorders sections without changing stable draft keys', () => {
  const original = validDraft();
  const firstKey = original.sections[0].draftKey;
  const added = addTemplateSection(original);
  assert.equal(added.sections.length, 2);
  assert.equal(added.sections[0].draftKey, firstKey);

  const duplicated = duplicateTemplateSection(added, firstKey);
  assert.equal(duplicated.sections.length, 3);
  assert.notEqual(duplicated.sections[1].draftKey, firstKey);
  assert.equal(new Set(duplicated.sections.map((section) => section.id)).size, 3);

  const moved = moveTemplateSection(duplicated, firstKey, 2);
  assert.equal(moved.sections[2].draftKey, firstKey);
  const removed = removeTemplateSection(moved, moved.sections[0].draftKey);
  assert.equal(removed.sections.length, 2);
  const protectedLast = removeTemplateSection({ ...removed, sections: [removed.sections[0]] }, removed.sections[0].draftKey);
  assert.equal(protectedLast.sections.length, 1);
});

test('section helpers enforce the 50-section ceiling and clamp move boundaries', () => {
  let draft = validDraft();
  while (draft.sections.length < 50) draft = addTemplateSection(draft);
  const full = draft;
  assert.equal(addTemplateSection(full), full);
  assert.equal(duplicateTemplateSection(full, full.sections[0].draftKey), full);

  const lastKey = full.sections[49].draftKey;
  const movedToTop = moveTemplateSection(full, lastKey, -999);
  assert.equal(movedToTop.sections[0].draftKey, lastKey);
  const movedToBottom = moveTemplateSection(movedToTop, lastKey, 999);
  assert.equal(movedToBottom.sections[49].draftKey, lastKey);
});

test('dirty comparison ignores version/timestamps/draft keys but detects editable semantic changes', () => {
  const baseline = validDraft();
  const metadataOnly = {
    ...baseline,
    version: 99,
    createdAt: '2030-01-01T00:00:00Z',
    updatedAt: '2030-01-02T00:00:00Z',
    sections: baseline.sections.map((section) => ({ ...section, draftKey: `different-${section.draftKey}` })),
  };
  assert.equal(isTemplateDraftDirty(metadataOnly, baseline), false);
  assert.equal(isTemplateDraftDirty({ ...baseline, name: 'Changed' }, baseline), true);
});

test('advanced JSON is camelCase, parse failure is isolated, and save-as-copy resets authority fields', () => {
  const draft = validDraft();
  const json = formatEditorJson(draft);
  assert.match(json, /"schemaVersion": 2/);
  assert.doesNotMatch(json, /schema_version/);
  assert.equal(parseEditorJson('{invalid').value, null);
  assert.ok(parseEditorJson('{invalid').error);

  const copy = createSaveAsCopyDraft(draft, 'customer_review_copy', 'Customer Review Copy', '2026-08-24T00:00:00Z');
  assert.equal(copy.version, 1);
  assert.equal(copy.source.type, 'manual');
  assert.equal(copy.createdAt, '2026-08-24T00:00:00Z');
  assert.equal(copy.sections[0].draftKey, draft.sections[0].draftKey);
});

test('editor hook suppresses same-tick duplicate saves and adopts backend version/hash as the new baseline', async () => {
  const createDeferred = deferred<TemplateDetails>();
  let createCalls = 0;
  const fakeService = {
    validate: async (template: TemplateV2) => validResult(template),
    create: (request: { template: TemplateV2 }) => {
      createCalls += 1;
      assert.equal(request.template.id, 'customer_review');
      return createDeferred.promise;
    },
  } as unknown as TemplateService;

  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe() {
    editor = useTemplateEditor({ mode: 'create', templateId: null, origin: null }, fakeService);
    return null;
  }

  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });
  act(() => editor.setDraft(validDraft()));

  let first!: Promise<TemplateDetails | null>;
  let second!: Promise<TemplateDetails | null>;
  act(() => {
    first = editor.save();
    second = editor.save();
  });
  assert.equal(await second, null);
  await Promise.resolve();
  assert.equal(createCalls, 1);

  const authoritative = editorDraftToTemplate({ ...validDraft(), version: 4 });
  const savedDetails = {
    ...details(authoritative),
    fileSha256: 'c'.repeat(64),
  };
  createDeferred.resolve(savedDetails);
  await act(async () => { await first; });
  assert.equal(editor.details?.template.version, 4);
  assert.equal(editor.details?.fileSha256, 'c'.repeat(64));
  assert.equal(editor.dirty, false);
  assert.equal(editor.saving, false);
  act(() => renderer.unmount());
});

test('editor hook keeps the draft dirty and exposes a structured create conflict', async () => {
  const conflict = {
    code: 'TEMPLATE_ALREADY_EXISTS',
    messageKey: 'templates.errors.alreadyExists',
    retryable: false,
    debugId: 'conflict-debug',
  };
  const fakeService = {
    validate: async (template: TemplateV2) => validResult(template),
    create: async () => { throw conflict; },
  } as unknown as TemplateService;

  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe() {
    editor = useTemplateEditor({ mode: 'create', templateId: null, origin: null }, fakeService);
    return null;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });
  act(() => editor.setDraft(validDraft()));
  await act(async () => { await editor.save(); });
  assert.equal(editor.conflict?.debugId, 'conflict-debug');
  assert.equal(editor.dirty, true);
  assert.equal(editor.saveError, null);
  act(() => renderer.unmount());
});

test('invalid JSON validation never replaces the current form draft', async () => {
  const invalidResult: TemplateValidationResult = {
    valid: false,
    errors: [{ code: 'SCHEMA_VIOLATION', path: '/name', messageKey: 'templates.validation.schemaViolation' }],
    warnings: [],
    normalized: null,
  };
  const fakeService = {
    validate: async () => invalidResult,
  } as unknown as TemplateService;
  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe() {
    editor = useTemplateEditor({ mode: 'create', templateId: null, origin: null }, fakeService);
    return null;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });
  act(() => editor.setDraft(validDraft()));
  const before = editor.draft.name;
  let applied = true;
  await act(async () => {
    applied = await editor.applyJsonValue({ name: 'Broken' });
  });
  assert.equal(applied, false);
  assert.equal(editor.draft.name, before);
  assert.equal(editor.validation?.valid, false);
  act(() => editor.clearValidation());
  assert.equal(editor.validation, null);
  act(() => renderer.unmount());
});

test('edit save sends the authoritative version and file hash for optimistic concurrency', async () => {
  const loadedTemplate = editorDraftToTemplate({ ...validDraft(), version: 7 });
  const loadedDetails = {
    ...details(loadedTemplate),
    fileSha256: '7'.repeat(64),
  };
  let updateRequest: Parameters<TemplateService['update']>[0] | null = null;
  const fakeService = {
    get: async () => loadedDetails,
    validate: async (template: TemplateV2) => validResult(template),
    update: async (request: Parameters<TemplateService['update']>[0]) => {
      updateRequest = request;
      return {
        ...loadedDetails,
        template: { ...request.template, version: 8 },
        fileSha256: '8'.repeat(64),
      };
    },
  } as unknown as TemplateService;

  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe() {
    editor = useTemplateEditor({ mode: 'edit', templateId: loadedTemplate.id, origin: 'custom' }, fakeService);
    return null;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });
  act(() => editor.setDraft((current) => ({ ...current, description: 'Updated description.' })));
  await act(async () => { await editor.save(); });

  const capturedUpdateRequest = updateRequest as Parameters<TemplateService['update']>[0] | null;
  assert.ok(capturedUpdateRequest);
  assert.equal(capturedUpdateRequest.expectedVersion, 7);
  assert.equal(capturedUpdateRequest.expectedFileSha256, '7'.repeat(64));
  assert.equal(capturedUpdateRequest.template.id, loadedTemplate.id);
  assert.equal(editor.details?.template.version, 8);
  assert.equal(editor.details?.fileSha256, '8'.repeat(64));
  assert.equal(editor.dirty, false);
  act(() => renderer.unmount());
});

test('stale edit loads cannot replace the most recent route draft', async () => {
  const firstLoad = deferred<TemplateDetails>();
  const secondTemplate = editorDraftToTemplate({
    ...validDraft(),
    id: 'second_template',
    name: 'Second Template',
  });
  const fakeService = {
    get: ({ templateId }: { templateId: string }) => templateId === 'first_template'
      ? firstLoad.promise
      : Promise.resolve(details(secondTemplate)),
    validate: async (template: TemplateV2) => validResult(template),
  } as unknown as TemplateService;

  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe({ templateId }: { templateId: string }) {
    editor = useTemplateEditor({ mode: 'edit', templateId, origin: 'custom' }, fakeService);
    return null;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe templateId="first_template" />);
    await Promise.resolve();
  });
  await act(async () => {
    renderer.update(<Probe templateId="second_template" />);
    await Promise.resolve();
  });
  assert.equal(editor.draft.id, 'second_template');

  firstLoad.resolve(details(editorDraftToTemplate({ ...validDraft(), id: 'first_template', name: 'First Template' })));
  await act(async () => { await Promise.resolve(); });
  assert.equal(editor.draft.id, 'second_template');
  assert.equal(editor.details?.template.id, 'second_template');
  act(() => renderer.unmount());
});

test('backend validation transport failures become visible blocking issues', async () => {
  const failure = {
    code: 'TEMPLATE_IO_ERROR',
    messageKey: 'templates.errors.io',
    retryable: true,
    debugId: 'validation-io',
  };
  const fakeService = {
    validate: async () => { throw failure; },
  } as unknown as TemplateService;
  let editor!: ReturnType<typeof useTemplateEditor>;
  function Probe() {
    editor = useTemplateEditor({ mode: 'create', templateId: null, origin: null }, fakeService);
    return null;
  }
  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });
  act(() => editor.setDraft(validDraft()));
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 400));
  });
  assert.equal(editor.validation?.valid, false);
  assert.equal(editor.validation?.errors[0]?.code, 'TEMPLATE_IO_ERROR');
  assert.equal(editor.validation?.errors[0]?.messageKey, 'templates.errors.io');
  act(() => renderer.unmount());
});
