import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const originalCore = { ...await import('@tauri-apps/api/core') };
const originalEvent = { ...await import('@tauri-apps/api/event') };
const originalDialog = { ...await import('../src/components/ui/dialog') };
let selected: string | null = 'D:\\模型';
let failure = false;
const storage = {directory:'C:\\模型', summary_directory:'C:\\模型\\summary', alongside_recordings:'D:\\录音\\models', pending_cleanup:0};
const invoke = mock(async (command: string) => {
  if (command === 'get_model_storage') return storage;
  if (command === 'select_model_storage_folder') return selected;
  if (command === 'migrate_model_storage' && failure) throw Error('正在转写，请暂停后迁移');
});
mock.module('@tauri-apps/api/core', () => ({invoke}));
mock.module('@tauri-apps/api/event', () => ({listen: async () => () => {}}));
mock.module('../src/components/ui/dialog', () => ({
  Dialog: ({open, children}: any) => open ? <div>{children}</div> : null,
  DialogContent: ({children}: any) => <div>{children}</div>,
  DialogHeader: ({children}: any) => <div>{children}</div>,
  DialogTitle: ({children}: any) => <h2>{children}</h2>,
  DialogDescription: ({children}: any) => <p>{children}</p>,
}));
const {ModelStoragePicker} = await import('../src/components/ModelStoragePicker');
let renderer: ReactTestRenderer;
afterEach(async () => { await act(async () => renderer?.unmount()); invoke.mockClear(); selected='D:\\模型'; failure=false; });
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('@tauri-apps/api/event', () => originalEvent);
  mock.module('../src/components/ui/dialog', () => originalDialog);
});
async function start() { await act(async () => { renderer=create(<ModelStoragePicker/>); }); }
async function click(label: string) {
  const button=renderer.root.findAllByType('button').find(b => b.children.includes(label));
  expect(button).toBeDefined();
  await act(async () => button!.props.onClick());
}
test('cancelled folder selection never migrates models', async () => {
  selected=null; await start(); await click('选择模型文件夹');
  expect(invoke.mock.calls.some(c => c[0]==='migrate_model_storage')).toBe(false);
});
test('folder selection requires explicit migration confirmation', async () => {
  await start(); await click('选择模型文件夹');
  expect(invoke.mock.calls.some(c => c[0]==='migrate_model_storage')).toBe(false);
  await click('取消');
  expect(invoke.mock.calls.some(c => c[0]==='migrate_model_storage')).toBe(false);
});
test('alongside recordings uses the shared models subdirectory', async () => {
  await start(); await click('与录音放在同一目录'); await click('确认迁移并重启');
  expect(invoke.mock.calls).toContainEqual(['migrate_model_storage',{directory:storage.alongside_recordings}]);
  expect(JSON.stringify(renderer.toJSON())).toContain('正在重启 Meetily');
  expect(renderer.root.findAllByType('button').every(b=>b.props.disabled)).toBe(true);
});
test('busy backend retains original path and permits retry after error', async () => {
  failure=true; await start(); await click('选择模型文件夹'); await click('确认迁移并重启');
  expect(JSON.stringify(renderer.toJSON())).toContain('C:\\\\模型');
  expect(renderer.root.findByProps({role:'alert'}).children.join('')).toContain('正在转写');
  failure=false; await click('确认迁移并重启');
  expect(invoke.mock.calls.filter(c=>c[0]==='migrate_model_storage')).toHaveLength(2);
});
