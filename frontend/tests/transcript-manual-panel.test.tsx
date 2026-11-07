import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
let received: any;
const invoke = mock(async () => ({}));
const refresh = mock(async () => {});
const notify = mock();
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('sonner', () => ({ toast: { success: notify, error: notify } }));
mock.module('../src/components/VirtualizedTranscriptView', () => ({ VirtualizedTranscriptView: (p: any) => { received = p; return null; } }));
mock.module('../src/components/TranscriptView', () => ({ TranscriptView: () => null }));
mock.module('../src/components/MeetingDetails/TranscriptButtonGroup', () => ({ TranscriptButtonGroup: () => null }));
mock.module('../src/components/MeetingDetails/TranscriptFileSyncNotice', () => ({ TranscriptFileSyncNotice: () => null }));
const { TranscriptPanel } = await import('../src/components/MeetingDetails/TranscriptPanel');
let view: ReactTestRenderer;
const props = { transcripts: [], customPrompt: '', onPromptChange() {}, onCopyTranscript() {}, onOpenMeetingFolder: async () => {}, isRecording: false, meetingId: 'meeting', onRefetchTranscripts: refresh };
afterEach(async () => { await act(async () => view?.unmount()); invoke.mockClear(); refresh.mockClear(); notify.mockClear(); });
test('保存调用本机命令并携带会议与原文，成功后刷新转写', async () => {
  // The event is only a local file-status notification; no real desktop writes occur.
  const originalWindow = globalThis.window;
  Object.defineProperty(globalThis, 'window', { configurable: true, writable: true, value: { dispatchEvent: mock() } });
  try {
    await act(async () => { view = create(<TranscriptPanel {...props} />); });
    await act(async () => received.onUpdateSegment('segment', '新文本', '旧文本'));
    expect(invoke).toHaveBeenCalledWith('api_update_transcript_segment', { meetingId: 'meeting', transcriptId: 'segment', text: '新文本', expectedText: '旧文本' });
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledTimes(1);
  } finally { Object.defineProperty(globalThis, 'window', { configurable: true, writable: true, value: originalWindow }); }
});
test('录音期间不向转写列表提供编辑回调', async () => {
  await act(async () => { view = create(<TranscriptPanel {...props} isRecording />); });
  expect(received.onUpdateSegment).toBeUndefined();
});
