import { afterAll, afterEach, beforeEach, expect, jest, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { testInvoke as invoke, setTestInvoke, restoreTestInvoke } from './lib/tauri-test-invoke';
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
let respond: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
beforeEach(() => { setTestInvoke((command, args) => respond(command, args)); });
const { LarkUpcomingMeetings, calendarTime } = await import('../src/components/LarkUpcomingMeetings');
const { CalendarMeetingInfo } = await import('../src/components/MeetingDetails/CalendarMeetingInfo');
const { RecordingService } = await import('../src/services/recordingService');
const { CalendarRefreshSettings } = await import('../src/components/CalendarRefreshSettings');
const { Switch } = await import('../src/components/ui/switch');
let renderer: ReactTestRenderer | undefined;
const start = mock(async () => {}), settings = mock(() => {}), openMeeting = mock(() => {});
const event = { key: 'occurrence-one', revision: 'revision-one', title: '战略会', eventId: 'series_1', seriesId: 'series', start: 1794618000, end: 1794621600, allDay: false, meetingUrl: 'https://vc.larksuite.com/j/123456789' };
const connected = { configured: true, connected: true, accountKey: 'account', authRevision: 'login-one', syncedAt: 1794618000, events: [event] };
let account = 0;
beforeEach(() => {
  jest.useFakeTimers();
  connected.accountKey = `account-${++account}`;
  const storage = new Map<string, string>();
  const browser = Object.assign(new EventTarget(), {
    localStorage: { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value) },
  });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: browser });
});
const button = (text: string) => renderer!.root.findAllByType('button').find(b => b.children.filter(x => typeof x === 'string').join('').includes(text))!;
const content = () => JSON.stringify(renderer!.toJSON());
async function mount() {
  await act(async () => { renderer = create(<><LarkUpcomingMeetings disabled={false} onStart={start} onSettings={settings} onOpenMeeting={openMeeting} /><CalendarRefreshSettings /></>); });
}
async function advance(ms: number) { await act(async () => { jest.advanceTimersByTime(ms); }); }
async function automatic(enabled: boolean) { await act(async () => renderer!.root.findByType(Switch).props.onCheckedChange(enabled)); }
const syncCount = () => invoke.mock.calls.filter(c => c[0] === 'lark_calendar_sync').length;
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount()); renderer = undefined;
  jest.useRealTimers();
  invoke.mockClear(); start.mockClear(); settings.mockClear(); openMeeting.mockClear();
  if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow); else delete (globalThis as any).window;
});
afterAll(restoreTestInvoke);

test('默认启动同步一次，切回窗口和返回首页不重复请求，手动刷新仍可用', async () => {
  respond = async () => connected;
  await mount();
  expect(syncCount()).toBe(1);
  await advance(15 * 60000);
  await act(async () => { window.dispatchEvent(new Event('focus')); });
  expect(syncCount()).toBe(1);
  await act(async () => renderer!.unmount()); renderer = undefined;
  await mount();
  expect(syncCount()).toBe(1);
  await act(async () => button('刷新日程').props.onClick());
  expect(syncCount()).toBe(2);
});

test('自动刷新开关立即生效并记住选择，关闭后停止定时请求', async () => {
  respond = async () => connected;
  await mount(); await automatic(true);
  expect(window.localStorage.getItem('meetily.calendar.autoRefresh')).toBe('true');
  await advance(299999); expect(syncCount()).toBe(1);
  await advance(1); expect(syncCount()).toBe(2);
  await act(async () => renderer!.unmount()); renderer = undefined;
  await mount();
  expect(renderer!.root.findByType(Switch).props.checked).toBe(true);
  await advance(300000); expect(syncCount()).toBe(3);
  await automatic(false);
  await advance(600000); expect(syncCount()).toBe(3);
});

test('启动同步未结束时返回首页复用请求，完成后显示新日程', async () => {
  let finish!: (view: typeof connected) => void;
  const pending = new Promise<typeof connected>(resolve => { finish = resolve; });
  respond = async command => command === 'lark_calendar_sync' ? pending : { ...connected, events: [] };
  await mount();
  await act(async () => renderer!.unmount()); renderer = undefined;
  await mount(); expect(syncCount()).toBe(1);
  await act(async () => { finish(connected); });
  expect(content()).toContain('战略会');
  expect(content()).not.toContain('正在处理');
});

test('同步失败后暂停自动请求，返回首页仍暂停，手动成功后恢复', async () => {
  respond = async command => { if (command === 'lark_calendar_sync') throw '日历响应格式异常'; return connected; };
  await mount(); await automatic(true);
  await advance(600000); expect(syncCount()).toBe(1);
  expect(content()).toContain('自动刷新已暂停');
  await act(async () => renderer!.unmount()); renderer = undefined;
  await mount(); await advance(300000);
  expect(syncCount()).toBe(1); expect(content()).toContain('日历响应格式异常');
  respond = async () => connected;
  await act(async () => button('重新同步').props.onClick());
  await advance(300000); expect(syncCount()).toBe(3);
});

