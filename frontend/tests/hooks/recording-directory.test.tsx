import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const originalCore = { ...await import('@tauri-apps/api/core') };
const originalConfig = { ...await import('../../src/contexts/ConfigContext') };
const originalState = { ...await import('../../src/contexts/RecordingStateContext') };
const originalDevices = { ...await import('../../src/components/DeviceSelection') };
const originalStore = { ...await import('@tauri-apps/plugin-store') };
const originalLang = { ...await import('../../src/i18n') };
const originalToast = { ...await import('sonner') };
const prefs = { save_folder: 'C:\\old', auto_save: true, file_format:'mp4', preferred_mic_device:null, preferred_system_device:null };
let chosen: string | null = 'D:\\会议录音';
let rejectSave = false;
const invoke = mock(async (command: string, args?: any): Promise<any> => {
  if (command === 'get_recording_preferences') return prefs;
  if (command === 'select_recording_folder') return chosen;
  if (command === 'migrate_recording_storage') { if (rejectSave) throw new Error('目录不可写'); return { directory: chosen, message: '已迁移' }; }
});
mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));
mock.module('../../src/contexts/ConfigContext', () => ({ ...originalConfig, useConfig: () => ({setSelectedDevices() {}}) }));
mock.module('../../src/contexts/RecordingStateContext', () => ({ ...originalState, useRecordingState: () => ({isRecording:false}) }));
mock.module('../../src/components/DeviceSelection', () => ({ ...originalDevices, DeviceSelection: () => null }));
mock.module('@tauri-apps/plugin-store', () => ({ ...originalStore, Store: {load: async () => ({get: async () => true})} }));
mock.module('../../src/i18n', () => ({ ...originalLang, t: (s: string) => s, useUiLanguage: () => 'zh' }));
mock.module('sonner', () => ({...originalToast, toast: {success() {}, error() {}}}));
mock.module('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
mock.module('../../src/components/ui/dialog', () => ({Dialog: () => null, DialogContent: () => null, DialogDescription: () => null, DialogHeader: () => null, DialogTitle: () => null}));
const { RecordingSettings } = await import('../../src/components/RecordingSettings');
let renderer: ReactTestRenderer | undefined;
afterEach(async () => { if (renderer) await act(async () => renderer!.unmount()); renderer=undefined; invoke.mockClear(); chosen='D:\\会议录音'; rejectSave=false; });
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('../../src/contexts/ConfigContext', () => originalConfig);
  mock.module('../../src/contexts/RecordingStateContext', () => originalState);
  mock.module('../../src/components/DeviceSelection', () => originalDevices);
  mock.module('@tauri-apps/plugin-store', () => originalStore);
  mock.module('../../src/i18n', () => originalLang);
  mock.module('sonner', () => originalToast);
});
async function choose() {
  await act(async () => { renderer = create(<RecordingSettings />); });
  const button = renderer!.root.findAllByType('button').find(b => b.children.includes('选择文件夹并迁移'))!;
  await act(async () => button.props.onClick());
}
test('selected directory is persisted and shown only after successful save', async () => {
  await choose();
  expect(invoke.mock.calls).toContainEqual(['migrate_recording_storage', {directory:chosen}]);
  expect(JSON.stringify(renderer!.toJSON())).toContain('会议录音');
});
test('canceling picker does not modify the saved directory', async () => {
  chosen=null; await choose();
  expect(invoke.mock.calls.some(c => c[0]==='migrate_recording_storage')).toBe(false);
});
test('unwritable destination keeps the previous displayed directory', async () => {
  rejectSave=true; await choose();
  expect(JSON.stringify(renderer!.toJSON())).not.toContain('D:\\\\会议录音');
  expect(JSON.stringify(renderer!.toJSON())).toContain('old');
});

