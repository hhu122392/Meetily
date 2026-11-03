import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';

const invoke = mock(async (_command: string, _args?: unknown): Promise<unknown> => []);
const listeners = new Map<string, (event: any) => void>();
mock.module('@tauri-apps/api/core', () => ({ invoke }));
mock.module('@tauri-apps/api/event', () => ({
  listen: async (name: string, callback: (event: any) => void) => {
    listeners.set(name, callback);
    return () => listeners.delete(name);
  },
}));
const refetchMeetings = () => {};
mock.module('../src/components/Sidebar/SidebarProvider', () => ({ useSidebar: () => ({ refetchMeetings }) }));
const { RecordingQueue } = await import('../src/components/RecordingQueue');
let renderer: ReactTestRenderer;
const job = (status: string) => ({ meeting_id: 'one', title: '262701530-1-208', status, detail: '', progress: 46 });
const button = (label: string) => renderer.root.findAllByType('button').find(b => b.children.includes(label))!;
afterEach(async () => {
  if (renderer) await act(async () => renderer.unmount());
  invoke.mockReset(); listeners.clear();
});

test('取消指定任务后等待清理，不提供重复取消或继续，已取消不计入未完成数', async () => {
  let status = 'transcribing';
  invoke.mockImplementation(async command => {
    if (command === 'cancel_recording_job') status = 'cancelling';
    return command === 'get_recording_jobs' ? [job(status)] : null;
  });
  await act(async () => { renderer = create(<RecordingQueue />); });
  await act(async () => { await button('取消任务').props.onClick(); });
  expect(invoke.mock.calls).toContainEqual(['cancel_recording_job', { meetingId: 'one' }]);
  expect(JSON.stringify(renderer.toJSON())).toContain('正在取消');
  expect(button('取消任务')).toBeUndefined();
  expect(button('继续任务')).toBeUndefined();
  status = 'cancelled';
  await act(async () => { listeners.get('recording-queue-changed')!({}); });
  expect(JSON.stringify(renderer.toJSON())).toContain('已取消');
  expect(JSON.stringify(renderer.toJSON())).not.toContain('1 项未完成');
  expect(button('重试处理')).toBeUndefined();
});

test('失败、已暂停及正在总结的任务均可取消，录音中和已完成不提供取消', async () => {
  let status = 'failed';
  invoke.mockImplementation(async () => [job(status)]);
  await act(async () => { renderer = create(<RecordingQueue />); });
  for (const next of ['failed', 'paused', 'summarizing', 'importing', 'waiting_cloud']) {
    status = next;
    await act(async () => { listeners.get('recording-queue-changed')!({}); });
    expect(button('取消任务')).toBeDefined();
  }
  for (const next of ['capturing', 'completed', 'cancelled']) {
    status = next;
    await act(async () => { listeners.get('recording-queue-changed')!({}); });
    expect(button('取消任务')).toBeUndefined();
  }
});

test('云录音等待和下载显示来源并允许暂停', async () => {
  let status = 'waiting_cloud';
  invoke.mockImplementation(async command => {
    if (command === 'pause_recording_job') status = 'pausing';
    return command === 'get_recording_jobs' ? [job(status)] : null;
  });
  await act(async () => { renderer = create(<RecordingQueue />); });
  expect(JSON.stringify(renderer.toJSON())).toContain('等待云录音');
  expect(button('暂停任务')).toBeDefined();
  status = 'downloading_cloud';
  await act(async () => { listeners.get('recording-queue-changed')!({}); });
  expect(JSON.stringify(renderer.toJSON())).toContain('正在检查或下载云录音');
  await act(async () => { await button('暂停任务').props.onClick(); });
  expect(button('继续任务')).toBeUndefined();
});

test('暂停请求指定会议，暂停完成前不能继续；已暂停状态保留百分比并允许恢复', async () => {
  let status = 'transcribing';
  invoke.mockImplementation(async command => {
    if (command === 'pause_recording_job') status = 'pausing';
    if (command === 'resume_recording_job') status = 'queued';
    return command === 'get_recording_jobs' ? [job(status)] : null;
  });
  await act(async () => { renderer = create(<RecordingQueue />); });
  await act(async () => { await button('暂停任务').props.onClick(); });
  expect(invoke.mock.calls).toContainEqual(['pause_recording_job', { meetingId: 'one' }]);
  expect(button('继续任务')).toBeUndefined();
  expect(JSON.stringify(renderer.toJSON())).toContain('正在暂停');
  status = 'paused';
  await act(async () => { listeners.get('recording-queue-changed')!({}); });
  expect(JSON.stringify(renderer.toJSON())).toContain('46%');
  await act(async () => { await button('继续任务').props.onClick(); });
  expect(invoke.mock.calls).toContainEqual(['resume_recording_job', { meetingId: 'one' }]);
});

test('失败显示实际原因，生成纪要没有错误的暂停入口', async () => {
  invoke.mockImplementation(async command => {
    if (command === 'pause_recording_job') throw '状态已变化';
    return [job('transcribing')];
  });
  await act(async () => { renderer = create(<RecordingQueue />); });
  await act(async () => { await button('暂停任务').props.onClick(); });
  expect(JSON.stringify(renderer.toJSON())).toContain('状态已变化');
  invoke.mockImplementation(async () => [job('summarizing')]);
  await act(async () => { listeners.get('recording-queue-changed')!({}); });
  expect(button('暂停任务')).toBeUndefined();
  expect(JSON.stringify(renderer.toJSON())).toContain('纪要生成已开始');
});
