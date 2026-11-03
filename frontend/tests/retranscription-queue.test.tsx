import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';

const invoke = mock(async (_command: string, _args?: unknown): Promise<unknown> => ({}));
const success = mock(() => {});
const closed = mock(() => {});
const completed = mock(() => {});
const listeners = new Map<string, (event: any) => void>();
const model = { provider: 'whisper', name: 'small', displayName: 'Small', size_mb: 465 };
const fetchModels = mock(() => {});
const resetSelection = mock(() => {});
let available = true;
let savedLanguage = 'zh';
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('@tauri-apps/api/event', () => ({
  listen: async (name: string, callback: (event: any) => void) => {
    listeners.set(name, callback);
    return () => { listeners.delete(name); };
  },
}));
mock.module('sonner', () => ({ toast: { success } }));
mock.module('../src/contexts/ConfigContext', () => ({ useConfig: () => ({ selectedLanguage: savedLanguage, transcriptModelConfig: undefined }) }));
mock.module('../src/hooks/useTranscriptionModels', () => ({
  useTranscriptionModels: () => ({
    availableModels: available ? [model] : [], selectedModelKey: `${model.provider}:${model.name}`,
    setSelectedModelKey: () => {}, loadingModels: false, fetchModels, resetSelection,
  }),
}));
mock.module('../src/lib/analytics', () => ({ default: { track: async () => {}, trackError: async () => {} } }));
mock.module('../src/i18n', () => ({ t: (s: string) => s, useUiLanguage: () => {} }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({
  Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) => open ? <div>{children}</div> : null,
  DialogContent: Container, DialogDescription: Container, DialogFooter: Container,
  DialogHeader: Container, DialogTitle: Container,
}));
mock.module('../src/components/ui/select', () => ({
  Select: Container, SelectContent: Container, SelectItem: Container, SelectTrigger: Container, SelectValue: Container,
}));
const { RetranscribeDialog } = await import('../src/components/MeetingDetails/RetranscribeDialog');
let renderer: ReactTestRenderer;
const props = { open: true, onOpenChange: closed, meetingId: 'one', meetingFolderPath: 'D:/录音/one', onComplete: completed };
const submit = () => renderer.root.findAllByType('button').find(b => b.children.some(c => typeof c === 'string' && c.includes('加入后台队列')))!;
afterEach(async () => {
  if (renderer) await act(async () => renderer.unmount());
  invoke.mockReset(); invoke.mockImplementation(async () => ({}));
  success.mockClear(); closed.mockClear(); completed.mockClear(); listeners.clear(); available = true;
  model.provider = 'whisper'; model.name = 'small';
  savedLanguage = 'zh';
});

test('粤语选择在 SenseVoice 中保留，切回 Whisper 时自动恢复自动识别', async () => {
  savedLanguage = 'yue'; model.provider = 'sensevoice'; model.name = 'sensevoice-small-int8';
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  await act(async () => { await submit().props.onClick(); });
  expect(invoke.mock.calls[0][1]).toMatchObject({ language: 'yue', provider: 'sensevoice' });
  await act(async () => { renderer.unmount(); });
  invoke.mockClear(); model.provider = 'whisper'; model.name = 'small';
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  await act(async () => { await submit().props.onClick(); });
  expect(invoke.mock.calls[0][1]).toMatchObject({ language: null, provider: 'whisper' });
});

test('SenseVoice 转写将模型和中文语言原样交给后台任务', async () => {
  model.provider = 'sensevoice'; model.name = 'sensevoice-small-int8';
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  await act(async () => { await submit().props.onClick(); });
  expect(invoke.mock.calls[0]).toEqual(['start_retranscription_command', {
    meetingId: 'one', meetingFolderPath: 'D:/录音/one', language: 'zh', model: 'sensevoice-small-int8', provider: 'sensevoice',
  }]);
});
test('提交持久化成功后即关闭弹窗，同步重复点击只创建一次任务', async () => {
  let resolve!: (value: unknown) => void;
  invoke.mockImplementation(() => new Promise(r => { resolve = r; }));
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  const click = submit().props.onClick;
  let pending!: Promise<void>;
  await act(async () => { pending = click(); void click(); });
  expect(invoke).toHaveBeenCalledTimes(1);
  expect(closed).not.toHaveBeenCalled();
  await act(async () => { resolve({ meeting_id: 'one' }); await pending; });
  expect(invoke.mock.calls[0]).toEqual(['start_retranscription_command', {
    meetingId: 'one', meetingFolderPath: 'D:/录音/one', language: 'zh', model: 'small', provider: 'whisper',
  }]);
  expect(closed).toHaveBeenCalledWith(false);
  expect(success).toHaveBeenCalledTimes(1);
});
test('弹窗关闭后仍刷新当前会议结果，其他会议事件不触发刷新', async () => {
  await act(async () => { renderer = create(<RetranscribeDialog {...props} open={false} />); });
  await act(async () => { listeners.get('retranscription-complete')!({ payload: { meeting_id: 'other' } }); });
  expect(completed).not.toHaveBeenCalled();
  await act(async () => { listeners.get('retranscription-complete')!({ payload: { meeting_id: 'one' } }); });
  expect(completed).toHaveBeenCalledTimes(1);
  expect(closed).not.toHaveBeenCalled();
});
test('入队失败保留弹窗并显示原因，不显示成功', async () => {
  invoke.mockImplementation(async () => { throw '这场会议已有排队任务'; });
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  await act(async () => { await submit().props.onClick(); });
  expect(closed).not.toHaveBeenCalled();
  expect(success).not.toHaveBeenCalled();
  expect(JSON.stringify(renderer.toJSON())).toContain('这场会议已有排队任务');
});
test('没有可用模型时不能提交', async () => {
  available = false;
  await act(async () => { renderer = create(<RetranscribeDialog {...props} />); });
  expect(submit().props.disabled).toBe(true);
});
