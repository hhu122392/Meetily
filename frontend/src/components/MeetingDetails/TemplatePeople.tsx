'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import type { MeetingSummarySettings } from '@/hooks/meeting-details/useMeetingSummarySettings';
import {AttendanceEvidence} from './AttendanceEvidence';
const names=(value:string)=>[...new Set(value.split(/[\n\r,，、;；]/).map(s=>s.trim()).filter(Boolean))];

export function TemplatePeople({ meetingId, settings, disabled, save, update, refresh }: {
  meetingId: string; settings: MeetingSummarySettings; disabled: boolean;
  save: () => Promise<MeetingSummarySettings>; update: (patch: Partial<MeetingSummarySettings>) => void;
  refresh?:()=>Promise<void>;
}) {
  const [people, setPeople]=useState<{participants:string;host:string}|null>(null);
  const [message,setMessage]=useState('');
  const [busy,setBusy]=useState(false);
  const [roster,setRoster]=useState('');
  const identity=`${meetingId}:${settings.templateId}`;
  const current=useRef(identity); current.current=identity;
  useEffect(()=>{
    let active=true;setPeople(null);setMessage('');
    void invoke<{participants:string;host:string}>('meeting_template_people_read',{templateId:settings.templateId})
      .then(v=>{if(active){setPeople(v);setRoster(v?.participants||'');}}).catch(()=>{if(active)setMessage('常用名单读取失败，可继续手动填写。');});
    return ()=>{active=false;};
  },[identity,settings.templateId]);
  const mark=(name:string,status:string)=>{
    const present=names(settings.participants).filter(n=>n!==name),absent=names(settings.absent).filter(n=>n!==name);
    if(status==='present')present.push(name);if(status==='absent')absent.push(name);
    const patch:Partial<MeetingSummarySettings>={};
    if(present.join('、')!==settings.participants)patch.participants=present.join('、');
    if(absent.join('、')!==settings.absent)patch.absent=absent.join('、');
    update(patch);setMessage('已按你的选择填写本场名单，后续自动提取不会覆盖手动修改。');
  };
  return <div className="my-2 space-y-2">
    <AttendanceEvidence attendance={settings.attendance}/>
    {refresh&&<Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{setBusy(true);setMessage('');const expected=identity;try{await save();if(current.current!==expected)return;await refresh();if(current.current===expected)setMessage('已重新核对原文，手动修改和手动清空保持不变。');}catch(e){if(current.current===expected)setMessage(String(e));}finally{setBusy(false);}}}>重新识别名单</Button>}
    <details className="rounded border p-2"><summary className="cursor-pointer text-sm">常用名单（候选人员，需逐人确认）</summary>
    <p className="my-2 text-xs text-gray-500">按公司和模板保存，用于姓名校对和快速选择，不代表实际出席。未提到、未确认的人保持待确认，不会自动列为缺席。</p>
    <div className="max-h-48 space-y-1 overflow-y-auto">{names(people?.participants||'').map(name=><label key={name} className="flex items-center justify-between gap-2 text-sm"><span>{name}</span><select aria-label={`常用人员 ${name}`} className="rounded border bg-background p-1" disabled={disabled||busy} value={names(settings.participants).includes(name)?'present':names(settings.absent).includes(name)?'absent':''} onChange={e=>mark(name,e.target.value)}><option value="">待确认（留空）</option><option value="present">出席</option><option value="absent">缺席</option></select></label>)}</div>
    <label className="mt-3 block text-xs">编辑常用名单<textarea aria-label="编辑常用名单" className="mt-1 w-full rounded border bg-background p-2" rows={3} disabled={disabled||busy} value={roster} onChange={e=>setRoster(e.target.value)} placeholder="每行一个姓名，也可用顿号分隔"/></label>
    <Button size="sm" variant="outline" disabled={disabled||busy} onClick={async()=>{setBusy(true);setMessage('');const expected=identity;try{await invoke('meeting_template_people_update',{templateId:settings.templateId,people:{participants:roster,host:people?.host||''}});if(current.current===expected){setPeople({participants:names(roster).join('、'),host:people?.host||''});setMessage('已保存常用名单，本场出席和缺席没有改变。');}}catch(e){if(current.current===expected)setMessage(String(e));}finally{setBusy(false);}}}>保存常用名单</Button>
    <Button size="sm" variant="ghost" disabled={disabled||busy} onClick={async()=>{
      if(busy)return;setBusy(true);setMessage('');const expected=identity;
      try {
        await save();
        if(current.current!==expected)return;
        await invoke('meeting_template_people_save',{meetingId});
        const value=await invoke<{participants:string;host:string}>('meeting_template_people_read',{templateId:settings.templateId});
        if(current.current===expected){setPeople(value);setRoster(value.participants);setMessage('已保存为当前模板的常用名单。');}
      }catch{if(current.current===expected)setMessage('未能保存常用名单，请检查出席和缺席名单是否重复。');}
      finally{setBusy(false);}
    }}>将本场名单存为常用名单</Button>
    </details>
    <p role="status" className="text-xs text-gray-500">{message||'自动提取仅作待确认结果，发送前需核对；本场调整不会自动覆盖常用名单。'}</p>
  </div>;
}
