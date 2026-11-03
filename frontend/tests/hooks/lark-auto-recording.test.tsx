import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const originals = {
  core: { ...await import('@tauri-apps/api/core') },
  event: { ...await import('@tauri-apps/api/event') },
  stop: { ...await import('../../src/hooks/useRecordingStop') },
  state: { ...await import('../../src/contexts/RecordingStateContext') },
  toast: { ...await import('sonner') },
};
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originals.core);
  mock.module('@tauri-apps/api/event', () => originals.event);
  mock.module('../../src/hooks/useRecordingStop', () => originals.stop);
  mock.module('../../src/contexts/RecordingStateContext', () => originals.state);
  mock.module('sonner', () => originals.toast);
});

const handlers = new Map<string, (event: any) => void>();
const invoke = mock(async (_command: string, _args?: unknown) => false);
const listen = mock(async (name: string, handler: (event: any) => void) => {
  handlers.set(name, handler);
  return () => { if (handlers.get(name) === handler) handlers.delete(name); };
});
let stop = mock(async (_save: boolean) => {});
const setStatus = mock(() => {});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('@tauri-apps/api/event', () => ({ listen }));
mock.module('../../src/hooks/useRecordingStop', () => ({ useRecordingStop: () => ({ handleRecordingStop: stop }) }));
mock.module('../../src/contexts/RecordingStateContext', () => ({
  useRecordingState: () => ({ setStatus }), RecordingStatus: { ERROR: 'error' },
}));
mock.module('sonner', () => ({ toast: { error() {}, info() {} } }));
const { RecordingPostProcessingProvider } = await import('../../src/contexts/RecordingPostProcessingProvider');
let renderer: ReactTestRenderer | undefined;
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
  handlers.clear(); invoke.mockClear(); listen.mockClear(); setStatus.mockClear();
});

test('enables automation only with listeners ready; rerenders keep one listener and use the latest stop handler', async () => {
  await act(async () => { renderer = create(<RecordingPostProcessingProvider>first</RecordingPostProcessingProvider>); });
  expect(handlers.has('recording-stop-complete')).toBe(true);
  expect(invoke.mock.calls).toContainEqual(['lark_auto_recording_ready', { ready: true }]);
  const previous = stop;
  stop = mock(async (_save: boolean) => {});
  await act(async () => renderer!.update(<RecordingPostProcessingProvider>next</RecordingPostProcessingProvider>));
  expect(listen.mock.calls.filter(c => c[0] === 'recording-stop-complete')).toHaveLength(1);
  await act(async () => handlers.get('recording-stop-complete')!({ payload: true }));
  expect(stop).toHaveBeenCalledWith(true);
  expect(previous).not.toHaveBeenCalled();
  await act(async () => renderer!.unmount());
  renderer = undefined;
  expect(handlers.size).toBe(0);
  expect(invoke.mock.calls.at(-1)).toEqual(['lark_auto_recording_ready', { ready: false }]);
});

test('late listener registration after unmount cannot enable automatic recording', async () => {
  let release!: () => void;
  listen.mockImplementationOnce(async (name, handler) => {
    await new Promise<void>(resolve => { release = resolve; });
    handlers.set(name, handler);
    return () => { handlers.delete(name); };
  });
  await act(async () => { renderer = create(<RecordingPostProcessingProvider>pending</RecordingPostProcessingProvider>); });
  await act(async () => renderer!.unmount());
  renderer = undefined;
  await act(async () => release());
  expect(handlers.size).toBe(0);
  expect(invoke.mock.calls).not.toContainEqual(['lark_auto_recording_ready', { ready: true }]);
});
