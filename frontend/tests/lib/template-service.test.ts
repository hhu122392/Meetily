import assert from 'node:assert/strict';
import test from 'node:test';

import { normalizeTemplateApiError } from '../../src/services/templateService';

test('preserves a structured backend error and its debug id', () => {
  const backendError = {
    code: 'TEMPLATE_CONFLICT',
    messageKey: 'templates.errors.conflict',
    params: { templateId: 'weekly_sync' },
    retryable: true,
    debugId: 'server-debug-id',
  };

  assert.equal(normalizeTemplateApiError(backendError), backendError);
});

test('normalizes unknown rejections without parsing or exposing their text', () => {
  const normalized = normalizeTemplateApiError(
    'permission denied at C:\\Users\\private\\templates',
  );

  assert.equal(normalized.code, 'TEMPLATE_IO_ERROR');
  assert.equal(normalized.messageKey, 'templates.errors.io');
  assert.equal(normalized.retryable, true);
  assert.ok(normalized.debugId.length > 10);
  assert.doesNotMatch(JSON.stringify(normalized), /permission denied|Users|private/i);
});

test('rejects malformed lookalike objects instead of trusting partial fields', () => {
  const normalized = normalizeTemplateApiError({
    code: 'TEMPLATE_NOT_FOUND',
    messageKey: 'templates.errors.notFound',
  });

  assert.equal(normalized.code, 'TEMPLATE_IO_ERROR');
  assert.equal(normalized.retryable, true);
});
