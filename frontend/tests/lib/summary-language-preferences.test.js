import assert from 'node:assert/strict';
import { beforeEach, describe, test } from 'node:test';

const invokeResponses = [];

async function invokeMock() {
  if (invokeResponses.length === 0) {
    throw new Error('Unexpected Tauri invoke call');
  }
  return invokeResponses.shift();
}

function queueInvokeResponse(value) {
  invokeResponses.push(value);
}

function installLocalStorage() {
  const values = new Map();

  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      __TAURI_INTERNALS__: { invoke: invokeMock },
      localStorage: {
        getItem: (key) => values.get(key) ?? null,
        setItem: (key, value) => {
          values.set(key, value);
        },
        removeItem: (key) => {
          values.delete(key);
        },
        clear: () => {
          values.clear();
        },
      },
    },
  });

  return values;
}

function installFailingLocalStorage() {
  Object.defineProperty(globalThis, 'window', {
    configurable: true,
    value: {
      __TAURI_INTERNALS__: { invoke: invokeMock },
      localStorage: {
        getItem: () => null,
        setItem: () => {
          throw new Error('quota exceeded');
        },
        removeItem: () => {},
        clear: () => {},
      },
    },
  });
}

describe('summary language local fallback', () => {
  let storageValues;

  beforeEach(() => {
    invokeResponses.length = 0;
    storageValues = installLocalStorage();
  });

  test('reads summary language from local fallback when meeting has no folder', async () => {
    const prefs = await import('../../src/lib/summary-language-preferences');
    storageValues.set('summaryLanguageFallback:meeting-1', 'fr');
    queueInvokeResponse({
      language: null,
      storage: 'local_fallback',
    });

    assert.deepEqual(await prefs.readMeetingSummaryLanguage('meeting-1'), {
      language: 'fr',
      storage: 'local_fallback',
    });
  });

  test('saves summary language locally when command reports no folder', async () => {
    const prefs = await import('../../src/lib/summary-language-preferences');
    queueInvokeResponse({
      language: null,
      storage: 'local_fallback',
    });

    assert.deepEqual(await prefs.saveMeetingSummaryLanguage('meeting-1', 'es'), {
      language: 'es',
      storage: 'local_fallback',
    });
    assert.equal(storageValues.get('summaryLanguageFallback:meeting-1'), 'es');
  });

  test('clears local fallback when Auto is saved for a folderless meeting', async () => {
    const prefs = await import('../../src/lib/summary-language-preferences');
    storageValues.set('summaryLanguageFallback:meeting-1', 'de');
    queueInvokeResponse({
      language: null,
      storage: 'local_fallback',
    });

    assert.deepEqual(await prefs.saveMeetingSummaryLanguage('meeting-1', null), {
      language: null,
      storage: 'local_fallback',
    });
    assert.equal(storageValues.has('summaryLanguageFallback:meeting-1'), false);
  });

  test('caches detected language locally when meeting has no folder', async () => {
    const prefs = await import('../../src/lib/summary-language-preferences');
    queueInvokeResponse({
      language: null,
      storage: 'local_fallback',
    });

    await prefs.saveCachedDetectedSummaryLanguage('meeting-1', 'pt');

    assert.equal(storageValues.get('detectedSummaryLanguageFallback:meeting-1'), 'pt');
  });

  test('rejects when folderless summary language cannot be persisted locally', async () => {
    const prefs = await import('../../src/lib/summary-language-preferences');
    installFailingLocalStorage();
    queueInvokeResponse({
      language: null,
      storage: 'local_fallback',
    });

    await assert.rejects(
      prefs.saveMeetingSummaryLanguage('meeting-1', 'it'),
      /Failed to save summary language on this device/,
    );
  });
});
