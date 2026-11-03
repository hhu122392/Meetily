import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { UseImportAudioReturn } from '../src/hooks/useImportAudio';

const invoke = mock(async (_command: string, _args?: unknown): Promise<unknown> => ({}));
const listeners = new Map<string, (e: any) => Promise<void> | void>();
const completed = mock(() => {});
const applyLanguage = mock(async () => null);
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('@tauri-apps/api/event', () => ({ listen: async (name: string, cb: any) => { listeners.set(name, cb); return () => listeners.delete(name); } }));
mock.module('../src/lib/analytics', () => ({ default: { track: async () => {}, trackError: async () => {} } }));
mock.module('../src/lib/summary-language-preferences', () => ({ applyPinnedSummaryLanguageToMeeting: applyLanguage }));
mock.module('sonner', () => ({ toast: { warning: mock(() => {}) } }));
const { useImportAudio } = await import('../src/hooks/useImportAudio');
let value: UseImportAudioReturn;
let renderer: ReactTestRenderer;
function Harness() { value=useImportAudio({ onComplete:completed }); return null; }
afterEach(async () => { if(renderer) await act(async()=>renderer.unmount()); invoke.mockClear(); completed.mockClear(); applyLanguage.mockClear(); listeners.clear(); });

test('导入提交只复制入队，防重复点击，收到排队回执后可继续导入', async () => {
  await act(async()=>{ renderer=create(<Harness/>); });
  await act(async()=>{ await Promise.all([value.startImport('D:/one.wav','一','zh','small','whisper'),value.startImport('D:/one.wav','一','zh','small','whisper')]); });
  expect(invoke.mock.calls.filter(c=>c[0]==='start_import_audio_command')).toHaveLength(1);
  expect(value.isProcessing).toBe(true);
  const receipt={ meeting_id:'one',title:'一',segments_count:0,duration_seconds:0,queued:true };
  await act(async()=>{ await listeners.get('import-complete')!({payload:receipt}); });
  expect(value.isProcessing).toBe(false);
  expect(completed).toHaveBeenCalledWith(receipt);
  expect(applyLanguage).toHaveBeenCalledWith('one');
  await act(async()=>{ await value.startImport('D:/two.wav','二','en','small','whisper'); });
  expect(invoke.mock.calls.filter(c=>c[0]==='start_import_audio_command')).toHaveLength(2);
});
test('取消与最终入队同时发生时仍显示已保存任务，未提交的弹窗不消费其他导入事件', async () => {
  await act(async()=>{ renderer=create(<Harness/>); });
  const receipt={meeting_id:'one',title:'一',segments_count:0,duration_seconds:0,queued:true};
  await act(async()=>{ await listeners.get('import-complete')!({payload:receipt}); });
  expect(completed).not.toHaveBeenCalled();
  await act(async()=>{ await value.startImport('D:/one.wav','一'); });
  await act(async()=>{ await value.cancelImport(); });
  await act(async()=>{ await listeners.get('import-complete')!({payload:receipt}); });
  expect(completed).toHaveBeenCalledTimes(1);
  expect(value.status).toBe('complete');
});
