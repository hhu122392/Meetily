'use client';
import type { ReactNode } from 'react';
import ReactMarkdown, { type ExtraProps } from 'react-markdown';
import remarkGfm from 'remark-gfm';

type Element = NonNullable<ExtraProps['node']>;
type ElementContent = Element['children'][number];
type Root = { type: 'root'; children: (ElementContent | { type: 'doctype' })[] };

// Transform the parsed table, so escaped pipes, inline formatting and empty cells
// retain their meaning without changing the editable Markdown source.
function readableTables() {
  return (tree: Root) => {
    function visit(parent: Root | Element) {
      for (const node of parent.children) {
        if (node.type !== 'element') continue;
        if (node.tagName !== 'table') { visit(node); continue; }
        const rows: Element[] = [];
        function collect(element: Element) {
          if (element.tagName === 'tr') rows.push(element);
          else for (const child of element.children) if (child.type === 'element') collect(child);
        }
        collect(node);
        const cells = (row: Element) => row.children.filter((c): c is Element => c.type === 'element' && ['th', 'td'].includes(c.tagName));
        const headers = rows[0] ? cells(rows[0]) : [];
        const element = (tagName: string, children: ElementContent[], className: string): Element => ({ type: 'element', tagName, properties: { className }, children });
        node.tagName = 'div';
        node.properties = { className: 'space-y-3 my-4', 'data-table-cards': true };
        node.children = rows.slice(1).map(row => element('dl', cells(row).map((cell, index) => element('div', [
          element('dt', headers[index]?.children.length ? headers[index].children : [{ type: 'text', value: `字段 ${index + 1}` }], 'text-xs font-medium text-slate-500'),
          element('dd', cell.children.length ? cell.children : [{ type: 'text', value: '—' }], 'm-0 mt-1 text-sm text-slate-800 break-words'),
        ], 'min-w-0')), 'm-0 grid gap-3 rounded-lg border border-slate-200 bg-slate-50 p-4'));
        if (rows.length === 1) node.children = [element('dl', headers.map(header => element('div', [
          element('dt', header.children, 'text-xs font-medium text-slate-500'),
          element('dd', [{ type: 'text', value: '—' }], 'm-0 text-sm'),
        ], 'min-w-0')), 'm-0 grid gap-3 rounded-lg border border-slate-200 bg-slate-50 p-4')];
      }
    }
    visit(tree);
  };
}

function MarkdownBody({ content }: { content: string }) {
  return <div className="prose prose-sm max-w-none break-words prose-headings:text-slate-800 prose-h1:text-lg prose-h2:text-base prose-h3:text-sm prose-pre:overflow-x-auto">
    <ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[readableTables]} skipHtml components={{
      p: ({ children }) => <p className="whitespace-pre-line">{children}</p>,
      img: ({ alt }) => <span>[图片：{alt || '未加载外部图片'}]</span>,
      a: ({ href, children }) => <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>,
    }}>{content}</ReactMarkdown>
  </div>;
}

function CardFrame({ title, children }: { title: string; children: ReactNode }) {
  return <article className="overflow-hidden rounded-xl border border-slate-200 bg-white text-slate-800 shadow-sm">
    <header className="border-b border-blue-100 bg-blue-50 px-5 py-4"><h3 className="m-0 break-words text-base font-semibold text-blue-900">{title}</h3></header>
    <div className="space-y-4 px-5 py-4">{children}</div>
  </article>;
}

export function MarkdownCardPreview({ content }: { content: string }) {
  const heading = /^\s*# +([^\r\n]+)(?:\r?\n|$)/.exec(content);
  const title = heading?.[1].trim() || '详细会议纪要';
  const body = heading ? content.slice(0, heading.index) + content.slice(heading.index + heading[0].length) : content;
  return <CardFrame title={title}><MarkdownBody content={body} /></CardFrame>;
}

type CardText = { tag: string; content: string };
type CardElement = { tag: string; text?: CardText; fields?: { text: CardText }[]; elements?: CardText[]; actions?: { text: CardText; url?: string }[] };
export type LarkPreviewCard = { header: { title: CardText }; elements: CardElement[] };

export function LarkCardPreview({ card, fallback }: { card?: LarkPreviewCard; fallback: string }) {
  if (!card) return <MarkdownCardPreview content={fallback} />;
  const text = (value: CardText) => value.tag === 'lark_md' ? <MarkdownBody content={value.content} /> : <p className="m-0 whitespace-pre-wrap break-words text-sm">{value.content}</p>;
  return <CardFrame title={card.header.title.content}>{card.elements.map((item, index) => {
    if (item.tag === 'hr') return <hr key={index} className="border-slate-100" />;
    if (item.tag === 'div') return <section key={index} className="space-y-2">
      {item.fields && <div className="grid grid-cols-1 gap-3 text-slate-500 sm:grid-cols-2">{item.fields.map((field, i) => <div key={i}>{text(field.text)}</div>)}</div>}
      {item.text && text(item.text)}
    </section>;
    if (item.tag === 'note') return <div key={index} className="text-xs text-slate-500">{item.elements?.map((value, i) => <span key={i}>{value.content}</span>)}</div>;
    if (item.tag === 'action') return <div key={index}>{item.actions?.map((action, i) => <span key={i} className="inline-block rounded border border-slate-200 px-3 py-1.5 text-sm text-blue-700">{action.text.content}</span>)}</div>;
    return null;
  })}</CardFrame>;
}
