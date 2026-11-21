import { afterAll, afterEach, beforeEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
import { testInvoke as invoke, setTestInvoke, restoreTestInvoke } from './lib/tauri-test-invoke';
const originalDialog = { ...await import('../src/components/ui/dialog') };
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ ...originalDialog, Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) => open ? <div>{children}</div> : null, DialogContent: Container, DialogHeader: Container, DialogTitle: Container, DialogDescription: Container }));
const { ActionItems } = await import('../src/components/MeetingDetails/ActionItems');
let renderer: ReactTestRenderer | undefined;
const scrollPreview = mock(() => {});
const focusPreview = mock(() => {});
let respond: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
beforeEach(() => setTestInvoke((command, args) => respond(command, args)));
afterEach(async () => { if (renderer) await act(async () => renderer!.unmount()); renderer = undefined; invoke.mockClear(); scrollPreview.mockClear(); focusPreview.mockClear(); });
afterAll(() => { restoreTestInvoke(); mock.module('../src/components/ui/dialog', () => originalDialog); });
const field = (label: string) => renderer!.root.findByProps({ 'aria-label': label });
const button = (label: string) => renderer!.root.findAllByType('button').find(b => b.props['aria-label'] === label || b.children.filter(v => typeof v === 'string').join('').includes(label))!;
const item = { id: 'a', summary: '补充测试', evidence: '- 补充测试', ownerText: '', dueText: '', dueDate: '', dueTime: '', memberChatId: '', members: [] as { id: string; name: string }[] };
const base = { sourceHash: 'hash', revision: 'rev', items: [item], receipts: [] as any[], authorized: true, ownerId: 'ou_current' };
async function mount(context = structuredClone(base)) {
  respond = async (command, args) => {
    if (command === 'lark_actions_extract') return context;
    if (command === 'lark_actions_save') { context = { ...context, items: args?.items as typeof context.items, revision: 'rev-saved' }; return context; }
    if (command === 'lark_actions_preview') return { id: 'preview', ownerId: context.ownerId, items: context.items.filter(i => (args?.selectedIds as string[]).includes(i.id)) };
    if (command === 'lark_actions_sync') return { ...context, receipts: [{ id: 'a', summary: '补充测试', status: 'sent', detail: '已同步', taskUrl: 'https://applink.larksuite.com/task/a' }] };
    if (command === 'lark_manual_groups') return [{ id: 'oc_group', name: '会议群' }];
    if (command === 'lark_actions_members') return [{ id: 'ou_a', name: '张三' }, { id: 'ou_b', name: '张三' }];
    if (command === 'lark_app_authorize') return 'https://accounts.larksuite.com/authorize';
    return undefined;
  };
  await act(async () => { renderer = create(<ActionItems meetingId="meeting" saveEdits={async () => {}} />, {
    createNodeMock: element => element.props['aria-label'] === '同步预览' ? { scrollIntoView: scrollPreview, focus: focusPreview } : null,
  }); });
  await act(async () => button('行动项').props.onClick());
}
async function preview() {
  await act(async () => field('同步行动项 1').props.onChange({ target: { checked: true } }));
  await act(async () => button('预览同步').props.onClick());
}
test('提取和保存草稿不联网同步，负责人和日期默认留空，所有条目默认不选', async () => {
  await mount({ ...structuredClone(base), authorized: false });
  expect(field('同步行动项 1').props.checked).toBe(false);
  expect(field('截止日期 1').props.value).toBe('');
  expect(field('截止时刻 1').props.value).toBe('');
  expect(field('负责人原文 1').props.value).toBe('');
  expect(button('预览同步').props.disabled).toBe(true);
  expect(invoke.mock.calls.map(c => c[0])).toEqual(['summary_export_markdown', 'lark_actions_extract']);
  await act(async () => field('事项 1').props.onChange({ target: { value: '核对后事项' } }));
  await act(async () => button('保存待办草稿').props.onClick());
  expect(invoke.mock.calls.some(c => c[0] === 'lark_actions_sync' || c[0] === 'lark_actions_preview')).toBe(false);
});
test('只同步勾选项，预览后二次人工确认才调用创建，成功后不可再选', async () => {
  await mount(); await preview();
  expect(scrollPreview).toHaveBeenCalledWith({ block: 'nearest' });
  expect(focusPreview).toHaveBeenCalledWith({ preventScroll: true });
  expect(invoke.mock.calls.find(c => c[0] === 'lark_actions_preview')?.[1]).toEqual({ meetingId: 'meeting', sourceHash: 'hash', expectedRevision: 'rev', selectedIds: ['a'] });
  expect(field('已人工核对行动项').props.checked).toBe(false);
  expect(button('确认同步到 Lark 任务').props.disabled).toBe(true);
  expect(invoke.mock.calls.some(c => c[0] === 'lark_actions_sync')).toBe(false);
  await act(async () => field('已人工核对行动项').props.onChange({ target: { checked: true } }));
  await act(async () => button('确认同步到 Lark 任务').props.onClick());
  expect(invoke.mock.calls.filter(c => c[0] === 'lark_actions_sync')).toEqual([['lark_actions_sync', { previewId: 'preview', confirmed: true }]]);
  expect(field('同步行动项 1').props.checked).toBe(false);
  expect(field('同步行动项 1').props.disabled).toBe(true);
  expect(button('打开任务')).toBeDefined();
});

