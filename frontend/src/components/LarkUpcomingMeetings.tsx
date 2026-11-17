'use client';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import type { CalendarRecordingSelection } from '@/services/recordingService';
import { CALENDAR_REFRESH_INTERVAL, useCalendarAutoRefresh } from '@/hooks/useCalendarAutoRefresh';

// Kept for this application session so returning to the homepage does not
// repeat startup requests or resume a failed automatic sync.
type SyncSession = { started: boolean; error: string; pending?: Promise<CalendarView> };
const syncSessions = new Map<string, SyncSession>();

export type CalendarMeeting = CalendarRecordingSelection & {
  eventId: string; seriesId: string; start: number; end: number; allDay: boolean;
  meetingUrl?: string | null; eventUrl?: string | null; recordedMeetingId?: string | null;
};
type CalendarView = { configured: boolean; connected: boolean; accountKey: string; authRevision: string; syncedAt: number | null; events: CalendarMeeting[] };
export const calendarTime = (seconds: number) => new Intl.DateTimeFormat('zh-CN', {
  timeZone: 'Asia/Shanghai', year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23',
}).format(new Date(seconds * 1000));

export function LarkUpcomingMeetings({ disabled, recordingDisabled = false, onStart, onSettings, onOpenMeeting }: {
  disabled: boolean; recordingDisabled?: boolean; onStart: (selection: CalendarRecordingSelection) => Promise<void>;
  onSettings: () => void; onOpenMeeting: (id: string) => void;
}) {
  const [view, setView] = useState<CalendarView | null>(null);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [waiting, setWaiting] = useState(false);
  const { enabled: autoRefresh } = useCalendarAutoRefresh();
  const session = useRef<SyncSession | null>(null);
  const active = useRef(true), lock = useRef(false), authStarted = useRef(0);
  const awaitingRevision = useRef<string | null>(null);
  const run = useCallback(async (action: () => Promise<void>) => {
    if (lock.current) return;
    lock.current = true; setBusy(true); setError('');
    try { await action(); }
    catch (e) { if (active.current) setError(typeof e === 'string' ? e : e instanceof Error ? e.message : '日程操作失败，请重试。'); }
    finally { lock.current = false; if (active.current) setBusy(false); }
  }, []);
  const refresh = useCallback((reason: 'manual' | 'startup' | 'auto' | 'authorization' = 'manual') => {
    if (reason === 'auto' && session.current?.error) return Promise.resolve();
    return run(async () => {
      const cached = await invoke<CalendarView>('lark_calendar_status');
      if (!active.current) return;
      setView(cached);
      if (awaitingRevision.current !== null && cached.authRevision === awaitingRevision.current) return;
      if (cached.connected) {
        awaitingRevision.current = null;
        setWaiting(false);
        const key = JSON.stringify([cached.accountKey, cached.authRevision]);
        let current = syncSessions.get(key);
        if (!current) { current = { started: false, error: '' }; syncSessions.set(key, current); }
        session.current = current;
        if (current.pending) {
          const fresh = await current.pending;
          if (active.current) setView(fresh);
          return;
        }
        if ((reason === 'startup' && current.started) || (reason === 'auto' && current.error)) {
          setError(current.error); return;
        }
        current.started = true;
        try {
          current.pending = invoke<CalendarView>('lark_calendar_sync');
          const fresh = await current.pending;
          current.error = '';
          if (active.current) setView(fresh);
        } catch (e) {
          current.error = typeof e === 'string' ? e : e instanceof Error ? e.message : '日程同步失败，请重试。';
          throw current.error;
        } finally { current.pending = undefined; }
      }
    });
  }, [run]);
  useEffect(() => {
    active.current = true;
    void refresh('startup');
    return () => { active.current = false; };
  }, [refresh]);
  useEffect(() => {
    if (!autoRefresh) return;
    const timer = setInterval(() => { void refresh('auto'); }, CALENDAR_REFRESH_INTERVAL);
    return () => clearInterval(timer);
  }, [autoRefresh, refresh]);
  useEffect(() => {
    if (!waiting) return;
    const timer = setInterval(() => {
      if (Date.now() - authStarted.current > 300000) { awaitingRevision.current = null; setWaiting(false); setError('日历连接等待超时，请重试。'); }
      else void refresh('authorization');
    }, 3000);
    return () => clearInterval(timer);
  }, [waiting, refresh]);
  const connect = () => run(async () => {
    const url = await invoke<string>('lark_app_authorize', { calendarOnly: true });
    await invoke('open_external_url', { url });
    if (active.current) { awaitingRevision.current = view?.authRevision || ''; authStarted.current = Date.now(); setWaiting(true); }
  });
  const openLink = (item: CalendarMeeting, join: boolean) => run(async () => {
    const url = await invoke<string>('lark_calendar_link', { selection: { key: item.key, revision: item.revision }, join });
    await invoke('open_external_url', { url });
  });
  return <section className="mx-auto my-4 w-full max-w-3xl rounded-xl border bg-white p-4 shadow-sm" aria-label="Lark 近期会议">
    <div className="flex flex-wrap items-center justify-between gap-2">
      <div><h2 className="font-semibold text-slate-900">近期会议</h2><p className="mt-1 text-xs text-slate-500">Lark 安排日程 · Meetily 本地录音 · 会后手动选群发送</p></div>
      {view?.connected && <Button size="sm" variant="outline" disabled={busy || disabled} onClick={() => void refresh()}>刷新日程</Button>}
    </div>
    {!view && !error && <p className="mt-3 text-sm" role="status">正在读取日历连接…</p>}
    {view && !view.configured && <div className="mt-3 space-y-2"><p className="text-sm">首次使用先导入会议助手凭证，再连接个人日历。</p><Button disabled={busy || disabled} onClick={onSettings}>设置会议助手</Button></div>}
    {view?.configured && !view.connected && <div className="mt-3 space-y-2">
      <p className="text-sm text-slate-600">连接后同步你的主日历，包含周期会议的每一次。仅申请读取日程，启用本地录音及手动发送。</p>
      <Button disabled={busy || disabled || waiting} onClick={() => void connect()}>{waiting ? '等待 Lark 授权…' : '连接我的 Lark 日历'}</Button>
    </div>}
    {view?.connected && <>
      <p className="my-3 text-xs text-slate-500">最近一天至未来 14 天 · 北京时间{view.syncedAt ? ` · 上次同步 ${calendarTime(view.syncedAt)}` : ''}。先加入会议，再点击本地录音。</p>
      <p className="mb-2 text-xs text-slate-500">{autoRefresh ? (session.current?.error ? '自动刷新已暂停，请手动重新同步。' : '自动刷新已开启，每 5 分钟同步一次。') : '自动刷新已关闭，可手动刷新。'} 可在设置中的「Lark 会议助手」调整。</p>
      {!!error && <p className="mb-2 text-xs text-amber-700">以下可能是上次缓存，请刷新成功后再操作。</p>}
      {!view.events.length && !busy && !error && <p className="py-4 text-sm text-slate-500">暂无日程。在 Lark 主日历创建会议或接受邀请后，点击刷新；支持周期会议。</p>}
      <div className="max-h-80 space-y-2 overflow-y-auto">{view.events.map(item => <article key={item.key} className="rounded-lg border border-slate-200 p-3">
        <p className="font-medium break-words">{item.title}{item.seriesId && <span className="ml-2 text-xs font-normal text-slate-500">周期会议</span>}</p>
        <p className="mt-1 text-sm text-slate-600">{calendarTime(item.start)} — {calendarTime(item.end)}{item.allDay ? '（全天日程）' : ''}</p>
        <div className="mt-2 flex flex-wrap gap-2">
          {item.meetingUrl && <Button size="sm" variant="outline" disabled={busy || disabled || waiting || !!error} onClick={() => void openLink(item, true)}>加入会议</Button>}
          <Button size="sm" disabled={busy || disabled || recordingDisabled || waiting || !!error} onClick={() => void run(async () => { await onStart(item); })}>{item.recordedMeetingId ? '再次本地录音' : '开始本地录音'}</Button>
          {item.recordedMeetingId && <Button size="sm" variant="ghost" disabled={busy || disabled} onClick={() => onOpenMeeting(item.recordedMeetingId!)}>查看已有录音</Button>}
          {item.eventUrl && <Button size="sm" variant="ghost" disabled={busy || disabled || !!error} onClick={() => void openLink(item, false)}>查看日程</Button>}
        </div>
      </article>)}</div>
    </>}
    {busy && <p role="status" className="mt-2 text-xs text-slate-500">正在处理，请稍候…</p>}
    {error && <div className="mt-2"><p role="alert" className="text-sm text-red-700">{error}</p><Button size="sm" variant="ghost" disabled={busy || disabled} onClick={() => void refresh()}>重新同步</Button>{view?.configured && <Button size="sm" variant="ghost" disabled={busy || disabled || waiting} onClick={() => void connect()}>重新连接日历</Button>}</div>}
  </section>;
}
