import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test';
import { useEffect, useState, type ReactNode } from 'react';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { LANGUAGE_STORAGE_KEY, setUiLanguage, t, useInitializeUiLanguage, useUiLanguage } from '../src/i18n';
import { legacySummaryText } from '../src/lib/lark';
import type { MeetingSummary } from '../src/types';

const originalCore = { ...await import('@tauri-apps/api/core') };
const originalDialog = { ...await import('../src/components/ui/dialog') };
let respond: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
const invoke = mock((command: string, args?: Record<string, unknown>) => respond(command, args));
mock.module('@tauri-apps/api/core', () => ({ ...originalCore, invoke }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({
  ...originalDialog,
  Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) => open ? <div>{children}</div> : null,
  DialogContent: Container, DialogDescription: Container, DialogHeader: Container, DialogTitle: Container,
}));
const { LarkSendButton } = await import('../src/components/MeetingDetails/LarkSendButton');
const { LarkBotSettings } = await import('../src/components/LarkBotSettings');
const { LarkAppSettings } = await import('../src/components/LarkAppSettings');
let renderer: ReactTestRenderer | undefined;
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
const originalDocument = Object.getOwnPropertyDescriptor(globalThis, 'document');
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
  if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow); else delete (globalThis as any).window;
  if (originalDocument) Object.defineProperty(globalThis, 'document', originalDocument); else delete (globalThis as any).document;
  setUiLanguage('zh-CN'); invoke.mockClear();
});
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('../src/components/ui/dialog', () => originalDialog);
});
const config = { settings: { target_name: '产品周会', webhook: 'https://open.larksuite.com/open-apis/bot/v2/hook/test', secret: '' }, revision: 'revision-a' };
const button = (label: string) => renderer!.root.findAllByType('button').find(node => node.props['aria-label'] === label || node.children.filter(child => typeof child === 'string').join('').includes(label))!;
const content = () => JSON.stringify(renderer!.toJSON());
test('会议助手只有一个授权入口，一次请求会议控制和云录音权限', async () => {
  respond = async command => command === 'lark_app_status'
    ? { app_id:'cli_test',has_secret:true,enabled:true,authorized:true, routes:[], deliveries:[], recording:null }
    : command === 'lark_app_authorize' ? 'https://accounts.larksuite.com/test' : null;
  await act(async () => { renderer=create(<LarkAppSettings />); });
  await act(async () => button('授权会议助手').props.onClick());
  expect(invoke.mock.calls.filter(call => call[0] === 'lark_app_authorize')).toEqual([
    ['lark_app_authorize',{includeMeetingControl:true,includeCloudRecording:true}],
  ]);
  expect(button('授权结束会议')).toBeUndefined();
  expect(button('授权云录音')).toBeUndefined();
  expect(button('重新登录 Lark')).toBeUndefined();
  expect(content()).toContain('当前主持人');
});
test('损坏的 Lark 配置显示真实错误并阻止保存和授权', async () => {
  respond = async () => { throw '凭证格式异常。'; };
  await act(async () => { renderer = create(<LarkAppSettings />); });
  expect(content()).toContain('凭证格式异常');
  expect(content()).toContain('配置读取失败');
  expect(content()).not.toContain('请确认运行的是新版 Meetily');
  expect(button('保存并连接').props.disabled).toBe(true);
  expect(button('授权会议助手').props.disabled).toBe(true);
  expect(invoke.mock.calls.every(call => call[0] === 'lark_app_status')).toBe(true);
});
async function mountSender(getSummary = async () => '## 讨论结论\n保留当前编辑内容') {
  await act(async () => { renderer = create(<LarkSendButton compact meetingId="meeting-a" title="产品例会" createdAt="2026-10-14T02:00:00Z" getSummary={getSummary} />); });
  await act(async () => button('Webhook 发送').props.onClick());
}

