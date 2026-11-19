'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { LarkCredentialImport } from '@/components/LarkCredentialImport';

type SyncGroup = {id:string; name:string; since?:number};
type AppStatus = {
  app_id: string; has_secret: boolean; enabled: boolean; auto_send: boolean;
  manual_mode?: boolean;
  sync_groups?: SyncGroup[];
  commands?: {id:string; chat_id:string; meeting_no:string|null; detail:string; ack_state:string}[];
  authorized: boolean; owner_id: string; connection: string;
  auto_record?: boolean; automation?: string | null;
  recording_source?: 'cloud' | 'local';
  allow_group_launch?: boolean;
  invitations?: { id: string; topic: string; start: number; meeting_no: string | null; chat_id: string; status: string; detail: string }[];
  authorization_status?: string;
  routes: { reference: string; chat_id: string; remote_id: string | null }[];
  launches?: { request_id: string; chat_id: string; topic: string; status: string; detail: string; meeting_no: string | null; meeting_url: string | null }[];
  deliveries: { meeting_id: string; chat_id: string; status: string; detail: string }[];
  recording: { remote_id: string | null; meeting_no: string | null; topic: string | null; detail: string; blocked: boolean; stopped: boolean } | null;
};

const deliveryLabels: Record<string, string> = {
  queued: '等待发送', sending: '正在发送或上次发送中断，请核对群消息',
  sent: '已发送', failed: '发送失败', unknown: '结果不确定，请核对群消息',
};
const launchLabels: Record<string, string> = {
  queued: '等待创建', creating: '正在创建', ready: '已创建，等待发送链接',
  link_sending: '正在发送链接', sent: '链接已发送', failed: '创建失败',
  link_failed: '已创建，链接发送失败', unknown: '结果不确定，请核对', expired: '请求已过期',
};

