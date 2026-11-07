'use client';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';


type Status = { enabled: boolean; linked: boolean; remote_id?: string; detail?: string; blocked?: boolean; status?: string; chat_id?: string; chat_name?: string; delivery_detail?: string; attachment_status?: string; attachment_detail?: string; attachment_last_sent_at?: number };
export function LarkDeliveryStatus({ meetingId }: { meetingId: string }) {
  const [status, setStatus] = useState<Status | null>(null);


  useEffect(() => {
    let active = true;
    setStatus(null);
    const refresh = () => void invoke<Status>('lark_app_meeting_status', { meetingId }).then(v => { if (active) setStatus(v); }).catch(() => {});
    refresh(); const interval = setInterval(refresh, 5000);
    return () => { active = false; clearInterval(interval); };
  }, [meetingId]);
  if (!status?.enabled && !status?.linked) return null;
  return <div className="shrink-0 border-b bg-slate-50 px-4 py-2 text-xs flex flex-wrap items-center gap-2">
    <span>Lark：{status?.delivery_detail || status?.detail || '本次录音未关联会议。'}</span>
    {status?.attachment_status && <span className={['failed', 'unknown'].includes(status.attachment_status) ? 'text-red-700' : ''}>Markdown 附件：{status.attachment_status === 'sent' ? '发送成功' : status.attachment_detail || '尚未发送'}</span>}
    {status?.remote_id && <span>会议 ID：{status.remote_id}</span>}
    {status?.chat_id && <span title={status.chat_id}>目标群：{status.chat_name || '打开发送预览后获取群名'}</span>}
    {status?.linked && !status.blocked && (!status.status || status.status === 'awaiting_confirmation') && <span>生成后请在纪要区域编辑、保存，再点击“发送到 Lark”。</span>}

  </div>;
}


