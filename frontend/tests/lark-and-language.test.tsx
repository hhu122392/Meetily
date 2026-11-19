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
const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
afterEach(async () => {
  if (renderer) await act(async () => renderer!.unmount());
  renderer = undefined;
  if (originalWindow) Object.defineProperty(globalThis, 'window', originalWindow); else delete (globalThis as any).window;
  if (originalDocument) Object.defineProperty(globalThis, 'document', originalDocument); else delete (globalThis as any).document;
  if (originalNavigator) Object.defineProperty(globalThis, 'navigator', originalNavigator); else delete (globalThis as any).navigator;
  setUiLanguage('zh-CN'); invoke.mockClear();
});
afterAll(() => {
  mock.module('@tauri-apps/api/core', () => originalCore);
  mock.module('../src/components/ui/dialog', () => originalDialog);
});
const config = { settings: { target_name: '产品周会', webhook: 'https://open.larksuite.com/open-apis/bot/v2/hook/test', secret: '' }, revision: 'revision-a' };
const button = (label: string) => renderer!.root.findAllByType('button').find(node => node.props['aria-label'] === label || node.children.filter(child => typeof child === 'string').join('').includes(label))!;
const content = () => JSON.stringify(renderer!.toJSON());
test('本人账号登录不请求云录音权限', async () => {
  respond = async command => command === 'lark_app_status'
    ? { app_id:'cli_test',has_secret:true,enabled:true,authorized:true, recording_source:'cloud', routes:[], deliveries:[], recording:null }
    : command === 'lark_app_authorize' ? 'https://accounts.larksuite.com/test' : null;
  await act(async () => { renderer=create(<LarkAppSettings />); });
  await act(async () => button('授权会议助手').props.onClick());
  expect(invoke.mock.calls.filter(call => call[0] === 'lark_app_authorize')).toEqual([
    ['lark_app_authorize',{includeMeetingControl:true,includeCloudRecording:false}],
  ]);
  expect(button('授权结束会议')).toBeUndefined();
  expect(button('授权云录音')).toBeUndefined();
  expect(button('重新登录 Lark')).toBeUndefined();
  expect(content()).toContain('每人登录自己的 Lark 账号');
});
test('损坏的 Lark 配置显示真实错误并阻止保存和授权', async () => {
  respond = async () => { throw '凭证格式异常。'; };
  await act(async () => { renderer = create(<LarkAppSettings />); });
  expect(content()).toContain('凭证格式异常');
  expect(content()).toContain('配置读取失败');
  expect(content()).not.toContain('请确认运行的是新版 Meetily');
  expect(button('保存并连接').props.disabled).toBe(true);
  expect(button('授权会议助手')).toBeUndefined();
  expect(invoke.mock.calls.every(call => call[0] === 'lark_app_status')).toBe(true);
});

