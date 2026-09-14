import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import {
  countTemplateViews,
  filterDeletedTemplates,
  filterTemplates,
  shouldApplyTemplateResponse,
  stopTemplateCardActionPropagation,
  templateErrorToastId,
  templateTranslationKey,
} from '../../src/lib/template-library';
import { useTemplateLibrary } from '../../src/hooks/useTemplateLibrary';
import { resources } from '../../src/i18n/resources';
import type { TemplateService } from '../../src/services/templateService';
import type {
  DefaultTemplatePreference,
  ListTemplatesResponse,
  TemplateListItem,
  TemplatesDirectoryInfo,
} from '../../src/types/summary-template';

function item(overrides: Partial<TemplateListItem> = {}): TemplateListItem {
  return {
    id: 'standard_meeting',
    name: 'Standard Meeting Notes',
    description: 'General outcomes and actions',
    origin: 'builtin',
    schemaVersion: 2,
    version: 1,
    locale: 'en',
    tags: ['general'],
    sectionCount: 4,
    sourceType: 'manual',
    updatedAt: '2026-08-23T00:00:00Z',
    fileSha256: 'a'.repeat(64),
    semanticSha256: 'b'.repeat(64),
    isDefault: true,
    readOnly: true,
    overridesBuiltin: false,
    valid: true,
    validationSummary: { errorCount: 0, warningCount: 0 },
    ...overrides,
  };
}

const directory: TemplatesDirectoryInfo = {
  path: 'C:\\Users\\tester\\AppData\\Roaming\\Meetily\\templates',
  exists: true,
  writable: true,
  customTemplateCount: 1,
};

function response(templates: TemplateListItem[]): ListTemplatesResponse {
  return {
    templates,
    diagnostics: [],
    deletedTemplates: [],
    defaultTemplateId: templates.find((template) => template.isDefault)?.id ?? null,
  };
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

function leafKeys(value: unknown, prefix = ''): string[] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [prefix];
  return Object.entries(value as Record<string, unknown>)
    .flatMap(([key, child]) => leafKeys(child, prefix ? `${prefix}.${key}` : key));
}

test('filters all/custom/built-in views and searches names, IDs, descriptions and tags', () => {
  const templates = [
    item(),
    item({
      id: 'customer_review',
      name: '客户需求评审',
      description: 'Scope and risks',
      origin: 'custom',
      tags: ['SOP', 'sales'],
      isDefault: false,
      readOnly: false,
    }),
    item({ id: 'daily_standup', name: 'Daily Standup', origin: 'bundled', isDefault: false }),
  ];

  assert.deepEqual(filterTemplates(templates, 'custom', '').map(({ id }) => id), ['customer_review']);
  assert.deepEqual(filterTemplates(templates, 'builtin', '').map(({ id }) => id), ['standard_meeting', 'daily_standup']);
  assert.deepEqual(filterTemplates(templates, 'all', 'SALES').map(({ id }) => id), ['customer_review']);
  assert.deepEqual(filterTemplates(templates, 'all', 'daily_standup').map(({ id }) => id), ['daily_standup']);
  assert.deepEqual(filterTemplates(templates, 'all', 'scope').map(({ id }) => id), ['customer_review']);
});

test('keeps trash search separate and reports deterministic tab counts', () => {
  const templates = [item(), item({ id: 'custom', origin: 'custom', isDefault: false, readOnly: false })];
  const deleted = [{
    trashId: 'trash-1',
    originalTemplateId: 'weekly_sync',
    deletedAt: '2026-08-23T00:00:00Z',
    name: 'Weekly Sync',
    fileSha256: 'c'.repeat(64),
    valid: true,
    errorCode: null,
  }];

  assert.deepEqual(countTemplateViews(templates, deleted), { all: 2, custom: 1, builtin: 1, trash: 1 });
  assert.equal(filterDeletedTemplates(deleted, 'WEEKLY').length, 1);
  assert.equal(filterDeletedTemplates(deleted, 'missing').length, 0);
});

