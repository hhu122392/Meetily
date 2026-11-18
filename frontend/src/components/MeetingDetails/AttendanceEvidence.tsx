'use client';
import type {AttendanceMetadata} from '@/hooks/meeting-details/useMeetingSummarySettings';
export function AttendanceEvidence({attendance}:{attendance?:AttendanceMetadata|null}){
  if(!attendance)return null;
  return <div className="space-y-1 text-xs text-gray-500">
    <p>出席：{attendance.participantsManual?'保留手动填写':'按原文识别'}；缺席：{attendance.absentManual?'保留手动填写':'按原文识别'}。未明确提到的留空，不根据是否发言判断。</p>
    {!!attendance.conflicts.length&&<p className="text-amber-700">原文名单有冲突，未自动填入：{attendance.conflicts.join('、')}。请核对后手动填写。</p>}
    <details><summary className="cursor-pointer">查看人员识别依据（{attendance.evidence.length} 条）</summary><div className="mt-2 max-h-48 space-y-2 overflow-y-auto">{attendance.evidence.length?attendance.evidence.map((e,index)=><p key={`${e.name}:${e.status}:${index}`} className="rounded border p-2"><strong>{e.name} · {e.status==='present'?'出席':'缺席'}</strong><span className="block whitespace-pre-wrap">原文：{e.quote}</span></p>):<p>原文中没有识别到明确的出席或缺席名单，请手动核对。</p>}</div></details>
  </div>;
}
