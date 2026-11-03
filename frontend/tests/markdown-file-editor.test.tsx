import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
let failSave = false;
const original = '# 详细纪要\n\n| 任务 | 负责人 |\n| --- | --- |\n| 验收 | 小明 |\n\n<img src="https://example.com/track">';
const invoke = mock(async (command: string, args?: any): Promise<any> => {
  if (command === 'summary_markdown_read') return { name: '会议纪要.md', path: 'D:/会议纪要.md', content: original, hash: 'original', size: 100 };
  if (command === 'summary_markdown_save') { if (failSave) throw '文件已被其他操作修改'; return { name: '会议纪要.md', path: 'D:/会议纪要.md', content: args.content, hash: 'updated', size: 40 }; }
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ Dialog: Container, DialogContent: Container, DialogDescription: Container, DialogHeader: Container, DialogTitle: Container }));
const { MarkdownFileEditor } = await import('../src/components/MeetingDetails/MarkdownFileEditor');
let renderer: ReactTestRenderer;
const close = mock(() => {}), previewSend = mock(() => {}), saved = mock(() => {});
const button = (text: string) => renderer.root.findAllByType('button').find(b => b.children.includes(text))!;
afterEach(async () => { if (renderer) await act(async () => renderer.unmount()); invoke.mockClear(); close.mockClear(); previewSend.mockClear(); saved.mockClear(); failSave = false; });
async function mount() {
  await act(async () => { renderer = create(<MarkdownFileEditor meetingId="m1" onClose={close} onSaved={saved} onSendPreview={previewSend} />); });
}
test('文件只提供编辑保存，不再单独预览或发送消息', async () => {
  await mount();
  expect(renderer.root.findByType('textarea').props.value).toBe(original);
  expect(button('预览排版')).toBeUndefined();
  expect(renderer.root.findAllByType('table')).toHaveLength(0);
  expect(renderer.root.findAllByType('img')).toHaveLength(0);
  expect(invoke.mock.calls.every(c => c[0] === 'summary_markdown_read')).toBe(true);
});

test('未保存编辑不能直接关闭，保存后才打开发送预览', async () => {
  await mount();
  await act(async () => renderer.root.findByType('textarea').props.onChange({ target: { value: '# 修改后的详细纪要' } }));
  await act(async () => button('返回').props.onClick());
  expect(close).not.toHaveBeenCalled();
  await act(async () => button('保存并编辑消息').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['summary_markdown_save', { meetingId: 'm1', expectedHash: 'original', content: '# 修改后的详细纪要' }]);
  expect(previewSend).toHaveBeenCalledTimes(1);
  expect(invoke.mock.calls.some(c => c[0] === 'lark_app_check_send')).toBe(false);
});
test('保存冲突时保留编辑内容，不打开发送预览', async () => {
  failSave = true; await mount();
  await act(async () => renderer.root.findByType('textarea').props.onChange({ target: { value: '# 不要丢弃的编辑' } }));
  await act(async () => button('保存并编辑消息').props.onClick());
  expect(renderer.root.findByType('textarea').props.value).toBe('# 不要丢弃的编辑');
  expect(JSON.stringify(renderer.toJSON())).toContain('文件已被其他操作修改');
  expect(previewSend).not.toHaveBeenCalled();
});
