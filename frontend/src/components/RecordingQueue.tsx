'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';

type Job = { meeting_id: string; title: string; status: string; detail: string; progress?: number | null };
const labels: Record<string, string> = {
  capturing: '录音中', queued: '等待转写', transcribing: '后台转写中',
  importing: '正在导入音频',
  summary_pending: '等待总结', summarizing: '正在总结', completed: '处理完成', failed: '需要处理',
  pausing: '正在暂停', paused: '已暂停',
  cancelling: '正在取消', cancelled: '已取消',
  cloud_starting: '正在启动云录音', waiting_cloud: '等待云录音', downloading_cloud: '正在检查或下载云录音',
};

export function RecordingQueue() {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [error, setError] = useState('');
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [changing, setChanging] = useState<string | null>(null);
  const changingRef = useRef(false);
  const { refetchMeetings } = useSidebar();
  useEffect(() => {
    let disposed = false;
    const stops: (() => void)[] = [];
    const refresh = async () => {
      try {
        const next = await invoke<Job[]>('get_recording_jobs');
        if (!disposed) {
          setJobs(next); setError('');
          setProgress(prev => Object.fromEntries(Object.entries(prev).filter(([id]) =>
            next.some(job => job.meeting_id === id && job.status === 'transcribing'))));
        }
      }
      catch { if (!disposed) setError('暂时无法读取后台队列'); }
    };
    void refresh();
    const timer = setInterval(refresh, 3000);
    void listen('recording-queue-changed', () => { void refresh(); void refetchMeetings(); })
      .then(fn => { if (disposed) fn(); else stops.push(fn); });
    void listen<{ meeting_id: string; progress_percentage: number }>('retranscription-progress', ({ payload }) => {
      if (!disposed) setProgress(prev => ({ ...prev, [payload.meeting_id]: payload.progress_percentage }));
    }).then(fn => { if (disposed) fn(); else stops.push(fn); });
    return () => { disposed = true; clearInterval(timer); stops.forEach(fn => fn()); };
  }, [refetchMeetings]);
  if (!jobs.length && !error) return null;
  const updateJob = async (command: string, id: string) => {
    if (changingRef.current) return;
    changingRef.current = true; setChanging(id); setError('');
    try {
      await invoke(command, { meetingId: id });
      setJobs(await invoke<Job[]>('get_recording_jobs'));
    } catch (e) { setError(String(e)); }
    finally { changingRef.current = false; setChanging(null); }
  };
  const pending = jobs.filter(j => !['completed', 'cancelled'].includes(j.status)).length;
  return <details className="fixed bottom-4 right-4 z-40 w-80 max-w-[90vw] rounded-lg border bg-white p-3 shadow-md dark:bg-gray-900">
    <summary className="cursor-pointer text-sm font-medium">后台会议任务 · {pending ? `${pending} 项未完成` : '已全部处理'}</summary>
    <p className="mt-2 text-xs text-gray-500">暂停会在当前分段或准备步骤结束后生效，并保留已完成分段。显示“已暂停”后可切换公司；重启后仍暂停，点击继续才恢复。</p>
    <p className="mt-1 text-xs text-gray-500">取消会停止后续处理，保留已有录音和结果；不会停止 Lark 云录制。等待显示“已取消”后可切换公司。</p>
    {error && <p role="alert" className="text-xs text-red-600">{error}</p>}
    <ul className="mt-2 max-h-64 space-y-3 overflow-y-auto">{jobs.map(job => <li key={job.meeting_id} className="border-t pt-2 text-sm">
      <a href={`/meeting-details?id=${encodeURIComponent(job.meeting_id)}`} className="font-medium underline">{job.title}</a>
      <p>{labels[job.status] || job.status}{['transcribing', 'pausing', 'paused'].includes(job.status) && (job.progress ?? progress[job.meeting_id]) !== undefined && (job.progress ?? progress[job.meeting_id]) !== null ? ` · ${job.progress ?? progress[job.meeting_id]}%` : ''}</p>
      <p className="text-xs text-gray-500">{job.detail}</p>
      {['queued', 'summary_pending', 'transcribing', 'waiting_cloud', 'downloading_cloud'].includes(job.status) && <button className="mt-1 text-blue-600 underline" disabled={changing !== null} onClick={() => updateJob('pause_recording_job', job.meeting_id)}>暂停任务</button>}
      {job.status === 'paused' && <button className="mt-1 text-blue-600 underline" disabled={changing !== null} onClick={() => updateJob('resume_recording_job', job.meeting_id)}>继续任务</button>}
      {job.status === 'pausing' && <p className="text-xs text-amber-700">正在保存当前进度，请等待显示“已暂停”后再切换。</p>}
      {['importing', 'queued', 'summary_pending', 'transcribing', 'summarizing', 'cloud_starting', 'waiting_cloud', 'downloading_cloud', 'pausing', 'paused', 'failed'].includes(job.status) && <button className="mt-1 ml-3 text-red-600 underline" disabled={changing !== null} onClick={() => updateJob('cancel_recording_job', job.meeting_id)}>取消任务</button>}
      {job.status === 'cancelling' && <p className="text-xs text-amber-700">等待当前处理步骤退出，已有录音和结果会保留。</p>}
      {job.status === 'summarizing' && <p className="text-xs text-gray-500">纪要生成已开始，请等待完成后切换公司。</p>}
      {job.status === 'failed' && <button className="mt-1 ml-3 text-blue-600 underline" disabled={changing !== null} onClick={() => updateJob('retry_recording_job', job.meeting_id)}>重试处理</button>}
    </li>)}</ul>
  </details>;
}
