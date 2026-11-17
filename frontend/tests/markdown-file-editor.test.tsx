import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
let failSave = false;
const original = '# 详细纪要\n\n| 任务 | 负责人 |\n| --- | --- |\n| 验收 | 小明 |\n\n<img src="https://example.com/track">';
const invoke = mock(async (command: string, args?: any): Promise<any> => {
  if (command === 'summary_markdown_history_list') return [{name: '会议纪要-old.md', size: 20}];
  if (command === 'summary_markdown_history_read') return {name: args.version, content: '# 历史稿', hash: 'version-hash'};
  if (command === 'summary_markdown_history_restore') return {name: '会议纪要.md', path: 'D:/会议纪要.md', content: '# 历史稿', hash: 'restored', size: 20};
  if (command === 'summary_markdown_read') return { name: '会议纪要.md', path: 'D:/会议纪要.md', content: original, hash: 'original', size: 100 };
  if (command === 'summary_markdown_save') { if (failSave) throw '文件已被其他操作修改'; return { name: '会议纪要.md', path: 'D:/会议纪要.md', content: args.content, hash: 'updated', size: 40 }; }
  if(command==='summary_markdown_rename')return {name:args.name,path:`D:/${args.name}`,content:original,hash:'updated',size:100};
});
mock.module('@tauri-apps/api/core', () => ({ invoke }));
const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
mock.module('../src/components/ui/dialog', () => ({ Dialog: Container, DialogContent: Container, DialogDescription: Container, DialogHeader: Container, DialogTitle: Container }));
const { MarkdownFileEditor } = await import('../src/components/MeetingDetails/MarkdownFileEditor');
let renderer: ReactTestRenderer;
const close = mock(() => {}), previewSend = mock(() => {}), saved = mock(() => {});
const button = (text: string) => renderer.root.findAllByType('button').find(b => b.children.includes(text))!;
afterEach(async () => { if (renderer) await act(async () => renderer.unmount()); invoke.mockClear(); close.mockClear(); previewSend.mockClear(); saved.mockClear(); failSave = false; });
async function mount(edit = true) {
  await act(async () => { renderer = create(<MarkdownFileEditor meetingId="m1" onClose={close} onSaved={saved} onSendPreview={previewSend} />); });
  if (edit) await act(async () => button('编辑源码').props.onClick());
}

test('编辑文件名保存时携带当前名称和内容校验，关闭前保护未保存名称',async()=>{
  await mount(false);await act(async()=>renderer.root.findByProps({'aria-label':'Markdown 文件名'}).props.onChange({target:{value:'战略会.md'}}));
  await act(async()=>button('返回').props.onClick());expect(close).not.toHaveBeenCalled();
  await act(async()=>button('保存文件').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['summary_markdown_rename',{meetingId:'m1',expectedHash:'updated',expectedName:'会议纪要.md',name:'战略会.md'}]);
  expect(saved).toHaveBeenCalledWith('D:/战略会.md');
});
test('默认预览完整 Markdown 表格，切换源码不修改文件或加载外部图片', async () => {
  await mount(false);
  expect(renderer.root.findAllByType('table')).toHaveLength(1);
  expect(renderer.root.findAllByType('th')).toHaveLength(2);
  expect(renderer.root.findAllByType('td').map(td => td.children.join(''))).toEqual(['验收','小明']);
  await act(async () => button('编辑源码').props.onClick());
  expect(renderer.root.findByType('textarea').props.value).toBe(original);
  expect(renderer.root.findAllByType('img')).toHaveLength(0);
  expect(invoke.mock.calls.every(c => c[0] === 'summary_markdown_read')).toBe(true);
});

test('预览使用未保存草稿并保留转义竖线，切换后编辑不丢失', async () => {
  await mount();
  const content = '| 任务 | 人员 |\n| --- | --- |\n| A\\|B | **甲** |';
  await act(async () => renderer.root.findByType('textarea').props.onChange({target:{value:content}}));
  await act(async () => button('预览排版').props.onClick());
  expect(renderer.root.findAllByType('td')[0].children).toEqual(['A|B']);
  expect(renderer.root.findByType('strong').children).toEqual(['甲']);
  expect(renderer.root.findAllByType('img')).toHaveLength(0);
  await act(async () => button('编辑源码').props.onClick());
  expect(renderer.root.findByType('textarea').props.value).toBe(content);
  expect(invoke.mock.calls.some(c => c[0] === 'summary_markdown_save')).toBe(false);
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

test('查看历史不发送，确认恢复后只替换文件', async () => {
  await mount(); await act(async () => button('历史版本').props.onClick());
  await act(async () => renderer.root.findByProps({'aria-label':'历史版本'}).props.onChange({target:{value:'会议纪要-old.md'}}));
  expect(renderer.root.findByProps({'aria-label':'历史版本内容'}).props.readOnly).toBe(true);
  expect(invoke.mock.calls.some(c => c[0] === 'summary_markdown_history_restore')).toBe(false);
  await act(async () => button('恢复此版本').props.onClick());
  expect(invoke.mock.calls.some(c => c[0] === 'summary_markdown_history_restore')).toBe(false);
  await act(async () => button('确认恢复').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['summary_markdown_history_restore', {meetingId:'m1',version:'会议纪要-old.md',expectedHash:'original',versionHash:'version-hash'}]);
  expect(previewSend).not.toHaveBeenCalled();
  expect(renderer.root.findByProps({'aria-label':'Markdown 文件内容'}).props.value).toBe('# 历史稿');
});

test('未保存编辑时不允许历史恢复覆盖草稿', async () => {
  await mount();
  await act(async () => renderer.root.findByProps({'aria-label':'Markdown 文件内容'}).props.onChange({target:{value:'# 未保存草稿'}}));
  await act(async () => button('历史版本').props.onClick());
  await act(async () => renderer.root.findByProps({'aria-label':'历史版本'}).props.onChange({target:{value:'会议纪要-old.md'}}));
  expect(button('恢复此版本').props.disabled).toBe(true);
  expect(renderer.root.findByProps({'aria-label':'Markdown 文件内容'}).props.value).toBe('# 未保存草稿');
});
