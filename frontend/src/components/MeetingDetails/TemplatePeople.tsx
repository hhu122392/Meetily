'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import type { MeetingSummarySettings } from '@/hooks/meeting-details/useMeetingSummarySettings';

export function TemplatePeople({ meetingId, settings, disabled, save, update }: {
  meetingId: string; settings: MeetingSummarySettings; disabled: boolean;
  save: () => Promise<MeetingSummarySettings>; update: (patch: Partial<MeetingSummarySettings>) => void;
}) {
  const [people, setPeople]=useState<{participants:string;host:string}|null>(null);
  const [message,setMessage]=useState('');
  const [busy,setBusy]=useState(false);
  const identity=`${meetingId}:${settings.templateId}`;
  const current=useRef(identity); current.current=identity;
  useEffect(()=>{
    let active=true;setPeople(null);setMessage('');
    if (!disabled) void invoke<{participants:string;host:string}>('meeting_template_people_read',{templateId:settings.templateId})
      .then(v=>{if(active)setPeople(v);}).catch(()=>{if(active)setMessage('常用名单读取失败，可继续手动填写。');});
    return ()=>{active=false;};
  },[identity,disabled,settings.templateId]);
  return <div className="my-2 flex flex-wrap items-center gap-2">
    <Button size="sm" variant="outline" disabled={disabled||busy||!people?.participants} onClick={()=>{
      if(!people)return;
      update({participants:people.participants,absent:'',host:people.host});setMessage('已带入常用人员，请将本次未参会者移至缺席名单。');
    }}>带入常用参会人</Button>
    <Button size="sm" variant="ghost" disabled={disabled||busy} onClick={async()=>{
      if(busy)return;setBusy(true);setMessage('');const expected=identity;
      try {
        await save();
        if(current.current!==expected)return;
        await invoke('meeting_template_people_save',{meetingId});
        const value=await invoke<{participants:string;host:string}>('meeting_template_people_read',{templateId:settings.templateId});
        if(current.current===expected){setPeople(value);setMessage('已保存为当前模板的常用名单。');}
      }catch{if(current.current===expected)setMessage('未能保存常用名单，请检查出席和缺席名单是否重复。');}
      finally{setBusy(false);}
    }}>保存为常用名单</Button>
    <span className="text-xs text-gray-500">{message||'常用名单按公司和模板保存，本场调整不会自动覆盖常用名单。'}</span>
  </div>;
}
