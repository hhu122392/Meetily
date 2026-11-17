'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

type MarkdownFile = { name: string; path: string; content: string; hash: string; size: number };
export function MarkdownFileEditor({ meetingId, onClose, onSaved, onSendPreview }: {
  meetingId: string; onClose: () => void; onSaved: (path: string) => void; onSendPreview?: () => void;
}) {
  const [file, setFile] = useState<MarkdownFile | null>(null);
  const [content, setContent] = useState('');
  const [filename,setFilename]=useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const [view, setView] = useState<'preview' | 'source'>('preview');
  const [history, setHistory] = useState<Array<{ name: string; size: number }> | null>(null);
  const [version, setVersion] = useState<{ name: string; content: string; hash: string } | null>(null);
  const [confirmRestore, setConfirmRestore] = useState(false);
  const currentMeeting = useRef(meetingId); currentMeeting.current = meetingId;
  const lock = useRef(false);
  const dirty = file !== null && (content !== file.content||filename!==file.name);
  useEffect(() => {
    let active = true; setFile(null); setContent(''); setHistory(null); setVersion(null); setConfirmRestore(false);
    void invoke<MarkdownFile>('summary_markdown_read', { meetingId }).then(v => { if (active) { setFile(v); setContent(v.content);setFilename(v.name); } }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, [meetingId]);
  function close() {
    if (busy) return;
    if (dirty) { setError('内容尚未保存，请先保存，或点击“放弃未保存更改”。'); return; }
    onClose();
  }
  async function save(sendPreview: boolean) {
    if (!file || lock.current || !content.trim()) return;
    lock.current = true; setBusy(true); setError('');
    try {
      let updated = await invoke<MarkdownFile>('summary_markdown_save', { meetingId, expectedHash: file.hash, content });
      setFile(updated);setContent(updated.content);
      if(filename!==updated.name)updated=await invoke<MarkdownFile>('summary_markdown_rename',{meetingId,expectedHash:updated.hash,expectedName:updated.name,name:filename});
      setFilename(updated.name);
      setFile(updated); setContent(updated.content); setSaved(true); onSaved(updated.path);
      if (sendPreview) onSendPreview?.();
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  async function loadHistory(name?: string) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(''); setConfirmRestore(false);
    try {
      if (name) {
        const result = await invoke<{ name: string; content: string; hash: string }>('summary_markdown_history_read', { meetingId, version: name });
        if (currentMeeting.current === meetingId) setVersion(result);
      } else {
        const result = await invoke<Array<{name: string; size: number}>>('summary_markdown_history_list', { meetingId });
        if (currentMeeting.current === meetingId) setHistory(result);
      }
    } catch (e) { if (currentMeeting.current === meetingId) setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  async function restore() {
    if (!file || !version || dirty || lock.current || !confirmRestore) return;
    lock.current = true; setBusy(true); setError('');
    try {
      const updated = await invoke<MarkdownFile>('summary_markdown_history_restore', { meetingId, version: version.name, expectedHash: file.hash, versionHash: version.hash });
      if (currentMeeting.current !== meetingId) return;
      setFile(updated); setContent(updated.content);setFilename(updated.name); setSaved(true); setVersion(null); setHistory(null); setConfirmRestore(false); onSaved(updated.path);
    } catch (e) { if (currentMeeting.current === meetingId) setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  return <Dialog open onOpenChange={value => { if (!value) close(); }}>
    <DialogContent className="max-w-5xl max-h-[90vh] overflow-y-auto" onEscapeKeyDown={e => { if (dirty || busy) e.preventDefault(); }} onPointerDownOutside={e => { if (dirty || busy) e.preventDefault(); }}>
      <DialogHeader><DialogTitle>详细纪要 · Markdown</DialogTitle><DialogDescription>预览与源码使用同一份详细纪要，表格会在预览中显示。单独编辑文件后不会同步修改纪要面板；重新生成会保留旧文件版本。云文档使用这份完整纪要，群卡片显示概要。</DialogDescription></DialogHeader>
      {file && <>
        <label>Markdown 文件名<input aria-label="Markdown 文件名" className="w-full rounded border p-2" value={filename} maxLength={120} disabled={busy} onChange={e=>{setFilename(e.target.value);setSaved(false);}}/></label>
        <p className="break-all text-xs text-gray-600">{file.path}</p>
        <div className="flex gap-2" aria-label="纪要查看方式">
          <Button variant={view === 'preview' ? 'default' : 'outline'} aria-pressed={view === 'preview'} onClick={() => setView('preview')}>预览排版</Button>
          <Button variant={view === 'source' ? 'default' : 'outline'} aria-pressed={view === 'source'} onClick={() => setView('source')}>编辑源码</Button>
        </div>
        {view === 'source' ? <textarea aria-label="Markdown 文件内容" className="w-full min-h-[45vh] rounded border p-3 font-mono text-sm resize-y" value={content} disabled={busy} onChange={e => { setContent(e.target.value); setSaved(false); }} /> :
          <div aria-label="Markdown 排版预览" className="prose prose-sm dark:prose-invert max-w-none min-h-[45vh] overflow-x-auto rounded border p-4 [&_table]:w-full [&_table]:border-collapse [&_th]:border [&_th]:bg-muted [&_th]:p-2 [&_th]:text-left [&_td]:border [&_td]:p-2 [&_pre]:overflow-x-auto">
            <ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={{img: ({alt}) => <span>［图片：{alt || '未命名'}］</span>, a: ({children}) => <span className="text-blue-600 underline">{children}</span>}}>{content}</ReactMarkdown>
          </div>}
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy || !content.trim()} onClick={() => void save(false)}>{busy ? '正在保存…' : '保存文件'}</Button>
          {onSendPreview && <Button variant="outline" disabled={busy || !content.trim()} onClick={() => void save(true)}>保存并编辑消息</Button>}
          <Button variant="outline" disabled={busy} onClick={() => void loadHistory()}>历史版本</Button>
          <Button variant="outline" disabled={busy} onClick={close}>返回</Button>
          {dirty && <Button variant="ghost" disabled={busy} onClick={onClose}>放弃未保存更改</Button>}
        </div>
      </>}
      {history && <section className="rounded border p-3">
        <h3>历史 Markdown 版本</h3>
        <p className="text-xs text-gray-500">恢复只替换详细 Markdown 文件，不改变消息草稿，也不会发送到 Lark。当前文件会先备份。</p>
        {!history.length && <p>暂无历史版本。</p>}
        <select aria-label="历史版本" value={version?.name ?? ''} disabled={busy} onChange={e => { if (e.target.value) void loadHistory(e.target.value); }} className="my-2 w-full rounded border p-2">
          <option value="">选择要查看的版本</option>
          {history.map(item => <option key={item.name} value={item.name}>{item.name} · {Math.ceil(item.size / 1024)} KB</option>)}
        </select>
        {version && <>
          <textarea aria-label="历史版本内容" readOnly value={version.content} className="min-h-48 w-full rounded border p-2 font-mono text-sm" />
          {dirty && <p className="text-amber-700">当前编辑尚未保存，请先保存后恢复。</p>}
          {!confirmRestore ? <Button disabled={busy || dirty} onClick={() => setConfirmRestore(true)}>恢复此版本</Button> : <div>
            <p>确认恢复这个版本？当前文件将先备份。</p>
            <Button disabled={busy || dirty} onClick={() => void restore()}>确认恢复</Button>
            <Button variant="ghost" disabled={busy} onClick={() => setConfirmRestore(false)}>取消恢复</Button>
          </div>}
        </>}
      </section>}
      {!file && !error && <p role="status">正在读取 Markdown 文件…</p>}
      {saved && <p role="status" className="text-sm text-green-700">文件已保存，旧内容已保留版本；尚未发送到 Lark。</p>}
      {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    </DialogContent>
  </Dialog>;
}
