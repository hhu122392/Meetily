import { expect, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { MarkdownCardPreview, LarkCardPreview } from '../src/components/MeetingDetails/MinutesCardPreview';

test('表格按行展示字段卡片，保留空值、行内格式和转义竖线', async () => {
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(<MarkdownCardPreview content={'# 发布评审\n\n## 行动项\n| 事项 | 负责人 | 状态 |\n| :--- | ---: | :---: |\n| **接口联调** | 小明 | `A\\|B` |\n| [验收](https://example.com) | 小红 | |\n\n补充说明。'} />); });
  expect(renderer.root.findAllByType('table')).toHaveLength(0);
  expect(renderer.root.findAllByType('dl')).toHaveLength(2);
  expect(renderer.root.findAllByType('dt')).toHaveLength(6);
  expect(renderer.root.findAllByType('strong')[0].children).toEqual(['接口联调']);
  expect(renderer.root.findByType('code').children).toEqual(['A|B']);
  expect(renderer.root.findByType('a').props.href).toBe('https://example.com');
  const rendered = JSON.stringify(renderer.toJSON());
  for (const value of ['发布评审', '行动项', '小明', '小红', '—', '补充说明。']) expect(rendered).toContain(value);
  await act(async () => renderer.unmount());
});

test('预览不加载外部图片或执行 HTML，代码中的标题不替代会议主题', async () => {
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(<MarkdownCardPreview content={'```text\n# 代码标题\n```\n\n![远程图片](https://example.com/track)\n\n<img src="https://example.com/track2">\n<script>alert(1)</script>'} />); });
  expect(renderer.root.findAllByType('img')).toHaveLength(0);
  expect(renderer.root.findAllByType('script')).toHaveLength(0);
  expect(renderer.root.findByType('header').findByType('h3').children).toEqual(['详细会议纪要']);
  expect(renderer.root.findByType('code').children.join('')).toContain('# 代码标题');
  await act(async () => renderer.unmount());
});

test('发送预览使用实际卡片的标题、字段和概要，HTML 标签仅作为文字', async () => {
  let renderer!: ReactTestRenderer;
  await act(async () => { renderer = create(<LarkCardPreview fallback="旧的纯文本" card={{
    header: { title: { tag: 'plain_text', content: '会议纪要｜周会' } },
    elements: [
      { tag: 'div', fields: [{ text: { tag: 'plain_text', content: '会议号\n123456' } }] },
      { tag: 'div', text: { tag: 'lark_md', content: '**概要**\n完成联调。 &lt;at id=all&gt;' } },
      { tag: 'action', actions: [{ text: { tag: 'plain_text', content: '打开原会议链接' }, url: 'https://vc.larksuite.com/test' }] },
    ],
  }} />); });
  const rendered = JSON.stringify(renderer.toJSON());
  for (const value of ['会议纪要｜周会', '123456', '完成联调', '打开原会议链接']) expect(rendered).toContain(value);
  expect(rendered).not.toContain('旧的纯文本');
  expect(renderer.root.findAll(node => typeof node.type === 'string' && String(node.type) === 'at')).toHaveLength(0);
  expect(renderer.root.findAllByType('table')).toHaveLength(0);
  await act(async () => renderer.unmount());
});