describe('Lark preview and sending', () => {
  test('previews live edits without sending; sends only the edited preview with the reviewed config revision', async () => {
    respond = async command => command === 'lark_get_config' ? config : { already_sent: false, sent_at: 'now' };
    await mountSender();
    expect(invoke.mock.calls.map(call => call[0])).toEqual(['lark_get_config']);
    const textarea = renderer!.root.findByType('textarea');
    expect(textarea.props.value).toContain('当前编辑内容');
    await act(async () => textarea.props.onChange({ target: { value: '确认后的中文纪要' } }));
    await act(async () => button('确认发送').props.onClick());
    expect(invoke.mock.calls[1]).toEqual(['lark_send_summary', expect.objectContaining({
      meetingId: 'meeting-a', title: '产品例会', summary: '确认后的中文纪要', expectedConfigRevision: 'revision-a',
    })]);
    expect(content()).toContain('纪要已成功发送');
  });
  test('rapid double clicks result in one request and duplicate receipts are shown clearly', async () => {
    let finish!: (result: unknown) => void;
    respond = async command => command === 'lark_get_config' ? config : new Promise(resolve => { finish = resolve; });
    await mountSender();
    const send = button('确认发送').props.onClick;
    await act(async () => { send(); send(); });
    expect(invoke.mock.calls.filter(call => call[0] === 'lark_send_summary')).toHaveLength(1);
    await act(async () => finish({ already_sent: true, sent_at: 'earlier' }));
    expect(content()).toContain('本次未重复发送');
  });
  test('empty editor exports cannot be sent', async () => {
    respond = async () => config;
    await mountSender(async () => '  ');
    expect(button('确认发送').props.disabled).toBe(true);
    expect(content()).toContain('无法准备发送内容');
    expect(invoke.mock.calls.every(call => call[0] === 'lark_get_config')).toBe(true);
  });
  test('business or network failures remain visible without automatic retry', async () => {
    respond = async command => {
      if (command === 'lark_get_config') return config;
      throw '目标群配置已改变，请重新预览。';
    };
    await mountSender();
    await act(async () => button('确认发送').props.onClick());
    expect(content()).toContain('目标群配置已改变');
    expect(content()).not.toContain('纪要已成功发送');
    expect(invoke.mock.calls.filter(call => call[0] === 'lark_send_summary')).toHaveLength(1);
  });
  test('saving settings never sends a group message and secrets are masked', async () => {
    respond = async command => command === 'lark_get_config' ? config : undefined;
    await act(async () => { renderer = create(<LarkBotSettings />); });
    expect(renderer!.root.findByProps({ id: 'lark-webhook' }).props.type).toBe('password');
    await act(async () => button('保存配置').props.onClick());
    expect(invoke.mock.calls.map(call => call[0])).toEqual(['lark_app_status', 'lark_get_config', 'lark_save_config']);
  });
  test('legacy sections retain explicit ordering and modern data cannot use stale fallback conversion', () => {
    const data = { _section_order: ['actions', 'decisions'], decisions: { title: '决策', blocks: [{ content: '发布' }] }, actions: { title: '待办', blocks: [{ content: '测试' }] } };
    expect(legacySummaryText(data as unknown as MeetingSummary)).toBe('## 待办\n\n- 测试\n\n## 决策\n\n- 发布');
    expect(legacySummaryText({ ...data, markdown: 'older' } as unknown as MeetingSummary)).toBe('');
  });
});

