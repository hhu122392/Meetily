'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { LarkCredentialImport } from '@/components/LarkCredentialImport';
import { CalendarRefreshSettings } from '@/components/CalendarRefreshSettings';

type AppStatus = {
  app_id: string; has_secret: boolean; enabled: boolean; auto_send: boolean;
  manual_mode?: boolean;
  authorized: boolean; owner_id: string; connection: string;
  auto_record?: boolean; automation?: string | null;
  recording_source?: 'cloud' | 'local';
  allow_group_launch?: boolean;
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
  const [manualMode, setManualMode] = useState(true);
  const [autoSend, setAutoSend] = useState(false);
  const [autoRecord, setAutoRecord] = useState(false);
  const [recordingSource, setRecordingSource] = useState<'cloud' | 'local'>('local');
  const [allowGroupLaunch, setAllowGroupLaunch] = useState(false);
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
      setManualMode(!value.app_id || !!value.manual_mode);
      setAutoRecord(value.auto_record ?? false);
      setRecordingSource((!value.app_id || value.manual_mode) ? 'local' : (value.recording_source ?? 'local'));
      setAllowGroupLaunch(value.allow_group_launch ?? false);
    }
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
      if (manual) { setRecordingSource('local'); setAutoRecord(false); setAllowGroupLaunch(false); }
    }}>
      <option value="manual">本地录音 · 手动选群发送（推荐）</option>
      <option value="automatic">群指令开会 · 自动关联（由指定电脑接收）</option>
    </select></label>
    {manualMode&&<div className="rounded border p-3 text-sm"><p>录音来源：本地录音（麦克风和系统声音）</p><p className="mt-1 text-xs text-gray-500">云录音配置保留在“群指令开会 · 自动关联”模式中。若需切换，请先修改上方使用方式，再选择录音来源、保存并重新授权。</p></div>}
    <CalendarRefreshSettings />
    <LarkCredentialImport appId={appId} secretReady={!!secret.trim() || hasSavedSecret}
      disabled={busy || !initialized.current} onImport={credentials => {
        if (credentials.appId !== undefined && credentials.appId !== appId.trim()) {
          setAppId(credentials.appId); setSecret(''); setEditingSecret(false);
        }
        if (credentials.secret !== undefined) { setSecret(credentials.secret); setEditingSecret(true); }
        setError('');
      }} />
    <div className="space-y-2">
      <Label htmlFor="lark-app-id">App ID</Label>
      <Input id="lark-app-id" placeholder="cli_…" value={appId} disabled={busy} onChange={e => {
        setAppId(e.target.value); setSecret(''); setEditingSecret(false);
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
    {manualMode ? <p className="text-xs text-gray-600">使用本机麦克风和系统声音；不启动或下载 Lark 云录音，不接收机器人开会事件。保存后回到首页「近期会议」，连接我的 Lark 日历即可同步周期日程。日历只申请读取权限，会后在会议详情选择目标群并确认发送。</p> : <>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={autoSend} disabled={busy} onChange={e => setAutoSend(e.target.checked)} />新录音生成纪要后准备发往唯一绑定群（需审核确认）</label>
    <label className="flex items-center gap-2 text-sm"><input id="lark-auto-record" type="checkbox" checked={autoRecord} disabled={busy} onChange={e => setAutoRecord(e.target.checked)} />{recordingSource === 'local' ? '加入助手新建或本人绑定的会议时自动录音' : '加入会议助手新建的会议时自动录音'}</label>
    <div className="space-y-2">
      <Label htmlFor="lark-recording-source">录音来源</Label>
      <select id="lark-recording-source" className="w-full rounded-md border bg-transparent p-2 text-sm" value={recordingSource} disabled={busy} onChange={e => setRecordingSource(e.target.value as 'cloud' | 'local')}>
        <option value="local">本机录音（默认，麦克风和系统声音）</option>
        <option value="cloud">下载 Lark 云录音（需手动选择）</option>
      </select>
      <p className="text-xs text-gray-500">保存后对下一场会议生效；已有任务继续使用原来源。</p>
      {recordingSource === 'cloud' ? <p className="text-xs text-gray-500">授权本人入会且为主持人时，自动请求 Lark 开始录制。整场会议结束后，等待云文件就绪，下载到当前公司的录音目录，再后台转写和生成纪要；不会录入电脑的其他声音。下载不使用 Lark 语音转文字额度。请保持 Meetily 运行；重启后会继续等待或下载。暂停任务不会停止 Lark 云录制。</p>
        : <p className="text-xs text-gray-500">已有会议：先保存自动录音设置，再用授权账号在群里发送 <code>@会议助手 绑定会议 123456789</code>，将示例号码换成本场会议号。保持 Meetily 运行，在这台电脑入会后录制麦克风和系统声音，确认离会后停止。手动停止后本场不再重启。</p>}
    </div>
    <label className="flex items-center gap-2 text-sm"><input id="lark-allow-group-launch" type="checkbox" checked={allowGroupLaunch} disabled={busy} onChange={e => setAllowGroupLaunch(e.target.checked)} />允许所有已加入机器人的群成员 @会议助手 开会</label>
    <p className="text-xs text-gray-500">其他成员发起时仍使用本机授权账号创建会议，并关联发起群。授权本人需入会，云录音还需本人为主持人；其他成员不能使用绑定会议指令。关闭后只接受授权本人发起。</p>
    <p className="text-xs text-gray-500">请提前配置已下载的转写模型和纪要模型。云录音仅适用于开启自动录音后由助手新建的会议。</p>
    </>}
    <div className="flex gap-2 flex-wrap">
      <Button disabled={busy || !initialized.current || !appId.trim()} onClick={() => void perform(async () => {
        await invoke('lark_app_save', { appId, secret, enabled, autoSend, autoRecord, allowGroupLaunch, recordingSource, manualMode });
        setSecret(''); setEditingSecret(false);
      })}>保存并连接</Button>
      {!manualMode && <Button variant="outline" disabled={busy || !hasSavedSecret || !matchesSavedApp} onClick={() => void perform(async () => {
        const url = await invoke<string>('lark_app_authorize', { includeMeetingControl: true, includeCloudRecording: recordingSource === 'cloud' });
        await invoke('open_external_url', { url });
      })}>授权会议助手</Button>}
      <Button variant="ghost" disabled={busy} onClick={() => void perform(refresh)}>刷新状态</Button>
    </div>
    {!manualMode && <p className="text-xs text-gray-500">本机录音只申请会议识别和控制权限；选择云录音时才申请录制及下载权限。更换来源或账号后请重新授权。</p>}
    <div className="rounded-md bg-gray-50 p-3 text-sm space-y-2">
      <p>连接：{status?.connection || (error ? '配置读取失败' : '正在读取…')}</p>
      <p>已保存模式：{!status?.app_id ? '尚未配置' : status.manual_mode ? '本地录音 · 手动选群' : '群指令开会 · 自动关联'}</p>
      {!status?.manual_mode && <p>Lark 账号：{status?.authorization_status || (status?.authorized ? '已登录' : '未登录')}</p>}
      <p>自动录音：{status?.auto_record ? (status.recording_source === 'cloud' ? '已开启，适用于助手新建的会议' : '已开启，适用于助手新建或本人绑定的会议') : '未开启'}</p>
      {!!status?.app_id && <p>已保存录音来源：{status?.recording_source === 'local' ? '本机录音' : '下载 Lark 云录音'}</p>}
      <p>开会指令：{status?.manual_mode ? '已停用' : status?.allow_group_launch ? '所有已加入机器人的群成员均可发起' : '仅授权本人可发起'}</p>
      {status?.automation && <p>{status.automation}</p>}
      {status?.recording && <p>最近录音：{status.recording.topic || status.recording.remote_id || '未匹配'} — {status.recording.detail}</p>}
    </div>
    {!manualMode && <details className="text-sm space-y-2">
      <summary className="cursor-pointer font-medium">首次配置与使用步骤</summary>
      <ol className="list-decimal pl-5 space-y-2 mt-2">
        <li>后台开通机器人发消息、群内 @机器人消息、用户身份会议信息，以及应用身份“更新会议预约信息”“获取会议预约信息”权限，发布应用并把机器人添加到目标群。</li>
        <li>同时开通用户身份“更新会议信息”“更新会议录制信息”“获取会议录制信息”“获取会议信息”“下载妙记的音视频文件”，完成发布或生效步骤。</li>
        <li>在“安全设置 → 重定向 URL”添加 <code>http://127.0.0.1:18576/lark/callback</code>。</li>
        <li>保存连接后，在后台“事件与回调”保存长连接模式，添加“接收消息”事件。</li>
        <li>点击“授权会议助手”，在 Lark 页面完成一次授权。当前会议查询接口若未对租户开放，会显示错误码，并阻止自动匹配。</li>
        <li>由此处授权的用户在目标群实际 @机器人，发送 <code>开会</code> 或 <code>开会 研发周会</code>。等待机器人返回链接；无需填写会议号。Meetily 必须保持运行，只有授权本人能发起。</li>
        <li>选择来源并保存，加入对应 Lark 会议。云录音需本人为主持人，整场会议结束后自动下载并转写；本机录音在本人离会后停止。后台处理完成后到会议详情审核纪要并确认发送。</li>
      </ol>
      <p className="mt-2">支持 @会议助手 结束会议，或 结束会议:123456789；仅处理本群机器人会议，限该场发起人或授权账号操作。授权账号须在会中且为当前主持人。</p>
      <p className="mt-2">会议号绑定六小时内有效，只用于本场会议。下次使用相同会议号时请重新绑定。多个群绑定同一场会议时暂停自动录音。</p>
    </details>}
    {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
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

