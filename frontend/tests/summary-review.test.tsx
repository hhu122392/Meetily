import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';

let failSave = false;
let failExport = false;
let failSend = false;
let failEdit = false;
let lastSentAt: number | null = null;
let uncertain = false;
const sequence: string[] = [];
const invoke = mock(async (command: string, args?: any) => {
  sequence.push(command);
  if (command === 'summary_markdown_status') return {};
  if (command === 'summary_markdown_read') return { name: '会议纪要.md', path: 'D:/录音/会议纪要.md', content: '# 已有纪要', hash: 'test-hash', size: 20 };
  if (command === 'lark_app_meeting_status') return { linked: true, last_sent_at: lastSentAt };
  if (command === 'summary_export_markdown') { if (failExport) throw '磁盘已满'; return 'D:/录音/会议纪要.md'; }
  if (command === 'lark_app_edit_message') {
    if (failEdit) throw '消息草稿已变化';
    return { chat_id: 'oc_review', remote_id: '123456', revision: 'edited-2', message_title: args.title, message_summary: args.summary, message_edited: true, text: args.summary, card: { header: { title: { tag: 'plain_text', content: args.title } }, elements: [{ tag: 'div', text: { tag: 'lark_md', content: args.summary } }] }, markdown: { name: '会议纪要.md', path: 'D:/录音/会议纪要.md', content: '原始详细纪要', size: 32 } };
  }
  if (command === 'lark_app_preview_summary') return { message_title: '研发周会', message_summary: '已保存的修订纪要', chat_id: 'oc_review', chat_name: '研发组', last_sent_at: lastSentAt, remote_id: '123456', text: '不应替代真实卡片的旧预览文本', card: { header: { title: { tag: 'plain_text', content: '会议纪要｜研发周会' } }, elements: [{ tag: 'div', text: { tag: 'lark_md', content: '**概要**\n已保存的修订纪要' } }] }, revision: 'review-1', requires_duplicate_ack: uncertain, markdown: { name: '会议纪要.md', path: 'D:/录音/会议纪要.md', content: '# 中文纪要\n决定使用 Lark', size: 32 } };
  if (command === 'lark_app_check_send') { if (failSend) throw '内容已变化，请重新审核'; lastSentAt = 1792396800; return lastSentAt; }
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ Dialog: ({ open, children }: { open: boolean; children?: ReactNode }) => open ? <div>{children}</div> : null,
  DialogContent: Container, DialogDescription: Container, DialogHeader: Container, DialogTitle: Container }));
const { SummaryReviewActions } = await import('../src/components/MeetingDetails/SummaryReviewActions');
let renderer: ReactTestRenderer;
const button = (text: string) => renderer.root.findAllByType('button').find(b => b.props['aria-label'] === text || b.children.includes(text))!;
afterEach(async () => { if (renderer) await act(async () => renderer.unmount()); invoke.mockClear(); sequence.length = 0; failSave = failExport = failSend = failEdit = false; lastSentAt = null; uncertain = false; });
async function mount() {
  await act(async () => { renderer = create(<SummaryReviewActions meetingId="test" saveEdits={async () => { sequence.push('save-edits'); if (failSave) throw '保存失败'; }} />); });
  sequence.length = 0;
  await act(async () => button('发送到 Lark').props.onClick());
}
test('saves edits and Markdown before preview; requires a separate confirmation', async () => {
  await mount();
  expect(sequence).toEqual(['save-edits', 'summary_export_markdown', 'lark_app_preview_summary']);
  expect(JSON.stringify(renderer.toJSON())).toContain('oc_review');
  expect(JSON.stringify(renderer.toJSON())).toContain('已保存的修订纪要');
  await act(async () => button('确认发送').props.onClick());
  expect(invoke.mock.calls as unknown[]).toContainEqual(['lark_app_check_send', { meetingId: 'test', expectedRevision: 'review-1', acknowledgeDuplicateRisk: false, includeMarkdown: true }]);
  expect(button('已发送').props.disabled).toBe(true);
});

test('编辑纪要入口保存当前草稿后打开文件编辑器，不准备或发送群消息', async () => {
  await act(async () => { renderer = create(<SummaryReviewActions meetingId="test" saveEdits={async () => { sequence.push('save-edits'); }} />); });
  sequence.length = 0;
  await act(async () => renderer.root.findAllByType('button').find(b => b.props['aria-label'] === '编辑纪要')!.props.onClick());
  expect(sequence).toEqual(['save-edits', 'summary_export_markdown', 'summary_markdown_read']);
  expect(JSON.stringify(renderer.toJSON())).toContain('# 已有纪要');
});
test('save failure cannot open a sendable preview', async () => {
  failSave = true; await mount();
  expect(sequence).toEqual(['save-edits']);
  expect(button('确认发送')).toBeUndefined();
});

