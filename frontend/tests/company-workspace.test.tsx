import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';

let statusFails = false;
let startupWarning: string | null = null;
let switchFails = false;
let companies = [{ id: 'legacy', name: '甲公司', app_id: 'cli_a', tenant_key: '' }];
const invoke = mock(async (command: string, args?: any): Promise<any> => {
  if (command === 'company_status') {
    if (statusFails) throw '公司配置损坏';
    return { active: 'legacy', companies: [...companies], recording_root: 'D:/录音', warning: startupWarning };
  }
  if (command === 'company_create') {
    companies.push({ id: 'new-company', name: args.name, app_id: '', tenant_key: '' }); return 'new-company';
  }
  if (command === 'company_switch' && switchFails) throw '还有转写任务';
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const relaunch = mock(async () => {});
mock.module('@tauri-apps/plugin-process', () => ({ relaunch }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) => open ? <div>{children}</div> : null,
  DialogContent: Container, DialogDescription: Container, DialogHeader: Container, DialogTitle: Container }));
const storage = new Map<string, string>();
Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, value: { getItem: (k: string) => storage.get(k) ?? null, setItem: (k: string, v: string) => storage.set(k, v), removeItem: (k: string) => storage.delete(k) } });
const { CompanyBoundary, CompanySwitcher } = await import('../src/components/CompanyWorkspace');
let renderer: ReactTestRenderer;
const button = (text: string) => renderer.root.findAllByType('button').find(b => b.children.includes(text))!;
afterEach(async () => {
  startupWarning = null;
  relaunch.mockClear();
  if (renderer) await act(async () => renderer.unmount());
  invoke.mockClear(); statusFails = false; switchFails = false;
  companies = [{ id: 'legacy', name: '甲公司', app_id: 'cli_a', tenant_key: '' }]; storage.clear();
});
test('Lark 配置异常显示提示但仍能访问历史会议', async () => {
  startupWarning = 'Lark 配置无法读取：凭证格式异常。本地会议仍可查看。';
  await act(async () => { renderer = create(<CompanyBoundary><p>会议列表</p></CompanyBoundary>); });
  expect(renderer.root.findByProps({ role: 'alert' }).children.join('')).toContain('凭证格式异常');
  expect(JSON.stringify(renderer.toJSON())).toContain('会议列表');
  expect(invoke.mock.calls.map(c => c[0])).toEqual(['company_status']);
});
async function mount() {
  await act(async () => { renderer = create(<CompanyBoundary><span>会议列表</span><CompanySwitcher /></CompanyBoundary>); });
}
test('公司加载失败时不会渲染会议资料', async () => {
  statusFails = true; await mount();
  expect(JSON.stringify(renderer.toJSON())).toContain('公司配置损坏');
  expect(JSON.stringify(renderer.toJSON())).not.toContain('会议列表');
  await act(async () => button('重启 Meetily').props.onClick());
  expect(relaunch).toHaveBeenCalledTimes(1);
});
test('新增公司不切换，必须单独点击切换并重启', async () => {
  await mount();
  await act(async () => renderer.root.findByProps({ title: '公司资料：甲公司' }).props.onClick());
  await act(async () => renderer.root.findByProps({ id: 'new-company' }).props.onChange({ target: { value: '乙公司' } }));
  await act(async () => button('新增').props.onClick());
  expect(invoke.mock.calls.some(c => c[0] === 'company_switch')).toBe(false);
  expect(renderer.root.findByType('select').props.value).toBe('new-company');
  await act(async () => button('切换并重启').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['company_switch', { id: 'new-company' }]);
  expect(JSON.stringify(renderer.toJSON())).toContain('正在重启并切换');
});
test('切换被任务阻止时显示原因并保留当前公司', async () => {
  companies.push({ id: 'other', name: '乙公司', app_id: '', tenant_key: '' }); switchFails = true;
  await mount();
  await act(async () => renderer.root.findByProps({ title: '公司资料：甲公司' }).props.onClick());
  await act(async () => renderer.root.findByType('select').props.onChange({ target: { value: 'other' } }));
  await act(async () => button('切换并重启').props.onClick());
  expect(JSON.stringify(renderer.toJSON())).toContain('还有转写任务');
  expect(JSON.stringify(renderer.toJSON())).not.toContain('正在重启并切换');
  expect(renderer.root.findByProps({ title: '公司资料：甲公司' })).toBeTruthy();
});