test('群指令回执不确定时不提供重发或预约按钮', async () => {
  respond=async command=>command==='lark_app_status'?{app_id:'cli_test',has_secret:true,enabled:true,authorized:true,manual_mode:false,routes:[],deliveries:[],recording:null,commands:[{id:'om_1',chat_id:'oc_source',meeting_no:'123456789',detail:'已同步',ack_state:'unknown'}]}:undefined;
  await act(async()=>{renderer=create(<LarkAppSettings />);});
  expect(content()).toContain('送达不确定');expect(content()).toContain('123456789');
  expect(button('取消预约')).toBeUndefined();expect(button('重试核对与提醒')).toBeUndefined();
  expect(invoke.mock.calls.some(c=>c[0].includes('send'))).toBe(false);
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
  const pasteCredential = async (text: string) => {
    await act(async () => renderer!.root.findByProps({ id: 'lark-credential-paste' }).props.onPaste({
      preventDefault() {}, clipboardData: { getData: () => text },
    }));
  };
  test('后台辅助导入按两次复制填入，保存前不发送凭证或自动开启功能', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    await act(async () => button('打开 Lark 后台').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'open_external_url')).toEqual(['open_external_url', { url: 'https://open.larksuite.com/app/cli_test/baseinfo' }]);
    await pasteCredential('cli_imported');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
    expect(button('授权会议助手').props.disabled).toBe(true);
    await pasteCredential('TestOnlySecret0123456789abcdefABCD');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.type).toBe('password');
    expect(renderer!.root.findByProps({ id: 'lark-credential-paste' }).props.value).toBe('');
    expect(invoke.mock.calls.some(call => call[0] === 'lark_app_save')).toBe(false);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]).toEqual(expect.objectContaining({
      appId: 'cli_imported', secret: 'TestOnlySecret0123456789abcdefABCD', autoRecord: false, allowGroupLaunch: false,
    }));
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
    expect(invoke.mock.calls.some(call => call[0].includes('send'))).toBe(false);
  });
  test('合并凭证原子导入；无效和遮挡内容不覆盖原配置', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    await pasteCredential('App ID: cli_other\nApp Secret: ********');
    expect(renderer!.root.findByProps({ id: 'lark-app-id' }).props.value).toBe('cli_test');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('********');
    await pasteCredential('App ID: cli_other\nApp Secret: TestOnlySecret0123456789abcdefABCD');
    expect(renderer!.root.findByProps({ id: 'lark-app-id' }).props.value).toBe('cli_other');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('TestOnlySecret0123456789abcdefABCD');
    await pasteCredential('cli_third');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
  });
  test('只在点击时读取剪贴板，拒绝后可手动粘贴，错误不显示密钥', async () => {
    const readText = mock(async () => { throw new Error('TestOnlySecret0123456789abcdefABCD'); });
    Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { clipboard: { readText } } });
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(readText).not.toHaveBeenCalled();
    await act(async () => button('读取剪贴板').props.onClick());
    expect(readText).toHaveBeenCalledTimes(1);
    expect(content()).toContain('Ctrl+V');
    expect(content()).not.toContain('TestOnlySecret0123456789abcdefABCD');
    await pasteCredential('cli_pasted');
    expect(renderer!.root.findByProps({ id: 'lark-app-id' }).props.value).toBe('cli_pasted');
  });
  test('读取剪贴板期间切换应用不会将旧密钥填入新应用', async () => {
    let finish!: (text: string) => void;
    Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { clipboard: { readText: () => new Promise<string>(resolve => { finish = resolve; }) } } });
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    await act(async () => button('读取剪贴板').props.onClick());
    await act(async () => renderer!.root.findByProps({ id: 'lark-app-id' }).props.onChange({ target: { value: 'cli_changed' } }));
    await act(async () => finish('TestOnlySecret0123456789abcdefABCD'));
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
  });
  test('读取剪贴板可补充现有应用密钥，不读取或覆盖其他设置', async () => {
    const readText = mock(async () => 'TestOnlySecret0123456789abcdefABCD');
    Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { clipboard: { readText } } });
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(button('读取剪贴板').props.disabled).toBe(false);
    await act(async () => button('读取剪贴板').props.onClick());
    expect(renderer!.root.findByProps({ id: 'lark-app-id' }).props.value).toBe('cli_test');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('TestOnlySecret0123456789abcdefABCD');
    await act(async () => button('刷新状态').props.onClick());
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('TestOnlySecret0123456789abcdefABCD');
    expect(invoke.mock.calls.every(call => call[0] === 'lark_app_status')).toBe(true);
  });
  test('空配置先导入 App ID，已有 App ID 可只补密钥', async () => {
    respond = async command => command === 'lark_app_status' ? { ...appStatus, app_id: '', has_secret: false, manual_mode: false } : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(renderer!.root.findByProps({ 'aria-label': '会议助手使用方式' }).props.value).toBe('manual');
    expect(button('授权会议助手')).toBeUndefined();
    await pasteCredential('TestOnlySecret0123456789abcdefABCD');
    expect(content()).toContain('请先复制并粘贴 App ID');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('');
    await pasteCredential('cli_new');
    await pasteCredential('TestOnlySecret0123456789abcdefABCD');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('TestOnlySecret0123456789abcdefABCD');
  });
  test('本地手动模式不申请云录音权限，也不启用机器人事件和自动录音', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    await act(async () => renderer!.root.findByProps({ 'aria-label': '会议助手使用方式' }).props.onChange({target:{value:'manual'}}));
    expect(button('授权会议助手')).toBeUndefined();
    expect(renderer!.root.findAllByProps({id:'lark-recording-source'})).toHaveLength(0);
    await act(async()=>button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call=>call[0]==='lark_app_save')?.[1]).toEqual(expect.objectContaining({manualMode:true,recordingSource:'local',autoRecord:false,allowGroupLaunch:false}));
    expect(invoke.mock.calls.some(call=>call[0]==='lark_app_authorize')).toBe(false);
  });
  test('saving sends credentials only to the backend and clears the secret input without sending messages', async () => {
    respond = async command => command === 'lark_app_status' ? appStatus : undefined;
    await act(async () => { renderer = create(<LarkAppSettings />); });
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.type).toBe('password');
    expect(renderer!.root.findByProps({ id: 'lark-app-secret' }).props.value).toBe('********');
    await act(async () => button('更换密钥').props.onClick());
    await act(async () => renderer!.root.findByProps({ id: 'lark-app-secret' }).props.onChange({ target: { value: 'test-only-secret' } }));
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')).toEqual(['lark_app_save', { appId: 'cli_test', secret: 'test-only-secret', enabled: true, autoSend: true, autoRecord: false, allowGroupLaunch: false, recordingSource: 'local', manualMode: false, syncGroups: [] }]);
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
    const auto = () => renderer!.root.findByProps({ id: 'lark-auto-record' });
    expect(auto().props.checked).toBe(false);
    await act(async () => auto().props.onChange({ target: { checked: true } }));
    await act(async () => button('刷新状态').props.onClick());
    expect(auto().props.checked).toBe(true);
    expect(invoke.mock.calls.some(call => call[0] === 'lark_app_save')).toBe(false);
    await act(async () => button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(call => call[0] === 'lark_app_save')?.[1]?.autoRecord).toBe(true);
  });
  test('选择本人群后刷新不覆盖，保存才提交', async () => {
    respond=async command=>command==='lark_app_status'?{...appStatus,authorized:true}:command==='lark_sync_groups'?[{id:'oc_mine',name:'项目群'}]:undefined;
    await act(async()=>{renderer=create(<LarkAppSettings />);});
    await act(async()=>button('加载我的群').props.onClick());
    const group=()=>renderer!.root.findByProps({'aria-label':'项目群'});
    await act(async()=>group().props.onChange({target:{checked:true}}));
    await act(async()=>button('刷新状态').props.onClick());
    expect(group().props.checked).toBe(true);expect(invoke.mock.calls.some(c=>c[0]==='lark_app_save')).toBe(false);
    await act(async()=>button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(c=>c[0]==='lark_app_save')?.[1]?.syncGroups).toEqual([{id:'oc_mine',name:'项目群'}]);
    expect(invoke.mock.calls.some(c=>c[0].includes('send'))).toBe(false);
  });
  test('旧云录音配置再次保存为本地录音，不自动打开录音开关', async () => {
    respond=async command=>command==='lark_app_status'?{...appStatus,recording_source:'cloud'}:undefined;
    await act(async()=>{renderer=create(<LarkAppSettings />);});
    expect(renderer!.root.findByProps({id:'lark-auto-record'}).props.checked).toBe(false);
    await act(async()=>button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(c=>c[0]==='lark_app_save')?.[1]).toEqual(expect.objectContaining({recordingSource:'local',autoRecord:false}));
  });
  test('切换登录账号清空上一账号的群选择', async () => {
    let owner='ou_first';
    respond=async command=>command==='lark_app_status'?{...appStatus,authorized:true,owner_id:owner,sync_groups:owner==='ou_first'?[{id:'oc_old',name:'旧账号群'}]:[]}:undefined;
    await act(async()=>{renderer=create(<LarkAppSettings />);});
    expect(renderer!.root.findByProps({'aria-label':'旧账号群'}).props.checked).toBe(true);
    owner='ou_second';await act(async()=>button('刷新状态').props.onClick());
    expect(renderer!.root.findAllByProps({'aria-label':'旧账号群'})).toHaveLength(0);
    await act(async()=>button('保存并连接').props.onClick());
    expect(invoke.mock.calls.find(c=>c[0]==='lark_app_save')?.[1]?.syncGroups).toEqual([]);
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