test('取消附件后需确认才发送，并明确传递不上传文件', async () => {
  await mount();
  await act(async () => renderer.root.findByProps({ 'aria-label': '发送 Markdown 文件附件' }).props.onChange({ target: { checked: false } }));
  expect(sequence).not.toContain('lark_app_check_send');
  expect(JSON.stringify(renderer.toJSON())).toContain('不上传文件');
  await act(async () => button('确认发送').props.onClick());
  expect(invoke.mock.calls as unknown[]).toContainEqual(['lark_app_check_send', { meetingId: 'test', expectedRevision: 'review-1', acknowledgeDuplicateRisk: false, includeMarkdown: false }]);
  expect(JSON.stringify(renderer.toJSON())).toContain('本次未发送文件');
});
test('export failure preserves visible error and does not send', async () => {
  failExport = true; await mount();
  expect(sequence).not.toContain('lark_app_preview_summary');
  expect(JSON.stringify(renderer.toJSON())).toContain('磁盘已满');
});
test('a stale approval cannot be clicked again without a fresh preview', async () => {
  failSend = true; await mount();
  await act(async () => button('确认发送').props.onClick());
  expect(button('确认发送')).toBeUndefined();
  expect(JSON.stringify(renderer.toJSON())).toContain('重新审核');
});

test('shows group name and last success; a repeated send needs a fresh confirmation', async () => {
  lastSentAt = 1792300000;
  await mount();
  expect(JSON.stringify(renderer.toJSON())).toContain('研发组');
  expect(JSON.stringify(renderer.toJSON())).toContain('上次发送时间：');
  expect(sequence).not.toContain('lark_app_check_send');
  await act(async () => { const confirm = button('确认再次发送'); confirm.props.onClick(); confirm.props.onClick(); });
  expect(sequence.filter(x => x === 'lark_app_check_send')).toHaveLength(1);
  expect(button('已发送').props.disabled).toBe(true);
  expect(JSON.stringify(renderer.toJSON())).toContain(new Date(1792396800000).toLocaleString('zh-CN', { hour12: false }));
  await act(async () => button('返回编辑').props.onClick());
  await act(async () => button('发送到 Lark').props.onClick());
  expect(sequence.filter(x => x === 'lark_app_check_send')).toHaveLength(1);
  await act(async () => button('确认再次发送').props.onClick());
  expect(sequence.filter(x => x === 'lark_app_check_send')).toHaveLength(2);
});

test('previews local Markdown and requires explicit duplicate acknowledgment before sending', async () => {
  uncertain = true;
  await mount();
  const view = JSON.stringify(renderer.toJSON());
  expect(view).toContain('会议纪要.md');
  expect(renderer.root.findAllByType('details')).toHaveLength(0);
  expect(view).toContain('会议纪要｜研发周会');
  expect(view).not.toContain('不应替代真实卡片的旧预览文本');
  expect(renderer.root.findAllByType('table')).toHaveLength(0);
  expect(view).toContain('将发送的消息内容（卡片）');
  expect(button('确认再次发送').props.disabled).toBe(true);
  await act(async () => button('确认再次发送').props.onClick());
  expect(sequence).not.toContain('lark_app_check_send');
  await act(async () => renderer.root.findByProps({ 'aria-label': '接受可能重复发送' }).props.onChange({ target: { checked: true } }));
  expect(sequence).not.toContain('lark_app_check_send');
  await act(async () => button('确认再次发送').props.onClick());
  expect(invoke.mock.calls as unknown[]).toContainEqual(['lark_app_check_send', { meetingId: 'test', expectedRevision: 'review-1', acknowledgeDuplicateRisk: true, includeMarkdown: true }]);
  expect(JSON.stringify(renderer.toJSON())).toContain('纪要卡片和 Markdown 附件均已发送');
});


test('消息修改须保存并更新卡片预览后，才能确认发送', async () => {
  await mount();
  await act(async () => renderer.root.findByProps({ 'aria-label': '消息标题' }).props.onChange({ target: { value: '确认发布安排' } }));
  await act(async () => renderer.root.findByProps({ 'aria-label': '消息概要' }).props.onChange({ target: { value: '- 周五发布\n- 小明负责验收' } }));
  expect(button('确认发送').props.disabled).toBe(true);
  await act(async () => button('确认发送').props.onClick());
  expect(sequence).not.toContain('lark_app_check_send');
  await act(async () => button('返回编辑').props.onClick());
  expect(JSON.stringify(renderer.toJSON())).toContain('消息修改尚未保存');
  await act(async () => button('保存消息并更新预览').props.onClick());
  expect(invoke.mock.calls as unknown[]).toContainEqual(['lark_app_edit_message', { meetingId: 'test', expectedRevision: 'review-1', title: '确认发布安排', summary: '- 周五发布\n- 小明负责验收' }]);
  expect(JSON.stringify(renderer.toJSON())).toContain('确认发布安排');
  expect(sequence).not.toContain('lark_app_check_send');
  expect(button('确认发送').props.disabled).toBeFalsy();
  await act(async () => button('确认发送').props.onClick());
  expect(invoke.mock.calls as unknown[]).toContainEqual(['lark_app_check_send', { meetingId: 'test', expectedRevision: 'edited-2', acknowledgeDuplicateRisk: false, includeMarkdown: true }]);
});

test('消息保存冲突时保留输入并禁止发送旧预览，可放弃修改', async () => {
  await mount(); failEdit = true;
  await act(async () => renderer.root.findByProps({ 'aria-label': '消息概要' }).props.onChange({ target: { value: '保留我的修改' } }));
  await act(async () => button('保存消息并更新预览').props.onClick());
  expect(renderer.root.findByProps({ 'aria-label': '消息概要' }).props.value).toBe('保留我的修改');
  expect(button('确认发送').props.disabled).toBe(true);
  expect(sequence).not.toContain('lark_app_check_send');
  await act(async () => button('放弃消息修改').props.onClick());
  expect(renderer.root.findByProps({ 'aria-label': '消息概要' }).props.value).toBe('已保存的修订纪要');
});
