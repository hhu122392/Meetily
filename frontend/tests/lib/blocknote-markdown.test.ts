import assert from 'node:assert/strict';
import { afterEach, describe, test } from 'node:test';
import type { Block } from '@blocknote/core';

import { blocksToMarkdownSafely } from '../../src/lib/blocknote-markdown';

const originalConsoleError = console.error;

describe('blocksToMarkdownSafely', () => {
  afterEach(() => {
    console.error = originalConsoleError;
  });

  test('returns markdown when conversion succeeds', async () => {
    let conversionCalls = 0;
    const editor = {
      blocksToMarkdownLossy: async () => {
        conversionCalls += 1;
        return '# Summary';
      },
    };

    const result = await blocksToMarkdownSafely(editor, [] as Block[], {
      source: 'test-success',
    });

    assert.deepEqual(result, {
      markdown: '# Summary',
      ok: true,
    });
    assert.equal(conversionCalls, 1);
  });

  test('returns fallback markdown when conversion throws', async () => {
    const error = new Error('conversion failed');
    const editor = {
      blocksToMarkdownLossy: async () => {
        throw error;
      },
    };
    const consoleErrors: unknown[][] = [];
    console.error = (...args: unknown[]) => {
      consoleErrors.push(args);
    };

    const result = await blocksToMarkdownSafely(
      editor,
      [{ id: 'block-1' }] as unknown as Block[],
      {
        source: 'test-fallback',
        fallbackMarkdown: 'existing markdown',
      },
    );

    assert.deepEqual(result, {
      markdown: 'existing markdown',
      ok: false,
    });
    assert.equal(consoleErrors.length, 1);
    assert.deepEqual(consoleErrors[0], [
      'Failed to convert BlockNote blocks to markdown',
      {
        source: 'test-fallback',
        blocksCount: 1,
        error,
      },
    ]);
  });

  test('omits markdown when conversion throws without fallback', async () => {
    const editor = {
      blocksToMarkdownLossy: async () => {
        throw new Error('conversion failed');
      },
    };
    console.error = () => undefined;

    const result = await blocksToMarkdownSafely(editor, [] as Block[], {
      source: 'test-empty-fallback',
    });

    assert.deepEqual(result, {
      markdown: undefined,
      ok: false,
    });
  });
});
