import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
const Container = ({children}: {children?: ReactNode}) => <div>{children}</div>;
mock.module('../src/components/ui/tooltip', () => ({ Tooltip: Container, TooltipContent: Container, TooltipTrigger: Container }));
const { EditableTranscriptSegment } = await import('../src/components/EditableTranscriptSegment');
let view: ReactTestRenderer;
const event = { stopPropagation() {}, preventDefault() {}, nativeEvent: { isComposing: false } };
const button = (name: string) => view.root.findAllByType('button').find(b => b.props['aria-label'] === name)!;
const props = { id: 's', timestamp: 3, text: '原文🙂', isStreaming: false, showConfidence: false };
async function mount(onUpdateSegment?: (id: string, text: string, expected: string) => Promise<void>) {
  await act(async () => { view = create(<EditableTranscriptSegment {...props} onUpdateSegment={onUpdateSegment} />); });
}
async function edit(text: string) {
  await act(async () => button('编辑这段转写').props.onClick(event));
  await act(async () => view.root.findByType('textarea').props.onChange({ target: { value: text } }));
}
afterEach(async () => { await act(async () => view?.unmount()); });

test('逐段编辑提交完整原文快照和中文多行内容，Ctrl+Enter 保存', async () => {
  const save = mock(async () => {}); await mount(save); await edit(' 修改🙂\n第二行 ');
  await act(async () => view.root.findByType('textarea').props.onKeyDown({ ...event, key: 'Enter', ctrlKey: true }));
  expect(save).toHaveBeenCalledWith('s', '修改🙂\n第二行', '原文🙂');
  expect(view.root.findAllByType('textarea')).toHaveLength(0);
});
test('取消和 Escape 丢弃草稿，空白不能保存', async () => {
  const save = mock(async () => {}); await mount(save); await edit('  ');
  expect(button('保存转写').props.disabled).toBe(true);
  await act(async () => view.root.findByType('textarea').props.onKeyDown({ ...event, key: 'Escape' }));
  await edit('临时内容');
  await act(async () => button('取消编辑转写').props.onClick(event));
  expect(save).not.toHaveBeenCalled();
  expect(JSON.stringify(view.toJSON())).toContain('原文🙂');
});
test('保存失败保留草稿，重新刷新原文不会悄悄覆盖编辑开始时的快照', async () => {
  const save = mock(async () => { throw new Error('proofread_source_changed'); });
  await mount(save); await edit('我的修改');
  await act(async () => view.update(<EditableTranscriptSegment {...props} text="别人已修改" onUpdateSegment={save} />));
  await act(async () => button('保存转写').props.onClick(event));
  expect(save).toHaveBeenCalledWith('s', '我的修改', '原文🙂');
  expect(view.root.findByType('textarea').props.value).toBe('我的修改');
  expect(JSON.stringify(view.toJSON())).toContain('转写已变化');
});
test('保存中阻止重复请求和取消，中文输入法确认不触发保存', async () => {
  let finish!: () => void;
  const save = mock(() => new Promise<void>(resolve => { finish = resolve; }));
  await mount(save); await edit('新内容');
  await act(async () => view.root.findByType('textarea').props.onKeyDown({ ...event, nativeEvent: { isComposing: true }, key: 'Enter', ctrlKey: true }));
  expect(save).not.toHaveBeenCalled();
  await act(async () => button('保存转写').props.onClick(event));
  expect(button('取消编辑转写').props.disabled).toBe(true);
  await act(async () => button('保存转写').props.onClick(event));
  expect(save).toHaveBeenCalledTimes(1);
  await act(async () => finish());
});
test('没有保存回调时不出现铅笔，录音页面维持只读', async () => {
  await mount(); expect(view.root.findAllByType('button')).toHaveLength(0);
});
