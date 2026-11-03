import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

let available = false;
let failDownload = false;
let pendingDownload: (() => void) | undefined;
let holdDownload = false;
const handlers = new Map<string, Set<(event: any) => void>>();
const storage = new Map<string, string>();
Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
  getItem: (key: string) => storage.get(key) ?? null,
  setItem: (key: string, value: string) => storage.set(key, value),
} });
Object.defineProperty(globalThis, 'window', { configurable: true, value: { location: { reload: mock(() => {}) } } });
const invoke = mock(async (command: string, args?: any): Promise<any> => {
  if (command === 'get_onboarding_status') return { completed: false, current_step: 3, model_status: { summary: 'pending' } };
  if (command === 'check_first_launch') return false;
  if (command === 'builtin_ai_get_recommended_model') return 'qwen3.5:2b';
  if (command === 'builtin_ai_is_model_ready') return false;
  if (command === 'sensevoice_get_available_models') return [{ name: 'sensevoice-small-int8', status: available ? 'Available' : 'Missing' }];
  if (command === 'sensevoice_has_available_models') return available;
  if (command === 'sensevoice_download_model') {
    if (failDownload) throw new Error('测试下载失败');
    if (holdDownload) { await new Promise<void>(resolve => { pendingDownload = resolve; }); return; }
    available = true;
    handlers.get('sensevoice-model-download-complete')?.forEach(handler => handler({ payload: { modelName: args.modelName } }));
  }
  if (command === 'sensevoice_cancel_download') {
    pendingDownload?.(); pendingDownload = undefined;
    handlers.get('sensevoice-model-download-progress')?.forEach(handler => handler({ payload: { modelName: 'sensevoice-small-int8', status: 'cancelled', progress: 0 } }));
    return 'cancelled';
  }
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('@tauri-apps/api/event', () => ({ listen: async (name: string, handler: any) => {
  const set = handlers.get(name) ?? new Set(); set.add(handler); handlers.set(name, set);
  return () => set.delete(handler);
} }));
mock.module('@tauri-apps/plugin-os', () => ({ platform: () => 'windows' }));
mock.module('sonner', () => ({ toast: { error: mock(() => {}), info: mock(() => {}) } }));
mock.module('../src/components/ui/button', () => ({ Button: (props: any) => <button {...props} /> }));
mock.module('../src/components/onboarding/OnboardingContainer', () => ({ OnboardingContainer: ({ children }: any) => <div>{children}</div> }));
const { OnboardingProvider } = await import('../src/contexts/OnboardingContext');
const { DownloadProgressStep } = await import('../src/components/onboarding/steps/DownloadProgressStep');
let renderer: ReactTestRenderer;
async function start() { await act(async () => { renderer = create(<OnboardingProvider><DownloadProgressStep /></OnboardingProvider>); }); }
async function click(label: string) {
  const button = renderer.root.findAllByType('button').find(button => button.children.includes(label));
  expect(button).toBeDefined(); expect(button!.props.disabled).toBeFalsy();
  await act(async () => { await button!.props.onClick(); });
}
afterEach(async () => {
  pendingDownload?.(); pendingDownload = undefined;
  await act(async () => renderer?.unmount()); invoke.mockClear(); storage.clear(); handlers.clear(); available = false; failDownload = false; holdDownload = false;
});

test('首次设置不发起下载，用户选择来源后只下载 SenseVoice，完成设置不下载纪要', async () => {
  await start();
  expect(invoke.mock.calls.some(([command]) => command.includes('download_model'))).toBe(false);
  const select = renderer.root.findByProps({ 'aria-label': 'SenseVoice 下载来源' });
  expect(select.findAllByType('option')).toHaveLength(4);
  await act(async () => select.props.onChange({ target: { value: 'modelscope' } }));
  await click('下载 SenseVoice');
  expect(invoke.mock.calls.filter(([command]) => command === 'sensevoice_download_model')).toEqual([
    ['sensevoice_download_model', { modelName: 'sensevoice-small-int8', source: 'modelscope' }],
  ]);
  await click('继续（不下载纪要模型）');
  expect(invoke.mock.calls.some(([command]) => command === 'complete_onboarding')).toBe(true);
  expect(invoke.mock.calls.some(([command]) => command === 'builtin_ai_download_model')).toBe(false);
});

test('下载失败后可切换来源重试，并记住新来源', async () => {
  failDownload = true; await start(); await click('下载 SenseVoice');
  await act(async () => renderer.root.findByProps({ 'aria-label': 'SenseVoice 下载来源' }).props.onChange({ target: { value: 'hf-mirror' } }));
  failDownload = false; await click('重试下载');
  const calls = invoke.mock.calls.filter(([command]) => command === 'sensevoice_download_model');
  expect(calls.map(([, args]) => args.source)).toEqual(['auto', 'hf-mirror']);
  expect(storage.get('sensevoice-download-source')).toBe('hf-mirror');
});

test('已安装转写模型可直接继续，缺少纪要模型不阻塞，也不后台下载', async () => {
  available = true; await start(); await click('继续（不下载纪要模型）');
  expect(invoke.mock.calls.some(([command]) => command.includes('download_model'))).toBe(false);
});

test('下载中禁用换源，取消后可选择新来源并继续下载', async () => {
  holdDownload = true; await start();
  await act(async () => {
    void renderer.root.findAllByType('button').find(button => button.children.includes('下载 SenseVoice'))!.props.onClick();
  });
  expect(renderer.root.findByProps({ 'aria-label': 'SenseVoice 下载来源' }).props.disabled).toBe(true);
  await click('取消下载');
  const select = renderer.root.findByProps({ 'aria-label': 'SenseVoice 下载来源' });
  expect(select.props.disabled).toBe(false);
  await act(async () => select.props.onChange({ target: { value: 'huggingface' } }));
  holdDownload = false; await click('下载 SenseVoice');
  const calls = invoke.mock.calls.filter(([command]) => command === 'sensevoice_download_model');
  expect(calls.map(([, args]) => args.source)).toEqual(['auto', 'huggingface']);
  expect(invoke.mock.calls.some(([command]) => command === 'builtin_ai_download_model')).toBe(false);
});
