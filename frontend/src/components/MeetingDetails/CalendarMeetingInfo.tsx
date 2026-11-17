'use client';
import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { calendarTime } from '@/components/LarkUpcomingMeetings';
import type { MeetingSummarySettings } from '@/hooks/meeting-details/useMeetingSummarySettings';
type Snapshot = { event: { title: string; start: number; end: number }; invitees: string[]; invitationNote: string };
export function CalendarMeetingInfo({ meetingId, disabled, update }: {
  meetingId: string; disabled: boolean; update: (patch: Partial<MeetingSummarySettings>) => void;
}) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let active = true; setSnapshot(null); setError('');
    void invoke<Snapshot | null>('lark_calendar_meeting', { meetingId }).then(v => { if (active) setSnapshot(v); }).catch(() => { if (active) setError('关联日程读取失败，可手动核对人员。'); });
    return () => { active = false; };
  }, [meetingId]);
  if (error) return <p role="alert" className="text-xs text-amber-700">{error}</p>;
  if (!snapshot) return null;
  return <div className="my-2 rounded border bg-blue-50 p-3 text-sm">
    <p>录音关联日程：{snapshot.event.title}</p>
    <p>计划时间（北京时间）：{calendarTime(snapshot.event.start)} — {calendarTime(snapshot.event.end)}</p>
    <p className="whitespace-pre-wrap">邀请人员：{snapshot.invitees.join('、') || '未取得名单'}</p>
    <p className="text-xs text-slate-600">{snapshot.invitationNote} 邀请不代表实际出席；如需带入，请再核对缺席人员。</p>
    {!!snapshot.invitees.length && <Button size="sm" variant="outline" className="mt-2" disabled={disabled} onClick={() => update({ participants: snapshot.invitees.join('、'), absent: '' })}>带入邀请名单并核对</Button>}
  </div>;
}
