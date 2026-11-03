import { afterEach, beforeEach, expect, jest, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const handlers = new Map<string, (event: any) => void>();
const custom = mock(() => 1);
const dismiss = mock(() => {});
let modelStatus: any = 'Available';
const invoke = mock(async () => [{ name: 'sensevoice-small-int8', status: modelStatus }]);
mock.module('@tauri-apps/api/event', () => ({ listen: async (name: string, handler: any) => {
  handlers.set(name, handler); return () => handlers.delete(name);
} }));
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('sonner', () => ({ toast: { custom, dismiss } }));
mock.module('../src/i18n', () => ({ t: (s: string) => s, useUiLanguage: () => 'zh-CN' }));
const { useDownloadProgressToast } = await import('../src/components/shared/DownloadProgressToast');
let state: ReturnType<typeof useDownloadProgressToast>;
function Harness() { state = useDownloadProgressToast(); return null; }
let renderer: ReactTestRenderer;
const modelName = 'sensevoice-small-int8';
const toastId = `download-${modelName}`;
async function progress(percent = 99, status = 'downloading') {
  await act(async () => handlers.get('sensevoice-model-download-progress')!({ payload: {
    modelName, progress: percent, status, downloaded_mb: 228.45, total_mb: 228.45,
  } }));
}
async function advance(ms: number) { await act(async () => { jest.advanceTimersByTime(ms); }); }
beforeEach(async () => {
  jest.useFakeTimers(); modelStatus = 'Available';
  await act(async () => { renderer = create(<Harness />); });
});
afterEach(async () => {
  await act(async () => renderer.unmount());
  jest.useRealTimers(); handlers.clear(); custom.mockClear(); dismiss.mockClear(); invoke.mockClear();
});

test('完成事件后主动清除浮窗，不只删除内部状态', async () => {
  await progress();
  await act(async () => handlers.get('sensevoice-model-download-complete')!({ payload: { modelName } }));
  expect(state.downloads.get(modelName)?.status).toBe('completed');
  await advance(4000);
  expect(dismiss).toHaveBeenCalledWith(toastId);
  expect(state.downloads.has(modelName)).toBe(false);
});
test('完成事件缺失时以本地模型就绪状态修正残留99%', async () => {
  await progress(); await advance(3000);
  expect(invoke).toHaveBeenCalledWith('sensevoice_get_available_models');
  expect(state.downloads.get(modelName)?.status).toBe('completed');
  expect(state.downloads.get(modelName)?.progress).toBe(100);
  await advance(4000); expect(dismiss).toHaveBeenCalledWith(toastId);
});
test('字节数已满但模型仍在校验时不伪造下载完成', async () => {
  modelStatus = { Downloading: { progress: 99 } };
  await progress(); await advance(3000);
  expect(state.downloads.get(modelName)?.status).toBe('downloading');
  expect(dismiss).not.toHaveBeenCalled();
});
test('仅完成进度事件也能清理，无需等待独立完成事件', async () => {
  await progress(100, 'completed'); await advance(4000);
  expect(dismiss).toHaveBeenCalledWith(toastId);
  expect(state.downloads.has(modelName)).toBe(false);
});
test('失败后重试不会被上一次失败的延时清理关闭', async () => {
  await act(async () => handlers.get('sensevoice-model-download-error')!({ payload: { modelName, error: 'Network error' } }));
  await advance(5000); await progress(10); await advance(7000);
  expect(state.downloads.get(modelName)?.status).toBe('downloading');
  expect(dismiss).not.toHaveBeenCalled();
});
test('空闲时不查询模型，卸载后清除残留浮窗与计时器', async () => {
  await advance(6000); expect(invoke).not.toHaveBeenCalled();
  await progress(); await act(async () => renderer.unmount());
  expect(dismiss).toHaveBeenCalledWith(toastId);
  await advance(6000); expect(invoke).not.toHaveBeenCalled();
});

test('本机状态查询失败时保留下载状态，不误报成功', async () => {
  invoke.mockRejectedValueOnce(new Error('IPC unavailable'));
  await progress(); await advance(3000);
  expect(state.downloads.get(modelName)?.status).toBe('downloading');
  expect(dismiss).not.toHaveBeenCalled();
});
test('查询期间已取消下载时，迟到的就绪结果不能覆盖取消状态', async () => {
  let resolve!: (value: any) => void;
  invoke.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
  await progress(); await advance(3000);
  await progress(0, 'cancelled');
  await act(async () => { resolve([{ name: modelName, status: 'Available' }]); });
  expect(state.downloads.get(modelName)?.status).toBe('cancelled');
  await advance(6000); expect(dismiss).toHaveBeenCalledWith(toastId);
});
