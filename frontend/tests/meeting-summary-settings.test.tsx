import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { useMeetingSummarySettings, type MeetingSummarySettings } from '../src/hooks/meeting-details/useMeetingSummarySettings';
const empty: MeetingSummarySettings = {templateId:'standard_meeting',customPrompt:'',participants:'',absent:'',host:''};
const saved = new Map<string, MeetingSummarySettings>();
let deferred: (() => void) | null = null;
let hold = false;
const invoke = mock(async (command: string, args: any) => {
  if (command === 'meeting_summary_settings_read') return {...(saved.get(args.meetingId) ?? empty)};
  if (hold) await new Promise<void>(resolve => {deferred=resolve;});
  saved.set(args.meetingId, {...args.settings});
});
mock.module('@tauri-apps/api/core', () => ({invoke}));
let state: ReturnType<typeof useMeetingSummarySettings>;
let renderer: ReactTestRenderer;
function View({id}: {id:string}) {state=useMeetingSummarySettings(id);return null;}
async function show(id:string) { await act(async () => { if (renderer) renderer.update(<View id={id}/>); else renderer=create(<View id={id}/>); }); }
afterEach(async () => { deferred?.(); await act(async () => renderer?.unmount());renderer=undefined as any;saved.clear();invoke.mockClear();hold=false;deferred=null; });
test('会议背景、人员与模板分别保存，切换会议不会串用', async () => {
  await show('a');
  await act(async () => state.update({templateId:'daily_standup',customPrompt:'会议A背景',participants:'小明',host:'小李'}));
  await act(async () => { await state.save(); });
  await show('b'); expect(state.settings).toEqual(empty);
  await show('a'); expect(state.settings.customPrompt).toBe('会议A背景');expect(state.settings.host).toBe('小李');
  expect(state.settings.templateId).toBe('daily_standup');expect(state.dirty).toBe(false);
});
test('保存期间后续编辑保留为未保存，生成拿到点击时的快照', async () => {
  await show('a'); await act(async () => state.update({customPrompt:'第一版'}));
  hold=true; let request!: Promise<MeetingSummarySettings>;
  await act(async () => { request=state.save(); });
  await act(async () => state.update({customPrompt:'第二版'}));
  await act(async () => {deferred!();await request;});
  expect((await request).customPrompt).toBe('第一版');expect(state.settings.customPrompt).toBe('第二版');expect(state.dirty).toBe(true);
});
test('保存时切换会议会阻止旧会议继续生成', async () => {
  await show('a');hold=true;let error='';let request!: Promise<unknown>;
  await act(async () => {request=state.save().catch(e => {error=String(e);});});
  await show('b'); await act(async () => {deferred!();await request;});
  expect(error).toContain('会议已切换');expect(state.settings).toEqual(empty);
});