export function LarkAppSettings() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [appId, setAppId] = useState('');
  const [secret, setSecret] = useState('');
  const [editingSecret, setEditingSecret] = useState(false);
  const [enabled, setEnabled] = useState(false);
  const [manualMode, setManualMode] = useState(false);
  const [autoSend, setAutoSend] = useState(false);
  const [autoRecord, setAutoRecord] = useState(false);
  const recordingSource = 'local';
  const allowGroupLaunch = false;
  const [syncGroups, setSyncGroups] = useState<SyncGroup[]>([]);
  const [availableGroups, setAvailableGroups] = useState<SyncGroup[]>([]);
  const [groupSearch, setGroupSearch] = useState('');
  const groupIdentity = useRef('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const lock = useRef(false);
  const initialized = useRef(false);
  const matchesSavedApp = !!status && appId.trim() === status.app_id;
  const hasSavedSecret = matchesSavedApp && !!status?.has_secret;
  const showSavedSecret = hasSavedSecret && !editingSecret;
  const loginAvailable = matchesSavedApp && !!status?.authorized;

  const refresh = useCallback(async () => {
    const value = await invoke<AppStatus>('lark_app_status');
    if (!initialized.current) {
      initialized.current = true;
      setAppId(value.app_id); setEnabled(value.enabled); setAutoSend(value.auto_send);
      setManualMode(!!value.app_id && !!value.manual_mode);
      setAutoRecord(value.auto_record ?? false);
      setSyncGroups(value.sync_groups ?? []);
    }
    const identity = `${value.app_id}:${value.owner_id}`;
    if (groupIdentity.current && groupIdentity.current !== identity) { setSyncGroups(value.sync_groups ?? []); setAvailableGroups([]); }
    groupIdentity.current = identity;
    setStatus(value);
  }, []);

  useEffect(() => {
    let disposed = false;
    const update = () => { if (!disposed) void refresh().catch(e => { if (!disposed) setError(typeof e === 'string' ? `无法读取 Lark 配置：${e} 原配置已保留，请恢复有效配置备份后重启。` : '无法读取企业应用配置，请稍后重试。'); }); };
    update();
    const timer = setInterval(update, 5000);
    return () => { disposed = true; clearInterval(timer); };
  }, [refresh]);

  async function perform(action: () => Promise<unknown>) {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError('');
    try { await action(); await refresh(); }
    catch (e) { setError(typeof e === 'string' ? e : '操作失败，请重试。'); }
    finally { lock.current = false; setBusy(false); }
  }

  return <section className="space-y-4 py-4 max-w-2xl border-b mb-6 pb-6">
    <div>
      <h2 className="text-lg font-semibold">Lark 会议助手</h2>
      <p className="text-sm text-gray-600 mt-2">推荐本地录音：在自己电脑录制双方声音、转写和生成纪要，审核后选择群，由会议助手发送。</p>
    </div>
    <label className="block space-y-1"><span>使用方式</span><select aria-label="会议助手使用方式" className="w-full rounded border p-2" disabled={busy} value={manualMode ? 'manual' : 'automatic'} onChange={e => {
      const manual = e.target.value === 'manual'; setManualMode(manual);
      if (manual) { setAutoRecord(false); }
    }}>
      <option value="automatic">同步群会议 · 本机自动录音（默认）</option>
      <option value="manual">本地录音 · 手动选群发送</option>
    </select></label>
    <p className="text-xs text-gray-500">每人登录自己的 Lark 账号、选择自己的会议群；录音保存在各自电脑，无需统一服务。</p>

    <LarkCredentialImport appId={appId} secretReady={!!secret.trim() || hasSavedSecret}
      disabled={busy || !initialized.current} onImport={credentials => {
        if (credentials.appId !== undefined && credentials.appId !== appId.trim()) {
          setAppId(credentials.appId); setSecret(''); setEditingSecret(false); setSyncGroups([]); setAvailableGroups([]);
        }
        if (credentials.secret !== undefined) { setSecret(credentials.secret); setEditingSecret(true); }
        setError('');
      }} />
    <div className="space-y-2">
      <Label htmlFor="lark-app-id">App ID</Label>
      <Input id="lark-app-id" placeholder="cli_…" value={appId} disabled={busy} onChange={e => {
        setAppId(e.target.value); setSecret(''); setEditingSecret(false); setSyncGroups([]); setAvailableGroups([]);
      }} />
      <Label htmlFor="lark-app-secret">App Secret</Label>
      <div className="flex items-center gap-2">
        <Input id="lark-app-secret" type="password" autoComplete="new-password"
          value={showSavedSecret ? '********' : secret} readOnly={showSavedSecret} disabled={busy}
          aria-describedby="lark-secret-description" placeholder="从 Lark 开发者后台复制新密钥"
          onChange={e => { if (!showSavedSecret) setSecret(e.target.value); }} />
        {hasSavedSecret && <Button variant="outline" disabled={busy} onClick={() => {
          setSecret(''); setEditingSecret(!editingSecret);
        }}>{editingSecret ? '取消更换' : '更换密钥'}</Button>}
      </div>
      <p id="lark-secret-description" className="text-xs text-gray-500">
        {hasSavedSecret ? 'App Secret 已加密保存。星号仅表示已配置，不会读取或显示真实密钥；不更换则继续使用原值。' : '尚未为当前 App ID 保存密钥，请填写后保存。'}
      </p>
      {!manualMode && <div role="status" className={`rounded-md border px-3 py-2 text-sm ${loginAvailable ? 'border-green-200 bg-green-50 text-green-800' : 'border-gray-200 bg-gray-50 text-gray-700'}`}>
        <p className="font-medium">登录状态：{!status ? (error ? '配置读取失败' : '正在读取…') : !matchesSavedApp ? 'App ID 已修改，请先保存并重新授权' : loginAvailable ? '已登录 · 授权可用' : '未登录或授权已失效'}</p>
        {matchesSavedApp && <p className="mt-1 text-xs">{status?.authorization_status || (loginAvailable ? 'Lark 账号已登录' : '请点击下方“授权会议助手”登录 Lark。')}</p>}
      </div>}
    </div>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={enabled} disabled={busy} onChange={e => setEnabled(e.target.checked)} />启用会议助手</label>
    {manualMode ? <p className="text-xs text-gray-600">使用本机麦克风和系统声音手动录音，会后在会议详情选择目标群并确认发送。需要指定会议号入会自动录音时，请选择「同步群会议 · 本机自动录音」，启用本机自动录音后在群里发送会议号。</p> : <>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={autoSend} disabled={busy} onChange={e => setAutoSend(e.target.checked)} />新录音生成纪要后准备发往唯一绑定群（需审核确认）</label>
    <label className="flex items-center gap-2 text-sm"><input id="lark-auto-record" type="checkbox" checked={autoRecord} disabled={busy} onChange={e => setAutoRecord(e.target.checked)} />本人加入已同步的指定会议后，自动在本机录音</label>
    <p className="text-xs text-gray-500">录制麦克风和系统声音，不下载云录音。逐群轮询，每轮至少 15 秒，群多或翻页时耗时增加；待参会和录音时约每 5 秒核验本人状态。请等录音提示后再开始讨论。</p>
    <div className="space-y-2 rounded border p-3">
      <div className="flex items-center justify-between"><Label>同步会议群</Label><Button variant="outline" disabled={busy || !loginAvailable} onClick={() => void perform(async () => setAvailableGroups(await invoke<SyncGroup[]>('lark_sync_groups')))}>加载我的群</Button></div>
      <p className="text-xs text-gray-500">已选择 {syncGroups.length} 个群，不再限制为 10 个。只列出本人和会议助手都已加入的群，群越多同步等待越长。首次保存后再发指令；只补读最近十分钟，避免旧消息启动录音。</p>
      {(availableGroups.length > 0 || syncGroups.length > 0) && <Input aria-label="搜索会议群" placeholder="搜索会议群" value={groupSearch} onChange={e => setGroupSearch(e.target.value)} />}
      <div className="max-h-60 overflow-y-auto space-y-2">{[...availableGroups, ...syncGroups.filter(g => !availableGroups.some(a => a.id === g.id))].filter(g => g.name.toLowerCase().includes(groupSearch.trim().toLowerCase())).map(g => <label key={g.id} className="flex gap-2 text-sm"><input type="checkbox" aria-label={g.name} checked={syncGroups.some(v => v.id === g.id)} disabled={busy} onChange={e => setSyncGroups(previous => e.target.checked ? [...previous, g] : previous.filter(v => v.id !== g.id))} />{g.name}</label>)}</div>
      {!availableGroups.length && !syncGroups.length && <p className="text-xs text-gray-500">登录后加载并选择群，再点击“保存并连接”。</p>}
    </div>
    <p className="text-xs text-gray-500">在所选群里实际 @会议助手，发送“会议ID:123 456 789”、入会链接或完整邀请，就会立即建立六小时有效的待参会绑定。任何群成员发指令都能同步；只有指令发送者的电脑负责回执。没有预约或到点提醒。多群绑定同一场会议时需手动录音并选群。</p>
    </>}
    <div className="flex gap-2 flex-wrap">
      <Button disabled={busy || !initialized.current || !appId.trim()} onClick={() => void perform(async () => {
        await invoke('lark_app_save', { appId, secret, enabled, autoSend, autoRecord, allowGroupLaunch, recordingSource, manualMode, syncGroups });
        setSecret(''); setEditingSecret(false);
      })}>保存并连接</Button>
      {!manualMode && <Button variant="outline" disabled={busy || !hasSavedSecret || !matchesSavedApp} onClick={() => void perform(async () => {
        const url = await invoke<string>('lark_app_authorize', { includeMeetingControl: true, includeCloudRecording: false });
        await invoke('open_external_url', { url });
      })}>授权会议助手</Button>}
      <Button variant="ghost" disabled={busy} onClick={() => void perform(refresh)}>刷新状态</Button>
    </div>
    {!manualMode && <p className="text-xs text-gray-500">升级后请重新授权，新增“以用户身份读取群消息”和群列表权限。应用凭证仍需在本机辅助导入，登录不会自动取得 App Secret。</p>}
    <div className="rounded-md bg-gray-50 p-3 text-sm space-y-2">
      <p>连接：{status?.connection || (error ? '配置读取失败' : '正在读取…')}</p>
      <p>已保存模式：{!status?.app_id ? '尚未配置' : status.manual_mode ? '本地录音 · 手动选群' : '同步群会议 · 本机自动录音'}</p>
      {!status?.manual_mode && <p>Lark 账号：{status?.authorization_status || (status?.authorized ? '已登录' : '未登录')}</p>}
      <p>自动录音：{status?.auto_record ? (status.recording_source === 'cloud' ? '已开启，适用于助手新建的会议' : '已开启，适用于所选群内已同步的会议号') : '未开启'}</p>
      {!!status?.app_id && <p>已保存录音来源：{status?.recording_source === 'cloud' ? '旧云录音配置，请保存切换为本机录音' : '本机录音'}</p>}
      <p>群同步：{status?.manual_mode ? '已关闭' : `已选择 ${status?.sync_groups?.length ?? 0} 个群`}</p>
      {status?.automation && <p>{status.automation}</p>}
      {status?.recording && <p>最近录音：{status.recording.topic || status.recording.remote_id || '未匹配'} — {status.recording.detail}</p>}
    </div>
    {!manualMode && <details className="text-sm space-y-2">
      <summary className="cursor-pointer font-medium">首次配置与使用步骤</summary>
      <ol className="list-decimal pl-5 space-y-2 mt-2">
        <li>管理员给同一个会议助手开通群列表、以用户身份读取群消息、本人会议状态及机器人发消息权限，发布后将助手加入会议群。</li>
        <li>每台电脑辅助导入相同应用凭证，回调地址配置为 http://127.0.0.1:18576/lark/callback。</li>
        <li>每人点击“授权会议助手”登录自己的账号，加载自己的群，勾选需要同步的群和自动录音，保存。</li>
        <li>在群内实际 @会议助手，发送已有会议号或入会链接，然后本人在这台电脑入会。不要只输入机器人名称。</li>
        <li>确认录音已开始后再讨论；离会后自动停止并完成转写和纪要，发送仍需本人审核。</li>
      </ol>
      <p>日程分享链接需要本人主日历可访问且能核对会议号；失败时改发九位会议号或视频会议入会链接。当前不创建预约、不按邀请时间等待或提醒。</p>
      <p>录音检测的是登录账号的参会状态，请在这台电脑加入会议；无法区分同一账号在其他设备上的参会。每个账号只在一台电脑开启自动录音。</p>
      <p>电脑关机、休眠、Meetily 退出或未获权限时无法同步与录音；不会从云端补回漏录的声音。群消息同步会消耗 Lark API 调用额度。</p>
    </details>}
    {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
    {!!status?.commands?.length && <div className="space-y-2"><h3 className="font-medium text-sm">最近群指令</h3>{status.commands.map(c => <div key={c.id} className="text-xs rounded border p-2"><p>{c.meeting_no || '未绑定'} · {c.detail}</p><p>来源群：{c.chat_id}</p><p>群回执：{c.ack_state === 'sent' ? '已发送' : c.ack_state === 'unknown' ? '送达不确定，请核对群消息；不会自动重发' : '由指令发送者的电脑负责'}</p></div>)}</div>}
    {!!status?.launches?.length && <div className="space-y-2">
      <h3 className="font-medium text-sm">最近开会请求</h3>
      {status.launches.map(l => <div className="text-xs border rounded p-2 space-y-1 break-all" key={l.request_id}>
        <p>{l.topic} · {launchLabels[l.status] || l.status}</p>
        <p>{l.detail}</p>
        <p>来源群：{l.chat_id}{l.meeting_no ? ` · 会议号：${l.meeting_no}` : ''}</p>
        {l.meeting_url && <Button variant="ghost" disabled={busy} onClick={() => void perform(() => invoke('open_external_url', { url: l.meeting_url }))}>打开会议链接</Button>}
      </div>)}
    </div>}
    {!!status?.routes.length && <div className="space-y-2">
      <h3 className="font-medium text-sm">有效群绑定</h3>
      {status.routes.map(r => <div key={`${r.reference}:${r.chat_id}`} className="text-xs border rounded p-2 flex gap-2 items-center justify-between">
        <span className="break-all">会议 {r.remote_id || r.reference}<br />群 {r.chat_id}</span>
        <Button variant="ghost" disabled={busy} onClick={() => void perform(() => invoke('lark_app_remove_route', { reference: r.reference, chatId: r.chat_id }))}>移除绑定</Button>
      </div>)}
    </div>}
    {!!status?.deliveries.length && <div className="space-y-2">
      <h3 className="font-medium text-sm">最近发送记录</h3>
      {status.deliveries.map(d => <p className="text-xs border rounded p-2 break-all" key={d.meeting_id}>
        {deliveryLabels[d.status] || d.status} · {d.detail}<br />会议 {d.meeting_id} → 群 {d.chat_id}
      </p>)}
    </div>}
  </section>;
}

