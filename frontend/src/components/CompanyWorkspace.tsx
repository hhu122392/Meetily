'use client';
import { createContext, useContext, useEffect, useState, useRef, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { Building2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { initializeCompanyScope } from '@/lib/company-scope';

type Company = { id: string; name: string; app_id: string; tenant_key: string };
type Status = { active: string; companies: Company[]; recording_root: string | null; warning?: string | null };
const CompanyContext = createContext<{ status: Status; refresh: () => Promise<Status> } | null>(null);

export function CompanyBoundary({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState('');
  async function refresh() {
    const next = await invoke<Status>('company_status');
    initializeCompanyScope(next.active);
    const previous = sessionStorage.getItem('company_session');
    if (previous && previous !== next.active) {
      sessionStorage.removeItem('indexeddb_current_meeting_id');
      sessionStorage.removeItem('autoStartRecording');
    }
    sessionStorage.setItem('company_session', next.active);
    setStatus(next); return next;
  }
  useEffect(() => { void refresh().catch(e => setError(String(e))); }, []);
  if (!status) return <div className="p-8 text-sm" role={error ? 'alert' : 'status'}>{error || '正在加载公司资料…'}{error && <Button className="ml-3" onClick={() => void relaunch().catch(e => setError(`重启失败，请手动关闭并重新打开 Meetily：${String(e)}`))}>重启 Meetily</Button>}</div>;
  return <CompanyContext.Provider value={{ status, refresh }}>{status.warning && <div role="alert" className="border-b border-amber-300 bg-amber-50 px-4 py-3 text-sm text-amber-900">{status.warning}</div>}{children}</CompanyContext.Provider>;
}

export function CompanySwitcher({ compact = false }: { compact?: boolean }) {
  const context = useContext(CompanyContext);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState('');
  const [newName, setNewName] = useState('');
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [error, setError] = useState('');
  const lock = useRef(false);
  if (!context) return null;
  const { status, refresh } = context;
  const current = status.companies.find(c => c.id === status.active)!;
  async function action(work: () => Promise<void>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(String(e)); }
    finally { lock.current = false; setBusy(false); }
  }
  function show() {
    setSelected(status.active); setName(current.name); setError(''); setOpen(true);
    void refresh().catch(e => setError(String(e)));
  }
  return <>
    <button className={`flex items-center gap-2 rounded-md hover:bg-gray-100 text-sm ${compact ? 'mx-auto p-2' : 'mx-3 px-3 py-2 w-[calc(100%-1.5rem)]'}`} onClick={show} title={`公司资料：${current.name}`}>
      <Building2 className="h-4 w-4 shrink-0" /><span className={compact ? 'sr-only' : 'truncate'}>{current.name}</span>
    </button>
    <Dialog open={open || restarting} onOpenChange={value => { if (!busy && !restarting) setOpen(value); }}>
      <DialogContent onEscapeKeyDown={e => { if (busy || restarting) e.preventDefault(); }} onPointerDownOutside={e => { if (busy || restarting) e.preventDefault(); }}>
        <DialogHeader><DialogTitle>公司资料</DialogTitle><DialogDescription>会议、转写、纪要和 Lark 配置按公司保存。同一公司换账号仍可查看本机该公司的资料。AI 模型共用。</DialogDescription></DialogHeader>
        <div className="space-y-4">
          <div><label className="text-sm" htmlFor="company-name">当前公司名称</label><div className="flex gap-2 mt-1"><Input id="company-name" value={name} maxLength={60} disabled={busy || restarting} onChange={e => setName(e.target.value)} /><Button variant="outline" disabled={busy || restarting || !name.trim() || name === current.name} onClick={() => void action(async () => { await invoke('company_rename', { name }); await refresh(); })}>保存名称</Button></div></div>
          <p className="text-xs text-gray-500">{current.tenant_key ? '已通过 Lark 登录校验公司身份' : '公司身份待下次 Lark 登录校验；现有资料和授权保留'}</p>
          <div><label className="text-sm" htmlFor="company-select">切换公司</label><select id="company-select" className="mt-1 w-full rounded-md border p-2 bg-white" value={selected} disabled={busy || restarting} onChange={e => setSelected(e.target.value)}>{status.companies.map(c => <option key={c.id} value={c.id}>{c.name}{c.id === status.active ? '（当前）' : ''}</option>)}</select></div>
          <p className="text-xs text-gray-500">切换将重启 Meetily。请先保存当前编辑；后台转写可在右下角暂停，等待显示“已暂停”后切换。录音、导入、生成或发送纪要期间不能切换。新公司的机器人需要单独配置并授权。</p>
          <Button disabled={busy || restarting || !selected || selected === status.active} onClick={() => void action(async () => { await invoke('company_switch', { id: selected }); setRestarting(true); })}>{restarting ? '正在重启并切换…' : '切换并重启'}</Button>
          <div className="border-t pt-4"><label className="text-sm" htmlFor="new-company">新增公司</label><div className="flex gap-2 mt-1"><Input id="new-company" placeholder="输入公司名称" value={newName} maxLength={60} disabled={busy || restarting} onChange={e => setNewName(e.target.value)} /><Button variant="outline" disabled={busy || restarting || !newName.trim()} onClick={() => void action(async () => { const id = await invoke<string>('company_create', { name: newName }); await refresh(); setSelected(id); setNewName(''); })}>新增</Button></div></div>
          <p className="text-xs text-gray-500 break-all">录音根目录：{status.recording_root || '初始化后显示'}。新增录音自动使用公司子目录，历史文件保留原路径。</p>
          <p className="text-xs text-gray-500">这里控制软件内的资料范围，未对录音和纪要文件加密。</p>
          {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
        </div>
      </DialogContent>
    </Dialog>
  </>;
}
