'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

type MarkdownFile = { name: string; path: string; content: string; hash: string; size: number };
export function MarkdownFileEditor({ meetingId, onClose, onSaved, onSendPreview }: {
  meetingId: string; onClose: () => void; onSaved: (path: string) => void; onSendPreview?: () => void;
}) {
  const [file, setFile] = useState<MarkdownFile | null>(null);
  const [content, setContent] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const lock = useRef(false);
  const dirty = file !== null && content !== file.content;
  useEffect(() => {
    let active = true;
    void invoke<MarkdownFile>('summary_markdown_read', { meetingId }).then(v => { if (active) { setFile(v); setContent(v.content); } }).catch(e => { if (active) setError(String(e)); });
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
      const updated = await invoke<MarkdownFile>('summary_markdown_save', { meetingId, expectedHash: file.hash, content });
      setFile(updated); setContent(updated.content); setSaved(true); onSaved(updated.path);
      if (sendPreview) onSendPreview?.();
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  return <Dialog open onOpenChange={value => { if (!value) close(); }}>
    <DialogContent className="max-w-5xl max-h-[90vh] overflow-y-auto" onEscapeKeyDown={e => { if (dirty || busy) e.preventDefault(); }} onPointerDownOutside={e => { if (dirty || busy) e.preventDefault(); }}>
      <DialogHeader><DialogTitle>编辑 Markdown 文件</DialogTitle><DialogDescription>这里修改的是将要发送的详细纪要文件。单独编辑后，保存页面纪要不会覆盖它；重新生成纪要时会保留旧文件版本。Lark 卡片会从该文件提取概要。</DialogDescription></DialogHeader>
      {file && <>
        <p className="break-all text-xs text-gray-600">{file.path}</p>
        <textarea aria-label="Markdown 文件内容" className="w-full min-h-[45vh] rounded border p-3 font-mono text-sm resize-y" value={content} disabled={busy} onChange={e => { setContent(e.target.value); setSaved(false); }} />
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy || !content.trim()} onClick={() => void save(false)}>{busy ? '正在保存…' : '保存文件'}</Button>
          {onSendPreview && <Button variant="outline" disabled={busy || !content.trim()} onClick={() => void save(true)}>保存并编辑消息</Button>}
          <Button variant="outline" disabled={busy} onClick={close}>返回</Button>
          {dirty && <Button variant="ghost" disabled={busy} onClick={onClose}>放弃未保存更改</Button>}
        </div>
      </>}
      {!file && !error && <p role="status">正在读取 Markdown 文件…</p>}
      {saved && <p role="status" className="text-sm text-green-700">文件已保存，旧内容已保留版本；尚未发送到 Lark。</p>}
      {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    </DialogContent>
  </Dialog>;
}
