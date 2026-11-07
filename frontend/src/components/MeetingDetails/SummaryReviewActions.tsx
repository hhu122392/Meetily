'use client';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { SquarePen, Save, MessageSquare, Send, Info } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { MarkdownFileEditor } from './MarkdownFileEditor';
import { LarkCardPreview, type LarkPreviewCard } from './MinutesCardPreview';

type Draft = { chat_id: string; chat_name?: string | null; chat_name_error?: string; remote_id: string; text: string; card?: LarkPreviewCard; message_title?: string; message_summary?: string; message_edited?: boolean; revision: string; last_sent_at?: number | null; requires_duplicate_ack?: boolean; markdown?: { name: string; path: string; content: string; size: number } };
type ExportStatus = { path?: string; error?: string };
type DeliveryStatus = { linked: boolean; last_sent_at?: number | null; chat_name?: string | null };
const sentTime = (seconds: number) => new Date(seconds * 1000).toLocaleString('zh-CN', { hour12: false });
export function SummaryReviewActions({ meetingId, saveEdits, disabled, toolbarTarget }: {
  meetingId: string; saveEdits: () => Promise<void>; disabled?: boolean; toolbarTarget?: HTMLElement | null;
}) {
  const [file, setFile] = useState<ExportStatus>({});
  const [linked, setLinked] = useState(false);
  const [lastSentAt, setLastSentAt] = useState<number | null>(null);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [sent, setSent] = useState(false);
  const [duplicateAcknowledged, setDuplicateAcknowledged] = useState(false);
  const [includeMarkdown, setIncludeMarkdown] = useState(true);
  const [fileOpen, setFileOpen] = useState(false);
  const [messageTitle, setMessageTitle] = useState('');
  const [messageSummary, setMessageSummary] = useState('');
  const messageSummaryRef = useRef<HTMLTextAreaElement>(null);
  const messageDirty = !!draft && (messageTitle !== (draft.message_title ?? '') || messageSummary !== (draft.message_summary ?? ''));
  const lock = useRef(false);
  const hasDraft = !!draft;
  useEffect(() => {
    const input = messageSummaryRef.current;
    if (!open || !input) return;
    const fitHeight = () => {
      // Re-measure wrapped content, with room left for the title and controls.
      const maximum = Math.max(120, Math.floor(window.innerHeight * 0.55));
      const minimum = Math.min(192, maximum);
      input.style.height = 'auto';
      const contentHeight = input.scrollHeight + 2;
      input.style.height = `${Math.min(maximum, Math.max(minimum, contentHeight))}px`;
      input.style.overflowY = contentHeight > maximum ? 'auto' : 'hidden';
    };
    fitHeight();
    window.addEventListener('resize', fitHeight);
    return () => window.removeEventListener('resize', fitHeight);
  }, [messageSummary, open, hasDraft]);
  useEffect(() => {
    let active = true;
    setLastSentAt(null); setLinked(false); setOpen(false); setDraft(null); setFileOpen(false);
    const refresh = () => {
      void invoke<ExportStatus>('summary_markdown_status', { meetingId }).then(v => { if (active) setFile(v); }).catch(() => {});
      void invoke<DeliveryStatus>('lark_app_meeting_status', { meetingId }).then(v => {
        if (active) { setLinked(v.linked); setLastSentAt(previous => Math.max(previous ?? 0, v.last_sent_at ?? 0) || null); }
      }).catch(() => {});
    };
    refresh(); const timer = setInterval(refresh, 5000);
    return () => { active = false; clearInterval(timer); };
  }, [meetingId]);

  async function prepare(review: boolean, editFile = false) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError(''); setDraft(null); setSent(false); setDuplicateAcknowledged(false);
    setIncludeMarkdown(true);
    if (review) setOpen(true);
    try {
      await saveEdits();
      const path = await invoke<string>('summary_export_markdown', { meetingId });
      setFile({ path });
      if (editFile) setFileOpen(true);
      if (review) {
        const preview = await invoke<Draft>('lark_app_preview_summary', { meetingId });
        setDraft(preview);
        setMessageTitle(preview.message_title ?? ''); setMessageSummary(preview.message_summary ?? '');
        setLastSentAt(previous => Math.max(previous ?? 0, preview.last_sent_at ?? 0) || null);
      }
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  async function saveMessage() {
    if (!draft || lock.current || !messageTitle.trim() || !messageSummary.trim()) return;
    lock.current = true; setBusy(true); setError('');
    try {
      const updated = await invoke<Draft>('lark_app_edit_message', { meetingId, expectedRevision: draft.revision, title: messageTitle, summary: messageSummary });
      setDraft({ ...updated, chat_name: draft.chat_name, chat_name_error: draft.chat_name_error });
      setMessageTitle(updated.message_title ?? ''); setMessageSummary(updated.message_summary ?? '');
      setDuplicateAcknowledged(false);
    } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  function closeMessage() {
    if (lock.current) return;
    if (messageDirty) { setError('消息修改尚未保存，请先保存或放弃消息修改。'); return; }
    setOpen(false);
  }
  async function send() {
    if (!draft || lock.current || sent || messageDirty || (includeMarkdown && !draft.markdown) || (draft.requires_duplicate_ack && !duplicateAcknowledged)) return;
    lock.current = true; setBusy(true); setError('');
    try {
      const sentAt = await invoke<number>('lark_app_check_send', { meetingId, expectedRevision: draft.revision, acknowledgeDuplicateRisk: duplicateAcknowledged, includeMarkdown });
      setLastSentAt(sentAt);
      setSent(true);
    } catch (e) { setError(String(e)); setDraft(null); }
    finally { lock.current = false; setBusy(false); }
  }
  const actions = <>
      <Button className="h-8 w-8 rounded-full p-0" size="sm" variant="ghost" aria-label="编辑纪要" title="编辑纪要" disabled={disabled || busy} onClick={() => void prepare(false, true)}><SquarePen className="h-4 w-4" /><span className="sr-only">编辑纪要</span></Button>
      <Button className="h-8 w-8 rounded-full p-0" size="sm" variant="ghost" aria-label="保存 Markdown" title="保存 Markdown" disabled={disabled || busy} onClick={() => void prepare(false)}><Save className="h-4 w-4" /><span className="sr-only">保存 Markdown</span></Button>
      {linked && <>
        <Button className="h-8 w-8 rounded-full p-0" size="sm" variant="ghost" aria-label="编辑 Lark 消息" title="编辑 Lark 消息" disabled={disabled || busy} onClick={() => void prepare(true)}><MessageSquare className="h-4 w-4" /><span className="sr-only">编辑 Lark 消息</span></Button>
        <Button className="h-8 w-8 rounded-full p-0" size="sm" aria-label="发送到 Lark" title="发送到 Lark（预览后确认）" disabled={disabled || busy} onClick={() => void prepare(true)}><Send className="h-4 w-4" /><span className="sr-only">发送到 Lark</span></Button>
      </>}
      <Popover>
        <PopoverTrigger asChild><Button className="h-8 w-8 rounded-full p-0" size="sm" variant="ghost" aria-label="纪要文件与发送状态" title={linked ? (lastSentAt ? `上次发送时间：${sentTime(lastSentAt)}` : '尚未发送到 Lark') : '纪要文件信息'}><Info className="h-4 w-4" /></Button></PopoverTrigger>
        <PopoverContent align="end" className="w-80 max-w-[calc(100vw-2rem)] space-y-2 rounded-xl text-sm">
          <p className="font-medium">纪要文件与发送状态</p>
          {linked && <p>{lastSentAt ? `上次发送时间：${sentTime(lastSentAt)}` : '尚未发送到 Lark'}</p>}
          <p className="break-all text-xs text-gray-600">{file.path ? `Markdown 文件：${file.path}` : '保存后可查看 Markdown 文件位置。'}</p>
          <p className="text-xs text-gray-500">Lark 群消息使用独立的概要卡片，发送前需预览并确认。</p>
        </PopoverContent>
      </Popover>
  </>;
  return <>
    {toolbarTarget ? createPortal(actions, toolbarTarget) : <div className="flex flex-wrap items-center gap-0.5">{actions}</div>}
    {file.error && <p role="alert" className="px-4 py-2 text-sm text-red-700">{file.error}</p>}
    {!open && error && <p role="alert" className="px-4 py-2 text-sm text-red-700">{error}</p>}
    <Dialog open={open} onOpenChange={next => { if (!next) closeMessage(); }}>
      <DialogContent className="w-[calc(100vw-2rem)] max-w-7xl max-h-[92vh] overflow-y-auto rounded-2xl" onEscapeKeyDown={e => { if (messageDirty || busy) e.preventDefault(); }} onPointerDownOutside={e => { if (messageDirty || busy) e.preventDefault(); }}>
        <DialogHeader><DialogTitle>Lark 消息 · 编辑与确认发送</DialogTitle>
          <DialogDescription>请核对目标群和概要卡片。详细内容在 Markdown 文件中，可单独编辑；只有勾选发送文件时，确认后才会上传附件。</DialogDescription>
        </DialogHeader>
        {busy && !draft && <p role="status">正在保存并准备预览…</p>}
        {draft && <>
          <p className="break-all">目标群：<strong>{draft.chat_name || '暂时无法获取群名'}</strong></p>
          <p className="break-all text-xs text-gray-500">群 ID：{draft.chat_id}</p>
          {!draft.chat_name && <p className="text-amber-700">{draft.chat_name_error || '请检查网络及应用的读取群信息权限，并核对群 ID 后发送。'}</p>}
          {draft.last_sent_at && <p className="text-amber-700">此纪要已于 {sentTime(draft.last_sent_at)} 发送过；再次确认会在群内新增一条消息。</p>}
          <p>会议 ID：{draft.remote_id}</p>
          <section className="space-y-3 rounded-lg border bg-slate-50 p-3">
            <h3 className="font-medium">消息标题与概要</h3>
            <p className="text-xs text-gray-600">仅修改发群消息，不改动 Markdown 文件。详细纪要更新后会重新提取概要。</p>
            <label className="block space-y-1"><span>消息标题</span><input aria-label="消息标题" className="w-full rounded border bg-white p-2" maxLength={100} disabled={busy || sent} value={messageTitle} onChange={e => setMessageTitle(e.target.value)} /></label>
            <label className="block space-y-1"><span>消息概要</span><textarea ref={messageSummaryRef} aria-label="消息概要" className="block w-full resize-none rounded border bg-white p-3 text-sm leading-7" rows={6} maxLength={1500} disabled={busy || sent} value={messageSummary} onChange={e => setMessageSummary(e.target.value)} /></label>
            <p className="text-xs text-gray-500">支持文字、标题和列表，不使用表格。保存后，下方会显示实际待发送的卡片。</p>
            <div className="flex gap-2">
              <Button className="h-8 rounded-full" size="sm" variant="outline" disabled={busy || sent || !messageDirty || !messageTitle.trim() || !messageSummary.trim()} onClick={() => void saveMessage()}>保存消息并更新预览</Button>
              {messageDirty && <Button className="h-8 rounded-full" size="sm" variant="ghost" disabled={busy} onClick={() => { setMessageTitle(draft.message_title ?? ''); setMessageSummary(draft.message_summary ?? ''); setError(''); }}>放弃消息修改</Button>}
            </div>
            {!messageDirty && draft.message_edited && <p className="text-xs text-green-700">消息修改已保存，确认后才会发送。</p>}
          </section>
          {messageDirty && <p role="status" className="text-amber-700">消息有未保存的修改；请先保存并更新预览，再确认发送。</p>}
          <h3 className="font-medium">将发送的消息内容（卡片）</h3>
          <div className="max-h-[60vh] overflow-auto"><LarkCardPreview card={draft.card} fallback={draft.text} /></div>
          {draft.markdown ? <section className="space-y-2 rounded border p-3">
            <label className="flex items-center gap-2 font-medium"><input aria-label="发送 Markdown 文件附件" type="checkbox" checked={includeMarkdown} disabled={busy || sent} onChange={e => setIncludeMarkdown(e.target.checked)} />同时发送 Markdown 文件附件</label>
            {!includeMarkdown && <p className="text-xs text-gray-600">本次只发送概要卡片，不上传文件；详细纪要仍保存在本机。</p>}
            <p>{draft.markdown.name} · {draft.markdown.size.toLocaleString('zh-CN')} 字节</p>
            <p className="break-all text-xs text-gray-600">本地位置：{draft.markdown.path}</p>
            <Button className="h-8 rounded-full" size="sm" variant="outline" disabled={busy || sent || messageDirty} onClick={() => { setOpen(false); setFileOpen(true); }}>编辑 Markdown 文件</Button>
          </section> : <p role="alert" className="text-red-700">Markdown 文件尚未准备好，请返回重新保存。</p>}
          {draft.requires_duplicate_ack && <div className="space-y-2 rounded border border-amber-300 bg-amber-50 p-3">
            <p>上次发送结果不确定，卡片或附件可能已送达。请先核对群消息；再次发送可能产生重复消息。</p>
            <label className="flex items-center gap-2"><input aria-label="接受可能重复发送" type="checkbox" checked={duplicateAcknowledged} disabled={busy || sent} onChange={e => setDuplicateAcknowledged(e.target.checked)} />已核对群消息，接受可能重复并继续发送</label>
          </div>}
          <div className="flex gap-2">
            <Button variant="outline" disabled={busy} onClick={closeMessage}>返回编辑</Button>
            <Button disabled={busy || sent || messageDirty || (includeMarkdown && !draft.markdown) || (draft.requires_duplicate_ack && !duplicateAcknowledged)} onClick={() => void send()}>{sent ? '已发送' : busy ? '正在处理…' : (draft.last_sent_at || draft.requires_duplicate_ack) ? '确认再次发送' : '确认发送'}</Button>
          </div>
        </>}
        {error && <p role="alert" className="text-red-700">{error}</p>}
        {sent && <p role="status" className="text-green-700">{includeMarkdown ? '纪要卡片和 Markdown 附件均已发送到上述群。' : '概要卡片已发送到上述群，本次未发送文件。'}如需再次发送，请关闭此窗口后重新点击“发送到 Lark”。</p>}
      </DialogContent>
    </Dialog>
    {fileOpen && <MarkdownFileEditor meetingId={meetingId} onClose={() => setFileOpen(false)} onSaved={path => setFile({ path })} onSendPreview={linked ? () => { setFileOpen(false); void prepare(true); } : undefined} />}
  </>;
}
