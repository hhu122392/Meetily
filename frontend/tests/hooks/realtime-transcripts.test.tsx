import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { useState } from 'react';
import type { Transcript } from '../../src/types';

const handlers = new Map<string, (event: any) => void>();
const warning = mock(() => {});
mock.module('@tauri-apps/api/event', () => ({ listen: async (name: string, handler: any) => {
  handlers.set(name, handler);
  return () => { handlers.delete(name); };
} }));
mock.module('@tauri-apps/api/core', () => ({ invoke: async () => null }));
mock.module('sonner', () => ({ toast: { warning } }));
const { useRealtimeTranscripts } = await import('../../src/hooks/useRealtimeTranscripts');
let view: ReactTestRenderer;
let rows: Transcript[] = [];
function Harness() {
  const [value, setValue] = useState<Transcript[]>([]);
  useRealtimeTranscripts(setValue);
  rows = value;
  return null;
}
async function event(name: string, payload: unknown) {
  await act(async () => handlers.get(name)!({ payload }));
}
afterEach(async () => { await act(async () => view?.unmount()); warning.mockClear(); });

test('实时文字按录音隔离，离会后的旧结果不能串入下一场', async () => {
  await act(async () => { view = create(<Harness />); });
  await event('recording-started', { folder_path: 'first' });
  const transcript = { folder_path: 'first', sequence_id: 0, text: '第一场', audio_start_time: 1 };
  await event('realtime-transcript', transcript);
  await event('realtime-transcript', transcript);
  expect(rows.map(row => row.text)).toEqual(['第一场']);
  await event('recording-stopped', {});
  await event('realtime-transcript', { ...transcript, sequence_id: 1 });
  expect(rows).toHaveLength(1);
  await event('recording-started', { folder_path: 'second' });
  await event('realtime-transcript', transcript);
  expect(rows).toHaveLength(0);
  await event('realtime-transcript', { ...transcript, folder_path: 'second', text: '第二场' });
  expect(rows.map(row => row.text)).toEqual(['第二场']);
  await event('realtime-transcription-status', { folder_path: 'first', message: '旧错误' });
  expect(warning).not.toHaveBeenCalled();
  await event('realtime-transcription-status', { folder_path: 'second', message: '本场错误' });
  expect(warning).toHaveBeenCalledWith('本场错误');
  await act(async () => view.unmount());
  expect(handlers.size).toBe(0);
});
