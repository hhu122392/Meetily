'use client';
import { useEffect,useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
type Stats={transcriptionSeconds:number|null;audioSeconds:number|null;summarySeconds:number|null;model:string|null;provider:string|null};
const time=(seconds:number|null)=>seconds===null?'暂无记录':seconds<60?`${seconds.toFixed(1)} 秒`:`${(seconds/60).toFixed(1)} 分钟`;
export function ProcessingStats({meetingId}:{meetingId:string}) {
  const [stats,setStats]=useState<Stats|null>(null);
  useEffect(()=>{let active=true;setStats(null);const refresh=()=>void invoke<Stats>('meeting_processing_stats',{meetingId}).then(v=>{if(active)setStats(v);}).catch(()=>{});
    refresh();const timer=setInterval(refresh,10000);return()=>{active=false;clearInterval(timer);};
  },[meetingId]);
  if(!stats)return null;
  return <details className="text-xs text-gray-600"><summary className="cursor-pointer">处理耗时：转写 {time(stats.transcriptionSeconds)} · 纪要 {time(stats.summarySeconds)}</summary>
    <p className="mt-1">录音时长：{time(stats.audioSeconds)} · 转写模型：{stats.model||'未记录'} {stats.provider||''}</p>
    <p>转写计时包含本次加载、解码和转写，不含排队；暂停续传只记录最后一次执行。实时转写能力需用完整录音及目标电脑另行实测。</p>
  </details>;
}