test('自动刷新关闭时首次授权成功仍同步一次，授权等待只检查本地状态', async () => {
  let authorized = false;
  respond = async command => command === 'lark_app_authorize' ? 'https://accounts.larksuite.com/authorize'
    : { ...connected, connected: authorized, authRevision: authorized ? 'login-two' : 'login-one' };
  await mount();
  await act(async () => button('连接我的 Lark 日历').props.onClick());
  await advance(3000); expect(syncCount()).toBe(0);
  authorized = true;
  await advance(3000); expect(syncCount()).toBe(1);
  await advance(300000); expect(syncCount()).toBe(1);
});

test('同步周期实例不会自动录音、申请云录音或发送消息', async () => {
  respond = async () => connected;
  await mount();
  expect(content()).toContain('战略会'); expect(content()).toContain('周期会议');
  expect(start).not.toHaveBeenCalled();
  expect(invoke.mock.calls.map(c => c[0])).toEqual(['lark_calendar_status', 'lark_calendar_sync']);
  await act(async () => button('开始本地录音').props.onClick());
  expect(start).toHaveBeenCalledWith(event);
});

test('连接日历只请求日历模式，不自动保存应用密钥', async () => {
  respond = async command => command === 'lark_calendar_status' ? { ...connected, connected: false, events: [] } : 'https://accounts.larksuite.com/authorize';
  await mount();
  await act(async () => button('连接我的 Lark 日历').props.onClick());
  expect(invoke.mock.calls.filter(c => c[0] === 'lark_app_authorize')).toEqual([['lark_app_authorize', { calendarOnly: true }]]);
  expect(start).not.toHaveBeenCalled();
  expect(content()).toContain('等待 Lark 授权');
});

test('同步失败保留标明过期的缓存，重新同步成功才能操作', async () => {
  respond = async command => { if (command === 'lark_calendar_status') return connected; throw '日历权限不足'; };
  await mount();
  expect(content()).toContain('战略会'); expect(content()).toContain('上次缓存'); expect(content()).toContain('日历权限不足');
  expect(button('开始本地录音').props.disabled).toBe(true);
  respond = async () => ({ ...connected, events: [{ ...event, title: '改期后的战略会', revision: 'revision-two' }] });
  await act(async () => button('重新同步').props.onClick());
  expect(content()).toContain('改期后的战略会');
  expect(button('开始本地录音').props.disabled).toBe(false);
});

test('取消同步清掉待开条目；切换账号不保留旧账号列表', async () => {
  respond = async () => connected;
  await mount();
  respond = async () => ({ ...connected, accountKey: 'another', events: [] });
  await act(async () => button('刷新日程').props.onClick());
  expect(content()).not.toContain('战略会'); expect(content()).toContain('暂无日程');
});

test('加入会议先核对日程链接，不启动录音；重复点击只启动一次', async () => {
  respond = async command => command === 'lark_calendar_link' ? event.meetingUrl : connected;
  await mount();
  await act(async () => button('加入会议').props.onClick());
  expect(invoke.mock.calls.find(c => c[0] === 'lark_calendar_link')?.[1]).toEqual({ selection: { key: event.key, revision: event.revision }, join: true });
  expect(start).not.toHaveBeenCalled();
  await act(async () => { const click = button('开始本地录音').props.onClick; click(); click(); });
  expect(start).toHaveBeenCalledTimes(1);
});

test('录音服务只传递日程实例及设备，服务端核对实际标题', async () => {
  respond = async () => undefined;
  await new RecordingService().startCalendarRecording(event, 'mic', 'speaker');
  expect(invoke.mock.calls).toEqual([['lark_calendar_start_recording', { selection: { key: event.key, revision: event.revision }, micDeviceName: 'mic', systemDeviceName: 'speaker' }]]);
});

test('邀请名单不自动变成出席名单，点击带入后再人工核对', async () => {
  respond = async () => ({ event, invitees: ['甲', '乙'], invitationNote: '' });
  const update = mock(() => {});
  await act(async () => { renderer = create(<CalendarMeetingInfo meetingId="local" disabled={false} update={update} />); });
  expect(update).not.toHaveBeenCalled(); expect(content()).toContain('邀请不代表实际出席');
  await act(async () => button('带入邀请名单并核对').props.onClick());
  expect(update).toHaveBeenCalledWith({ participants: '甲、乙', absent: '' });
  expect(calendarTime(0)).toContain('08:00');
});
