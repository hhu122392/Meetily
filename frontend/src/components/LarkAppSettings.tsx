'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

type AppStatus = {
  app_id: string; has_secret: boolean; enabled: boolean; auto_send: boolean;
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
  const [autoSend, setAutoSend] = useState(false);
  const [autoRecord, setAutoRecord] = useState(false);
  const [recordingSource, setRecordingSource] = useState<'cloud' | 'local'>('cloud');
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
    setStatus(value);
    if (!initialized.current) {
      setAppId(value.app_id); setEnabled(value.enabled); setAutoSend(value.auto_send);
      setAutoRecord(value.auto_record ?? false);
      setRecordingSource(value.recording_source ?? 'cloud');
      setAllowGroupLaunch(value.allow_group_launch ?? false);
      initialized.current = true;
    }
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
      <h2 className="text-lg font-semibold">Lark 企业应用 · 自动匹配与发群</h2>
      <p className="text-sm text-gray-600 mt-2">在目标群 @会议助手 发送“开会”，机器人返回链接并关联本群。开启自动录音后，按下方来源获取录音，再后台转写和生成纪要。</p>
    </div>
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
      <div role="status" className={`rounded-md border px-3 py-2 text-sm ${loginAvailable ? 'border-green-200 bg-green-50 text-green-800' : 'border-gray-200 bg-gray-50 text-gray-700'}`}>
        <p className="font-medium">登录状态：{!status ? (error ? '配置读取失败' : '正在读取…') : !matchesSavedApp ? 'App ID 已修改，请先保存并重新授权' : loginAvailable ? '已登录 · 授权可用' : '未登录或授权已失效'}</p>
        {matchesSavedApp && <p className="mt-1 text-xs">{status?.authorization_status || (loginAvailable ? 'Lark 账号已登录' : '请点击下方“授权会议助手”登录 Lark。')}</p>}
      </div>
    </div>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={enabled} disabled={busy} onChange={e => setEnabled(e.target.checked)} />启用企业应用连接</label>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={autoSend} disabled={busy} onChange={e => setAutoSend(e.target.checked)} />新录音生成纪要后准备发往唯一绑定群（需审核确认）</label>
    <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={autoRecord} disabled={busy} onChange={e => setAutoRecord(e.target.checked)} />加入会议助手新建的会议时自动录音</label>
    <div className="space-y-2">
      <Label htmlFor="lark-recording-source">录音来源</Label>
      <select id="lark-recording-source" className="w-full rounded-md border bg-transparent p-2 text-sm" value={recordingSource} disabled={busy} onChange={e => setRecordingSource(e.target.value as 'cloud' | 'local')}>
        <option value="cloud">下载 Lark 云录音（默认）</option>
        <option value="local">本机录音（麦克风和系统声音）</option>
      </select>
      <p className="text-xs text-gray-500">保存后对下一场会议生效；已有任务继续使用原来源。</p>
      {recordingSource === 'cloud' ? <p className="text-xs text-gray-500">授权本人入会且为主持人时，自动请求 Lark 开始录制。整场会议结束后，等待云文件就绪，下载到当前公司的录音目录，再后台转写和生成纪要；不会录入电脑的其他声音。下载不使用 Lark 语音转文字额度。请保持 Meetily 运行；重启后会继续等待或下载。暂停任务不会停止 Lark 云录制。</p>
        : <p className="text-xs text-gray-500">使用已保存的麦克风和系统音频设备。本人入会后开始录音，连续两次确认离会后停止；手动停止后本场不再重启。请保持 Meetily 运行。</p>}
    </div>
    <label className="flex items-center gap-2 text-sm"><input id="lark-allow-group-launch" type="checkbox" checked={allowGroupLaunch} disabled={busy} onChange={e => setAllowGroupLaunch(e.target.checked)} />允许所有已加入机器人的群成员 @会议助手 开会</label>
    <p className="text-xs text-gray-500">其他成员发起时仍使用本机授权账号创建会议，并关联发起群。授权本人需入会，云录音还需本人为主持人；其他成员不能使用绑定会议指令。关闭后只接受授权本人发起。</p>
    <p className="text-xs text-gray-500">仅处理开启自动录音后新建的机器人会议。请提前配置已下载的转写模型和纪要模型。云录制、存储和下载是否可用，仍以当前 Lark 套餐及管理员设置为准；失败可在后台队列查看原因并重试。</p>
    <div className="flex gap-2 flex-wrap">
      <Button disabled={busy || !initialized.current || !appId.trim()} onClick={() => void perform(async () => {
        await invoke('lark_app_save', { appId, secret, enabled, autoSend, autoRecord, allowGroupLaunch, recordingSource });
        setSecret(''); setEditingSecret(false);
      })}>保存并连接</Button>
      <Button variant="outline" disabled={busy || !hasSavedSecret || !matchesSavedApp} onClick={() => void perform(async () => {
        const url = await invoke<string>('lark_app_authorize', { includeMeetingControl: true, includeCloudRecording: true });
        await invoke('open_external_url', { url });
      })}>授权会议助手</Button>
      <Button variant="ghost" disabled={busy} onClick={() => void perform(refresh)}>刷新状态</Button>
    </div>
    <p className="text-xs text-gray-500">一次授权即可申请会议识别、会议控制和云录音下载权限。旧版用户请点击一次补充授权；更换账号或重新登录也使用此按钮。首次使用前，请按下方步骤在 Lark 后台开通所需权限。</p>
    <div className="rounded-md bg-gray-50 p-3 text-sm space-y-2">
      <p>连接：{status?.connection || (error ? '配置读取失败' : '正在读取…')}</p>
      <p>Lark 账号：{status?.authorization_status || (status?.authorized ? '已登录' : '未登录')}</p>
      <p>自动录音：{status?.auto_record ? '已开启，仅处理启用后新建的机器人会议' : '未开启'}</p>
      <p>已保存录音来源：{status?.recording_source === 'local' ? '本机录音' : '下载 Lark 云录音'}</p>
      <p>开会指令：{status?.allow_group_launch ? '所有已加入机器人的群成员均可发起' : '仅授权本人可发起'}</p>
      {status?.automation && <p>{status.automation}</p>}
      {status?.recording && <p>最近录音：{status.recording.topic || status.recording.remote_id || '未匹配'} — {status.recording.detail}</p>}
    </div>
    <details className="text-sm space-y-2">
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
      <p className="mt-2">会议链接六小时内有效，每次开会请发送新指令。自动关联仅适用于机器人创建的会议；原生 Lark 会议仍可用 <code>@会议助手 绑定会议 123456789</code> 手动指定群。多群冲突、跨会议录音或跟踪中断时暂停发群。</p>
    </details>
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

