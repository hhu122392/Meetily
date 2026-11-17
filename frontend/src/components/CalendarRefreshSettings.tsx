'use client';
import { useCalendarAutoRefresh } from '@/hooks/useCalendarAutoRefresh';
import { Switch } from '@/components/ui/switch';

export function CalendarRefreshSettings() {
  const { enabled, update, error } = useCalendarAutoRefresh();
  return <div className="rounded-lg border p-3 space-y-2">
    <div className="flex items-center justify-between gap-4">
      <label htmlFor="calendar-auto-refresh" className="text-sm font-medium">自动刷新日历</label>
      <Switch id="calendar-auto-refresh" checked={enabled} onCheckedChange={update} />
    </div>
    <p className="text-xs text-slate-500">每次启动软件同步一次。开启后，停留首页期间每 5 分钟同步；失败后暂停，手动刷新成功后恢复。关闭后仍可手动刷新。</p>
    {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
  </div>;
}
