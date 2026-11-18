import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { useMeetingSummarySettings, type MeetingSummarySettings } from '../src/hooks/meeting-details/useMeetingSummarySettings';
const empty: MeetingSummarySettings = {templateId:'standard_meeting',customPrompt:'',participants:'',absent:'',host:''};
const saved = new Map<string, MeetingSummarySettings>();
let deferred: (() => void) | null = null;
let hold = false;
let failure = '';
let pendingRead:Promise<MeetingSummarySettings>|null=null;
const invoke = mock(async (command: string, args: any) => {
  if (command === 'meeting_summary_settings_read') return pendingRead??{...(saved.get(args.meetingId) ?? empty)};
  if (hold) await new Promise<void>(resolve => {deferred=resolve;});
  if (failure) throw Error(failure);
  saved.set(args.meetingId, {...args.settings});
});
mock.module('@tauri-apps/api/core', () => ({invoke}));
let state: ReturnType<typeof useMeetingSummarySettings>;
let renderer: ReactTestRenderer;
function View({id}: {id:string}) {state=useMeetingSummarySettings(id);return null;}
async function show(id:string) { await act(async () => { if (renderer) renderer.update(<View id={id}/>); else renderer=create(<View id={id}/>); }); }
afterEach(async () => { deferred?.(); await act(async () => renderer?.unmount());renderer=undefined as any;saved.clear();invoke.mockClear();hold=false;deferred=null;failure='';pendingRead=null; });
test('会议背景、人员与模板分别保存，切换会议不会串用', async () => {
  await show('a');
  await act(async () => state.update({templateId:'daily_standup',customPrompt:'会议A背景',participants:'小明',host:'小李'}));
  await act(async () => { await state.save(); });
  await show('b'); expect(state.settings).toEqual(empty);
  await show('a'); expect(state.settings.customPrompt).toBe('会议A背景');expect(state.settings.host).toBe('小李');
  expect(state.settings.templateId).toBe('daily_standup');expect(state.dirty).toBe(false);
});
test('自动回填期间手动修改与明确清空不会被异步结果覆盖',async()=>{
  await show('a');let resolve!:(s:MeetingSummarySettings)=>void;
  pendingRead=new Promise(r=>{resolve=r;});let request!:Promise<void>;
  await act(async()=>{request=state.refresh();});
  await act(async()=>state.update({participants:'',absent:'人工缺席'}));
  await act(async()=>{resolve({...empty,participants:'自动出席',absent:'自动缺席',attendance:{participantsManual:false,absentManual:false,evidence:[],conflicts:[]}});await request;});
  expect(state.settings.participants).toBe('');expect(state.settings.absent).toBe('人工缺席');expect(state.settings.attendance?.participantsManual).toBe(true);expect(state.dirty).toBe(true);
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

test('失焦与点击保存共用同一请求，后续修改按顺序写入且不被旧保存覆盖', async () => {
  await show('a');await act(async()=>state.update({customPrompt:'先保存的内容'}));hold=true;
  let first!:Promise<MeetingSummarySettings>,duplicate!:Promise<MeetingSummarySettings>,latest!:Promise<MeetingSummarySettings>;
  await act(async()=>{first=state.save();duplicate=state.save();});
  expect(first).toBe(duplicate);expect(invoke.mock.calls.filter(c=>c[0]==='meeting_summary_settings_save')).toHaveLength(1);
  await act(async()=>state.update({customPrompt:'最后一次编辑'}));
  await act(async()=>{latest=state.save();});
  expect(invoke.mock.calls.filter(c=>c[0]==='meeting_summary_settings_save')).toHaveLength(1);expect(state.saving).toBe(true);
  hold=false;await act(async()=>{deferred!();await first;await latest;});
  expect(saved.get('a')?.customPrompt).toBe('最后一次编辑');expect(state.dirty).toBe(false);expect(state.saving).toBe(false);
  await show('b');await show('a');expect(state.settings.customPrompt).toBe('最后一次编辑');
});

test('保存失败保留输入和未保存状态，显式重试成功后清除错误',async()=>{
  await show('a');await act(async()=>state.update({customPrompt:'需要保留的背景'}));failure='磁盘暂不可写';
  await act(async()=>{await state.save().catch(()=>{});});
  expect(state.settings.customPrompt).toBe('需要保留的背景');expect(state.dirty).toBe(true);expect(state.error).toContain('磁盘暂不可写');expect(state.saving).toBe(false);expect(saved.has('a')).toBe(false);
  failure='';await act(async()=>{await state.save();});expect(state.error).toBe('');expect(state.dirty).toBe(false);
});

test('离开页面后仍写入原会议，但不再继续启动生成',async()=>{
  await show('a');await act(async()=>state.update({customPrompt:'会议A背景'}));hold=true;let request!:Promise<unknown>,error='';
  await act(async()=>{request=state.save().catch(e=>{error=String(e);});});
  await act(async()=>renderer.unmount());renderer=undefined as any;hold=false;
  await act(async()=>{deferred!();await request;});expect(saved.get('a')?.customPrompt).toBe('会议A背景');expect(error).toContain('会议已切换');
});
