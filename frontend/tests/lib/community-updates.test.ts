import assert from 'node:assert/strict';
import { afterEach, beforeEach, test } from 'node:test';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { UpdateService } from '../../src/services/updateService';

let calls: Array<{ command: string; args: unknown }>;
const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
beforeEach(() => {
  Object.defineProperty(globalThis, 'window', { value: {}, configurable: true, writable: true });
  calls = [];
  mockIPC((command, args) => {
    calls.push({ command, args });
    if (command === 'plugin:app|version') return '0.4.2';
    if (command === 'open_external_url') return null;
    throw new Error(`Unexpected network/update command: ${command}`);
  });
});
afterEach(() => {
  clearMocks();
  if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow);
  else Reflect.deleteProperty(globalThis, 'window');
});

test('startup does not contact an updater or open a browser', async () => {
  const result = await new UpdateService().checkForUpdates(false);
  assert.equal(result.manual, true);
  assert.equal(result.available, false);
  assert.deepEqual(calls.map(x => x.command), ['plugin:app|version']);
});

test('manual update action opens this repository without claiming latest version', async () => {
  const result = await new UpdateService().checkForUpdates(true);
  assert.equal(result.manual, true);
  assert.equal(result.currentVersion, '0.4.2');
  assert.deepEqual(calls[1], {
    command: 'open_external_url',
    args: { url: 'https://github.com/hhu122392/Meetily/releases' },
  });
});

test('browser errors remain visible to the caller', async () => {
  mockIPC(command => {
    if (command === 'plugin:app|version') return '0.4.2';
    throw new Error('browser unavailable');
  });
  await assert.rejects(new UpdateService().checkForUpdates(true), /browser unavailable/);
});

test('a stale update object cannot install another distribution', async () => {
  let downloaded = false;
  const update = { version: '9.0.0', download: async () => { downloaded = true; } };
  await assert.rejects(new UpdateService().downloadAndInstall(update as never), /MANUAL_UPDATE_REQUIRED/);
  assert.equal(downloaded, false);
});
