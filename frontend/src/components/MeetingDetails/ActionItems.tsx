'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ListTodo } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

type Member = { id: string; name: string };
type Action = {
  id: string; summary: string; evidence: string; ownerText: string; dueText: string;
  dueDate: string; dueTime: string; memberChatId: string; members: Member[];
};
type Receipt = { id: string; summary: string; status: 'sent' | 'failed' | 'sending' | 'unknown'; detail: string; taskGuid?: string; taskUrl?: string };
type Context = { extractionWarning?: string | null; sourceHash: string; revision: string; items: Action[]; receipts: Receipt[]; authorized: boolean; ownerId: string };
type Preview = { id: string; items: Action[]; ownerId: string };
const inputClass = 'w-full rounded-md border bg-background px-3 py-2 text-sm';

export function ActionItems({ meetingId, saveEdits, disabled }: { meetingId: string; saveEdits: () => Promise<void>; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [context, setContext] = useState<Context | null>(null);
  const [items, setItems] = useState<Action[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [groups, setGroups] = useState<{ id: string; name: string }[]>([]);
  const [chatId, setChatId] = useState('');
  const [members, setMembers] = useState<Member[]>([]);
  const [memberError, setMemberError] = useState('');
  const [loadingMembers, setLoadingMembers] = useState(false);
  const previewRef = useRef<HTMLElement>(null);
  const lock = useRef(false);
  useEffect(() => {
    if (!preview) return;
    previewRef.current?.scrollIntoView({ block: 'nearest' });
    previewRef.current?.focus({ preventScroll: true });
  }, [preview]);
  const dirty = !!context && JSON.stringify(items) !== JSON.stringify(context.items);
  const blocked = (id: string) => context?.receipts.some(r => r.id === id && r.status !== 'failed') ?? false;
  function invalidate() { setPreview(null); setConfirmed(false); setNotice(''); }
  function install(value: Context, reset = true) {
    setContext(value); setItems(value.items); invalidate();
    if (reset) setSelected([]);
  }
  async function run(work: () => Promise<void>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(String(e)); invalidate(); }
    finally { lock.current = false; setBusy(false); }
  }
  async function load() {
    const value = await invoke<Context>('lark_actions_extract', { meetingId }); install(value);
  }
  async function prepare() {
    await run(async () => {
      setOpen(true); setContext(null); setItems([]); setSelected([]); invalidate(); setGroups([]); setChatId(''); setMembers([]); setMemberError('');
      await saveEdits(); await invoke('summary_export_markdown', { meetingId }); await load();
    });
  }
  async function save(): Promise<Context> {
    if (!context) throw new Error('请先提取行动项。');
    const value = await invoke<Context>('lark_actions_save', { meetingId, sourceHash: context.sourceHash, expectedRevision: context.revision, items });
    install(value, false); return value;
  }
  function edit(id: string, update: Partial<Action>) {
    setItems(current => current.map(item => item.id === id ? { ...item, ...update } : item)); invalidate();
  }
  async function loadMembers(id: string) {
    if (lock.current) return;
    setChatId(id); setMembers([]); setMemberError(''); invalidate();
    if (!id) return;
    await run(async () => {
      setLoadingMembers(true);
      try {
        setMembers(await invoke<Member[]>('lark_actions_members', { chatId: id }));
      } catch (e) {
        const message = String(e);
        setMemberError(message.includes('99991672')
          ? '群成员读取被 Lark 拒绝（99991672）。请管理员在 Lark 开放平台开通应用身份权限 im:chat.members:read，发布并获批后重试。'
          : message);
      } finally {
        setLoadingMembers(false);
      }
    });
  }
  function close() {
    if (lock.current) return;
    if (dirty) { setError('行动项修改尚未保存，请先保存草稿或放弃修改。'); return; }
    setOpen(false); setSelected([]); invalidate();
  }
  return <>
    <Button size="sm" variant="ghost" className="h-8 w-8 rounded-full p-0" aria-label="行动项" title="行动项 · 确认后同步 Lark 任务" disabled={disabled || busy} onClick={() => void prepare()}><ListTodo className="h-4 w-4" /></Button>
    <Dialog open={open} onOpenChange={next => { if (!next) close(); }}>
      <DialogContent className="flex w-[calc(100vw-2rem)] max-w-5xl max-h-[92vh] flex-col overflow-hidden rounded-2xl" onEscapeKeyDown={e => { if (dirty || busy) e.preventDefault(); }} onPointerDownOutside={e => { if (dirty || busy) e.preventDefault(); }}>
        <DialogHeader className="shrink-0"><DialogTitle>行动项 · 核对后同步任务</DialogTitle><DialogDescription>生成纪要时默认从转写提取行动项，模板没有行动项栏目也会单独保存。旧会议仍可从纪要表格或列表提取。默认不勾选，人工确认后才同步任务。</DialogDescription></DialogHeader>
        {(busy || error || notice) && <div className="max-h-28 shrink-0 overflow-y-auto" aria-label="行动项处理状态" aria-busy={busy}>
          {busy && <p role="status">正在处理，请稍候…</p>}
          {error && <p role="alert" className="whitespace-pre-wrap text-sm text-red-700">{error}</p>}
          {notice && <p role="status" className="text-sm text-green-700">{notice}</p>}
        </div>}
        <div className="min-h-0 flex-1 overflow-y-auto" aria-label="行动项内容">
        <div className="space-y-4">
        {context && <>
          {context.extractionWarning && <p role="alert" className="rounded-lg border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900">{context.extractionWarning}</p>}
          <div className="flex flex-wrap items-center gap-2 rounded-lg border p-3 text-sm">
            <p className="flex-1">{context.authorized ? '已连接 Lark 任务：以当前登录账号创建。' : '同步前需连接 Lark 任务；提取和保存草稿可以先做。'}</p>
            <Button variant="outline" disabled={busy} onClick={() => void run(async () => {
              const url = await invoke<string>('lark_app_authorize', { tasksOnly: true });
              await invoke('open_external_url', { url }); setNotice('请在浏览器完成授权，再点击“刷新授权”。应用需先开通并发布 task:task:write 用户权限。');
            })}>{context.authorized ? '重新授权任务' : '连接 Lark 任务'}</Button>
            <Button variant="ghost" disabled={busy || dirty} onClick={() => void run(load)}>刷新授权</Button>
          </div>
          <details className="rounded-lg border p-3 text-sm"><summary className="cursor-pointer font-medium">匹配负责人（需要时选择群成员）</summary>
            <p className="my-2 text-xs text-muted-foreground">群仅用于查找人员，不会向群发送消息。同名人员需核对 ID。可选择多个负责人；不自动把当前账号填为负责人。</p>
            <div className="flex flex-wrap gap-2">
              <Button variant="outline" disabled={busy} onClick={() => void run(async () => { setGroups(await invoke('lark_manual_groups')); })}>获取可用群</Button>
              <select aria-label="负责人来源群" className={`${inputClass} flex-1`} value={chatId} disabled={busy} onChange={e => void loadMembers(e.target.value)}><option value="">选择会议群</option>{groups.map(g => <option key={g.id} value={g.id}>{g.name}</option>)}</select>
              {chatId && !loadingMembers && !members.length && <Button variant="outline" disabled={busy} onClick={() => void loadMembers(chatId)}>重试获取群成员</Button>}
            </div>
            {memberError && <p role="alert" className="mt-2 text-sm text-red-700">{memberError}</p>}
          </details>
          <p className="text-xs text-muted-foreground">负责人、截止时间未提到则留空。相对日期保留原文，请确认具体日期；填写时刻按北京时间处理。不支持的格式可手动补充，未提取到不代表会议没有待办。</p>
          {!items.length && !context.extractionWarning && <p className="rounded-lg bg-muted p-4 text-sm">未找到明确的待办事项。旧会议可重新生成纪要，从转写独立提取行动项；也可以手动补充。未提到的负责人和截止时间留空。</p>}
          <div className="space-y-3">{items.map((item, index) => <section key={item.id} className="space-y-3 rounded-lg border p-4" aria-label={`行动项 ${index + 1}`}>
            <label className="flex items-center gap-2 text-sm font-medium"><input type="checkbox" aria-label={`同步行动项 ${index + 1}`} checked={selected.includes(item.id)} disabled={busy || blocked(item.id)} onChange={e => { setSelected(current => e.target.checked ? [...current, item.id] : current.filter(id => id !== item.id)); invalidate(); }} />{blocked(item.id) ? '已同步或待核对，不重复创建' : '勾选此项同步'}</label>
            <fieldset disabled={busy || blocked(item.id)} className="space-y-3">
              <label className="block text-sm">事项<textarea aria-label={`事项 ${index + 1}`} className={inputClass} rows={2} value={item.summary} onChange={e => edit(item.id, { summary: e.target.value })} /></label>
              <details className="text-xs text-muted-foreground"><summary className="cursor-pointer">{item.evidence ? '查看纪要原文依据' : '人工补充'}</summary><p className="mt-1 whitespace-pre-wrap break-words">{item.evidence || '此项由你手动补充，请核对会议事实。'}</p></details>
              <div className="grid gap-3 md:grid-cols-2">
                <label className="block text-sm">负责人原文<input aria-label={`负责人原文 ${index + 1}`} className={inputClass} placeholder="未提到则留空" value={item.ownerText} onChange={e => edit(item.id, { ownerText: e.target.value })} /></label>
                <label className="block text-sm">匹配 Lark 负责人<select aria-label={`负责人 ${index + 1}`} className={inputClass} value="" disabled={!members.length} onChange={e => {
                  const member = members.find(m => m.id === e.target.value); if (!member) return;
                  const current = item.memberChatId === chatId ? item.members : [];
                  edit(item.id, { memberChatId: chatId, members: current.some(m => m.id === member.id) ? current : [...current, member] });
                }}><option value="">{members.length ? '选择人员，可多次添加' : loadingMembers ? '正在读取群成员…' : memberError ? '群成员读取失败，请在上方重试' : chatId ? '该群没有可选成员' : '请先在上方选择来源群'}</option>{members.map(m => <option key={m.id} value={m.id}>{m.name}（{m.id.slice(-8)}）</option>)}</select></label>
              </div>
              <div className="flex flex-wrap gap-2 text-xs">{item.members.length ? item.members.map(m => <button key={m.id} type="button" className="rounded-full border px-2 py-1" title={m.id} onClick={() => edit(item.id, { members: item.members.filter(v => v.id !== m.id) })}>{m.name} ×</button>) : <span className="text-muted-foreground">未指定负责人</span>}</div>
              <div className="grid gap-3 md:grid-cols-3">
                <label className="block text-sm">截止原文<input aria-label={`截止原文 ${index + 1}`} className={inputClass} placeholder="未提到则留空" value={item.dueText} onChange={e => edit(item.id, { dueText: e.target.value })} /></label>
                <label className="block text-sm">确认截止日期<input type="date" aria-label={`截止日期 ${index + 1}`} className={inputClass} value={item.dueDate} onChange={e => edit(item.id, { dueDate: e.target.value, ...(!e.target.value ? { dueTime: '' } : {}) })} /></label>
                <label className="block text-sm">具体时刻（可不填，北京时间）<input type="time" aria-label={`截止时刻 ${index + 1}`} className={inputClass} value={item.dueTime} disabled={!item.dueDate} onChange={e => edit(item.id, { dueTime: e.target.value })} /></label>
              </div>
            </fieldset>
          </section>)}</div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" disabled={busy || items.length >= 100} onClick={() => { setItems(current => [...current, { id: `new-${crypto.randomUUID()}`, summary: '', evidence: '', ownerText: '', dueText: '', dueDate: '', dueTime: '', memberChatId: '', members: [] }]); invalidate(); }}>补充行动项</Button>
            <Button variant="outline" disabled={busy || !dirty} onClick={() => void run(async () => { await save(); setNotice('行动项草稿已保存，仅保存在本机。'); })}>保存待办草稿</Button>
            {dirty && <Button variant="ghost" disabled={busy} onClick={() => { setItems(context.items); setSelected([]); setError(''); invalidate(); }}>放弃未保存修改</Button>}
            <Button disabled={busy || !context.authorized || !selected.length} onClick={() => void run(async () => {
              const saved = dirty ? await save() : context;
              const value = await invoke<Preview>('lark_actions_preview', { meetingId, sourceHash: saved.sourceHash, expectedRevision: saved.revision, selectedIds: selected });
              setPreview(value); setConfirmed(false);
            })}>预览同步（{selected.length}）</Button>
          </div>
          {!context.authorized ? <p className="text-sm text-muted-foreground">请先在上方连接 Lark 任务，完成授权后点击“刷新授权”，再预览同步。</p> : !selected.length && <p className="text-sm text-muted-foreground">请先勾选需要同步的行动项，再预览同步。</p>}
          {preview && <section ref={previewRef} tabIndex={-1} className="space-y-3 rounded-lg border border-blue-300 bg-blue-50/50 p-4" aria-label="同步预览">
            <p className="font-medium">将创建 {preview.items.length} 条 Lark 任务</p>
            {preview.items.some(item => Array.from(item.evidence).length > 2000) && <p className="text-sm text-amber-800">较长的原文依据会在任务描述中附前 2,000 字并注明节选，完整依据保留在 Meetily。</p>}
            <p className="text-xs text-muted-foreground">创建账号：{preview.ownerId}。创建后 Lark 可能通知所选负责人；不会向会议群发送消息。</p>
            <ul className="space-y-2 text-sm">{preview.items.map(item => <li key={item.id}><p className="font-medium">{item.summary}</p><p>负责人：{item.members.map(m => m.name).join('、') || '留空'}；截止：{item.dueDate ? `${item.dueDate}${item.dueTime ? ` ${item.dueTime}（北京时间）` : '（全天）'}` : '留空'}</p></li>)}</ul>
            <label className="flex items-center gap-2 text-sm"><input type="checkbox" aria-label="已人工核对行动项" checked={confirmed} disabled={busy} onChange={e => setConfirmed(e.target.checked)} />我已逐条核对事项、负责人和截止时间</label>
            <Button disabled={busy || !confirmed} onClick={() => void run(async () => {
              const id = preview.id; setPreview(null); setConfirmed(false); setSelected([]);
              const value = await invoke<Context>('lark_actions_sync', { previewId: id, confirmed: true });
              install(value); setNotice('本次同步已结束，请查看下方逐条结果；失败或未执行的条目不会自动重试。');
            })}>确认同步到 Lark 任务</Button>
          </section>}
          {!!context.receipts.length && <section className="space-y-2 rounded-lg border p-3" aria-label="任务同步记录"><p className="font-medium">同步记录（当前账号）</p>{context.receipts.map(r => <div key={r.id} className="border-t pt-2 text-sm"><p>{r.summary} · {r.status === 'sent' ? '已同步' : r.status === 'failed' ? '未成功' : '结果待核对'}</p><p className="text-xs text-muted-foreground">{r.detail || '上次请求中断，请先在 Lark 任务中核对；不会自动重试。'}</p>{r.taskUrl ? <Button size="sm" variant="link" disabled={busy} onClick={() => void run(async () => { await invoke('open_external_url', { url: r.taskUrl }); })}>打开任务</Button> : r.taskGuid && <p className="text-xs">任务 ID：{r.taskGuid}</p>}</div>)}</section>}
        </>}
        </div>
        </div>
        <Button className="shrink-0" variant="ghost" disabled={busy} onClick={close}>关闭</Button>
      </DialogContent>
    </Dialog>
  </>;
}