describe('Lark enterprise app configuration', () => {
  const appStatus = { app_id: 'cli_test', has_secret: true, enabled: true, auto_send: true, authorized: false, owner_id: '', connection: '未授权', routes: [], deliveries: [], recording: null };
  test('saving sends credentials only to the backend and clears the secret input without sending messages', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.type).toBe('password');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('********');
    await act(async () => button('更换密钥').props.onClick());
    await act(async () => renderer!.root.findByProps({ id: 'lark-app-secret' }).props.onChange({ target: { value: 'test-only-secret' } }));
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')).toEqual(['lark_app_save', { appId: 'cli_test', secret: 'test-only-secret', enabled: true, autoSend: true, autoRecord: false, allowGroupLaunch: false, recordingSource: 'cloud' }]);
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('********');
    expect(invoke.mock.calls.some(call => call[0].includes('send'))).toBe(false);
  });
  test('stored mask is never submitted as a replacement secret and refresh keeps login visible', async () => {
    respond = async command => command === 'lark_app_status' ? { ...appStatus, authorized: true, authorization_status: '已授权，支持自动续期' } : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(content()).toContain('已登录 · 授权可用');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.readOnly).toBe(true);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]?.secret).toBe('');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('********');
    expect(content()).toContain('已登录 · 授权可用');
    await act(async () => renderer!.root.findByProps({ id: 'lark-app-id' }).props.onChange({ target: { value: 'cli_other' } }));
    expect(content()).not.toContain('已登录 · 授权可用');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
  });
  test('status refresh preserves unsaved app ID and connection errors remain visible', async () => {
    respond = async command => { if (command === 'lark_app_status') return appStatus; throw '凭证无效'; };
    await act(async () => { renderer = create(<LarkAppSettings />); });
    await act(async () => renderer!.root.findByProps({ id: 'lark-app-id' }).props.onChange({ target: { value: 'cli_edited' } }));
    await act(async () => button('刷新状态').props.onClick());
    expect(renderer!.root.findByProps({ id: 'lark-app-id' }).props.value).toBe('cli_edited');
    await act(async () => button('保存并连接').props.onClick());
    expect(content()).toContain('凭证无效');
  });
  test('automatic recording requires a saved opt-in and status refresh preserves the pending choice', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    const auto = () => renderer!.root.findAllByType('input').filter(n => n.props.type === 'checkbox')[2];
    expect(auto().props.checked).toBe(false);
    await act(async () => auto().props.onChange({ target: { checked: true } }));
    await act(async () => button('刷新状态').props.onClick());
    expect(auto().props.checked).toBe(true);
    expect(invoke.mock.calls.some(call => call[0] === 'lark_app_save')).toBe(false);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]?.autoRecord).toBe(true);
  });
  test('group member launch setting survives refresh and is sent only on save', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    const toggle = () => renderer!.root.findByProps({ id: 'lark-allow-group-launch' });
    expect(toggle().props.checked).toBe(false);
    await act(async () => toggle().props.onChange({ target: { checked: true } }));
    await act(async () => button('刷新状态').props.onClick());
    expect(toggle().props.checked).toBe(true);
    expect(invoke.mock.calls.some(call => call[0] === 'lark_app_save')).toBe(false);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]?.allowGroupLaunch).toBe(true);
  });
  test('cloud is the default without enabling automation and source edits survive refresh until saved', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    const source = () => renderer!.root.findByProps({ id: 'lark-recording-source' });
    expect(source().props.value).toBe('cloud');
    await act(async () => source().props.onChange({ target: { value: 'local' } }));
    await act(async () => button('刷新状态').props.onClick());
    expect(source().props.value).toBe('local');
    expect(invoke.mock.calls.some(call => call[0] === 'lark_app_save')).toBe(false);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]).toEqual(expect.objectContaining({ recordingSource: 'local', autoRecord: false }));
  });
  test('saved local source is restored and cloud authorization is requested explicitly', async () => {
    respond = async command => command === 'lark_app_status' ? { ...appStatus, recording_source: 'local' } : 'https://accounts.larksuite.com/authorize';
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(renderer!.root.findByProps({ id: 'lark-recording-source' }).props.value).toBe('local');
    await act(async () => button('授权会议助手').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_authorize')?.[1]).toEqual({ includeMeetingControl: true, includeCloudRecording: true });
  });
  test('launch failures show the source group and opening a saved link never creates or sends a meeting', async () => {
    respond = async command => command === 'lark_app_status' ? { ...appStatus, launches: [{ request_id: 'r1', topic: '研发周会', chat_id: 'oc_source', status: 'link_failed', detail: '会议已创建，链接发送失败', meeting_no: '123456789', meeting_url: 'https://vc.larksuite.com/j/123456789' }] } : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(content()).toContain('oc_source');
    expect(content()).toContain('链接发送失败');
    await act(async () => button('打开会议链接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'open_external_url')).toEqual(['open_external_url', { url: 'https://vc.larksuite.com/j/123456789' }]);
    expect(invoke.mock.calls.some(call => call[0].includes('send') || call[0].includes('create'))).toBe(false);
  });
});

describe('interface language', () => {
  function browser(saved?: string) {
    const store = new Map<string, string>(saved ? [[LANGUAGE_STORAGE_KEY, saved]] : []);
    const handlers = new Map<string, (event: any) => void>();
    Object.defineProperty(globalThis, 'window', { configurable: true, value: {
      localStorage: { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => store.set(key, value) },
      addEventListener: (name: string, fn: (event: any) => void) => handlers.set(name, fn),
      removeEventListener: (name: string) => handlers.delete(name),
    } });
    Object.defineProperty(globalThis, 'document', { configurable: true, value: { documentElement: { lang: '' } } });
    return { store, handlers };
  }
  test('defaults to Chinese, persists switching and preserves unsaved component state', async () => {
    const { store } = browser();
    let mounts = 0;
    function EditorFixture() {
      useUiLanguage();
      const [value, setValue] = useState('');
      useEffect(() => { mounts++; }, []);
      return <><label>{t('Settings')}</label><input value={value} onChange={e => setValue(e.target.value)} /></>;
    }
    await act(async () => { renderer = create(<EditorFixture />); });
    expect(content()).toContain('设置');
    await act(async () => renderer!.root.findByType('input').props.onChange({ target: { value: '未保存的纪要' } }));
    await act(async () => setUiLanguage('en'));
    expect(content()).toContain('Settings');
    expect(content()).toContain('未保存的纪要');
    expect(mounts).toBe(1);
    expect(store.get(LANGUAGE_STORAGE_KEY)).toBe('en');
    expect(t('Qwen3.5-4B')).toBe('Qwen3.5-4B');
  });
  test('restores saved language and responds to another window without overwriting meeting data', async () => {
    const { handlers } = browser('en');
    function Startup() { useInitializeUiLanguage(); return <p>{t('Start Recording')}</p>; }
    await act(async () => { renderer = create(<Startup />); });
    expect(content()).toContain('Start Recording');
    await act(async () => handlers.get('storage')!({ key: LANGUAGE_STORAGE_KEY, newValue: 'zh-CN' }));
    expect(content()).toContain('开始录音');
    expect(document.documentElement.lang).toBe('zh-CN');
  });
  test('unavailable local storage does not prevent switching', () => {
    browser();
    window.localStorage.setItem = () => { throw new Error('blocked'); };
    expect(() => setUiLanguage('en')).not.toThrow();
    expect(t('Settings')).toBe('Settings');
  });
});