test('预览校验失败在独立状态区显示，可修改后重试，等待期间不重复请求或创建任务', async () => {
  await mount();
  const previous = respond;
  let rejectPreview!: (error: string) => void;
  respond = (command, args) => command === 'lark_actions_preview'
    ? new Promise((_, reject) => { rejectPreview = reject; })
    : previous(command, args);
  await preview();
  expect(field('行动项处理状态').props['aria-busy']).toBe(true);
  expect(button('预览同步').props.disabled).toBe(true);
  await act(async () => button('预览同步').props.onClick());
  expect(invoke.mock.calls.filter(c => c[0] === 'lark_actions_preview')).toHaveLength(1);
  await act(async () => rejectPreview('“补充测试”提到了负责人，请匹配 Lark 用户；确需不指定时请清空负责人文字。'));
  const status = field('行动项处理状态');
  expect(status.findByProps({ role: 'alert' }).children.join('')).toContain('请匹配 Lark 用户');
  expect(field('行动项内容').findAllByProps({ role: 'alert' })).toHaveLength(0);
  expect(button('预览同步').props.disabled).toBe(false);
  expect(button('确认同步到 Lark 任务')).toBeUndefined();
  respond = previous;
  await act(async () => field('事项 1').props.onChange({ target: { value: '修改后的测试' } }));
  await act(async () => button('预览同步').props.onClick());
  expect(renderer!.root.findAllByProps({ role: 'alert' })).toHaveLength(0);
  expect(field('同步预览').findAllByType('li')[0].findAllByType('p')[0].children.join('')).toBe('修改后的测试');
  expect(invoke.mock.calls.some(c => c[0] === 'lark_actions_sync')).toBe(false);
});
test('修改或改选使预览失效，重新预览使用保存后的版本', async () => {
  await mount(); await preview();
  await act(async () => field('事项 1').props.onChange({ target: { value: '新的事项' } }));
  expect(button('确认同步到 Lark 任务')).toBeUndefined();
  await act(async () => button('预览同步').props.onClick());
  expect(invoke.mock.calls.filter(c => c[0] === 'lark_actions_preview').at(-1)?.[1]?.expectedRevision).toBe('rev-saved');
  expect(field('已人工核对行动项').props.checked).toBe(false);
  await act(async () => field('同步行动项 1').props.onChange({ target: { checked: false } }));
  expect(button('确认同步到 Lark 任务')).toBeUndefined();
});
test('群成员按需加载，同名不自动选，允许明确选择多个负责人', async () => {
  await mount();
  expect(invoke.mock.calls.some(c => c[0] === 'lark_actions_members')).toBe(false);
  await act(async () => button('获取可用群').props.onClick());
  await act(async () => field('负责人来源群').props.onChange({ target: { value: 'oc_group' } }));
  expect(field('负责人 1').props.value).toBe('');
  await act(async () => field('负责人 1').props.onChange({ target: { value: 'ou_b' } }));
  await act(async () => field('负责人 1').props.onChange({ target: { value: 'ou_a' } }));
  await act(async () => button('保存待办草稿').props.onClick());
  const saved = invoke.mock.calls.find(c => c[0] === 'lark_actions_save')?.[1]?.items as typeof base.items;
  expect(saved[0].members.map(m => m.id)).toEqual(['ou_b', 'ou_a']);
  expect(saved[0].memberChatId).toBe('oc_group');
  expect(saved[0].dueDate).toBe('');
});
test('群成员权限被拒时提示应用身份权限，可原群重试并恢复选择', async () => {
  await mount();
  await act(async () => button('获取可用群').props.onClick());
  const previous = respond;
  respond = async (command, args) => {
    if (command === 'lark_actions_members') throw 'Lark 拒绝请求，错误码 99991672。';
    return previous(command, args);
  };
  await act(async () => field('负责人来源群').props.onChange({ target: { value: 'oc_group' } }));
  expect(renderer!.root.findByProps({ role: 'alert' }).children.join('')).toContain('im:chat.members:read');
  expect(field('负责人 1').props.disabled).toBe(true);
  respond = previous;
  await act(async () => button('重试获取群成员').props.onClick());
  expect(field('负责人来源群').props.value).toBe('oc_group');
  expect(field('负责人 1').props.disabled).toBe(false);
  expect(renderer!.root.findAllByProps({ role: 'alert' })).toHaveLength(0);
});
test('网络失败不会自动重试，重开窗口后未知回执仍阻止重复创建', async () => {
  await mount(); await preview();
  const previous = respond; respond = async (command, args) => { if (command === 'lark_actions_sync') throw '网络中断'; return previous(command, args); };
  await act(async () => field('已人工核对行动项').props.onChange({ target: { checked: true } }));
  await act(async () => button('确认同步到 Lark 任务').props.onClick());
  expect(invoke.mock.calls.filter(c => c[0] === 'lark_actions_sync')).toHaveLength(1);
  expect(button('确认同步到 Lark 任务')).toBeUndefined();
  await act(async () => button('关闭').props.onClick());
  respond = async (command, args) => command === 'lark_actions_extract' ? { ...base, receipts: [{ id: 'a', summary: '补充测试', status: 'unknown', detail: '请核对' }] } : previous(command, args);
  await act(async () => button('行动项').props.onClick());
  expect(field('同步行动项 1').props.disabled).toBe(true);
  expect(field('同步行动项 1').props.checked).toBe(false);
});
test('手动补充不默认同步，未保存修改阻止关闭，任务授权不修改录音配置', async () => {
  await mount();
  await act(async () => button('补充行动项').props.onClick());
  expect(field('同步行动项 2').props.checked).toBe(false);
  expect(field('截止日期 2').props.value).toBe('');
  await act(async () => button('关闭').props.onClick());
  expect(JSON.stringify(renderer!.toJSON())).toContain('行动项修改尚未保存');
  await act(async () => button('放弃未保存修改').props.onClick());
  await act(async () => button('重新授权任务').props.onClick());
  expect(invoke.mock.calls.find(c => c[0] === 'lark_app_authorize')?.[1]).toEqual({ tasksOnly: true });
  expect(invoke.mock.calls.some(c => c[0] === 'lark_app_save' || c[0] === 'lark_actions_sync')).toBe(false);
});

test("长依据预览明确说明节选且仍需人工确认", async () => {
  const context = structuredClone(base);
  context.items[0].evidence = "证".repeat(2001);
  await mount(context); await preview();
  expect(JSON.stringify(field("同步预览").findAllByType("p").map(p => p.children))).toContain("前 2,000 字");
  expect(field("已人工核对行动项").props.checked).toBe(false);
  expect(button("确认同步到 Lark 任务").props.disabled).toBe(true);
  expect(invoke.mock.calls.some(c => c[0] === "lark_actions_sync")).toBe(false);
});
