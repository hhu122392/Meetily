import { mock } from 'bun:test';
const originalCore = { ...await import('@tauri-apps/api/core') };
let respond: (command: string, args?: Record<string, unknown>) => Promise<unknown> = async () => undefined;
// One stable function avoids Bun retaining a different file's ESM mock binding.
export const testInvoke = mock((command: string, args?: Record<string, unknown>) => respond(command, args));
export function setTestInvoke(next: typeof respond) {
  respond = next;
  mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke: testInvoke }));
}
export function restoreTestInvoke() { mock.module('@tauri-apps/api/core', () => originalCore); }
setTestInvoke(respond);
