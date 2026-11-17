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

test('底部背景可明确保存，离开输入框自动保存，并显示未保存状态',async()=>{
  const save=mock(async()=>{}),change=mock();
  await act(async()=>{view=create(<TranscriptPanel {...props} usePagination segments={[{id:'s',text:'会议原文',timestamp:0}]} customPrompt="测试背景" onPromptChange={change} onSavePrompt={save} promptReady promptDirty/>);});
  const input=view.root.findByProps({'aria-label':'会议背景'});
  expect(input.props.value).toBe('测试背景');expect(input.props.disabled).toBe(false);
  expect(JSON.stringify(view.toJSON())).toContain('未保存');
  await act(async()=>input.props.onChange({target:{value:'补充背景'}}));expect(change).toHaveBeenCalledWith('补充背景');
  await act(async()=>input.props.onBlur());expect(save).toHaveBeenCalledTimes(1);
  const button=view.root.findAllByType('button').find(b=>b.children.includes('保存会议背景'))!;
  await act(async()=>button.props.onClick());expect(save).toHaveBeenCalledTimes(2);
  expect(invoke).not.toHaveBeenCalled();
});

test('加载或保存中禁用背景输入，已保存时失焦不重复保存，错误直接显示在底部',async()=>{
  const save=mock(async()=>{});const base={...props,usePagination:true,segments:[{id:'s',text:'原文',timestamp:0}],onSavePrompt:save};
  await act(async()=>{view=create(<TranscriptPanel {...base} promptReady={false}/>);});
  expect(view.root.findByProps({'aria-label':'会议背景'}).props.disabled).toBe(true);
  await act(async()=>view.root.findByProps({'aria-label':'会议背景'}).props.onBlur());expect(save).not.toHaveBeenCalled();
  await act(async()=>view.update(<TranscriptPanel {...base} promptReady promptSaving promptDirty/>));
  expect(view.root.findByProps({'aria-label':'会议背景'}).props.disabled).toBe(true);expect(JSON.stringify(view.toJSON())).toContain('保存中');
  await act(async()=>view.update(<TranscriptPanel {...base} promptReady/>));await act(async()=>view.root.findByProps({'aria-label':'会议背景'}).props.onBlur());expect(save).not.toHaveBeenCalled();
  await act(async()=>view.update(<TranscriptPanel {...base} promptReady promptDirty promptError="出席和缺席名单包含同一人"/>));
  expect(view.root.findByProps({role:'alert'}).children.join('')).toContain('出席和缺席名单');expect(JSON.stringify(view.toJSON())).toContain('保存失败');
});