test('accepts only the latest asynchronous response sequence', () => {
  assert.equal(shouldApplyTemplateResponse(7, 7), true);
  assert.equal(shouldApplyTemplateResponse(6, 7), false);
  assert.equal(shouldApplyTemplateResponse(8, 7), false);
});

test('card actions block both default behavior and bubbling', () => {
  let prevented = 0;
  let stopped = 0;
  stopTemplateCardActionPropagation({
    preventDefault: () => { prevented += 1; },
    stopPropagation: () => { stopped += 1; },
  } as Pick<Event, 'preventDefault' | 'stopPropagation'>);

  assert.equal(prevented, 1);
  assert.equal(stopped, 1);
});

test('runtime messages map to frozen translation keys and unknown keys do not leak', () => {
  assert.equal(templateTranslationKey('templates.errors.conflict'), 'errors.conflict');
  assert.equal(templateTranslationKey('templates.validation.transportFieldNaming'), 'validation.transportFieldNaming');
  assert.equal(templateTranslationKey('absolute path: C:\\private'), 'errors.io');
  assert.equal(
    templateErrorToastId('delete', {
      code: 'TEMPLATE_CONFLICT',
      messageKey: 'templates.errors.conflict',
      retryable: true,
      debugId: 'debug-1',
    }),
    'template:delete:TEMPLATE_CONFLICT:debug-1',
  );
});

test('English and Simplified Chinese template resources have exact key parity', () => {
  assert.deepEqual(
    leafKeys(resources.en.templates).sort(),
    leafKeys(resources['zh-CN'].templates).sort(),
  );
});

test('hook ignores stale list responses that complete after a newer refresh', async () => {
  const first = deferred<ListTemplatesResponse>();
  const second = deferred<ListTemplatesResponse>();
  let listCalls = 0;
  const fakeService = {
    list: () => (++listCalls === 1 ? first.promise : second.promise),
    getDirectory: async () => directory,
  } as unknown as TemplateService;

  let library!: ReturnType<typeof useTemplateLibrary>;
  function Probe() {
    library = useTemplateLibrary(fakeService);
    return null;
  }

  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
  });

  let refreshPromise!: Promise<void>;
  await act(async () => {
    refreshPromise = library.refresh();
    await Promise.resolve();
  });
  second.resolve(response([item({ id: 'newer', name: 'Newer' })]));
  await act(async () => { await refreshPromise; });
  assert.equal(library.data?.templates[0].id, 'newer');

  first.resolve(response([item({ id: 'stale', name: 'Stale' })]));
  await act(async () => { await first.promise; await Promise.resolve(); });
  assert.equal(library.data?.templates[0].id, 'newer');

  act(() => renderer.unmount());
});

test('hook suppresses a same-tick duplicate mutation before React can rerender', async () => {
  const mutation = deferred<DefaultTemplatePreference>();
  let mutationCalls = 0;
  const fakeService = {
    list: async () => response([item()]),
    getDirectory: async () => directory,
    setDefault: () => {
      mutationCalls += 1;
      return mutation.promise;
    },
  } as unknown as TemplateService;

  let library!: ReturnType<typeof useTemplateLibrary>;
  function Probe() {
    library = useTemplateLibrary(fakeService);
    return null;
  }

  let renderer!: TestRenderer.ReactTestRenderer;
  await act(async () => {
    renderer = TestRenderer.create(<Probe />);
    await Promise.resolve();
    await Promise.resolve();
  });

  let firstMutation!: Promise<DefaultTemplatePreference | null>;
  let secondMutation!: Promise<DefaultTemplatePreference | null>;
  act(() => {
    firstMutation = library.setDefault('standard_meeting');
    secondMutation = library.setDefault('standard_meeting');
  });
  assert.equal(mutationCalls, 1);
  assert.equal(await secondMutation, null);

  mutation.resolve({
    templateId: 'standard_meeting',
    resolvedTemplateId: 'standard_meeting',
    resolutionSource: 'user_default',
  });
  await act(async () => { await firstMutation; });
  assert.equal(library.pendingOperations.size, 0);

  act(() => renderer.unmount());
});
