'use client';
import {useRef,useState} from 'react';
import {SendHorizontal} from 'lucide-react';
import {invoke} from '@tauri-apps/api/core';
import {Button} from '@/components/ui/button';
import {Dialog,DialogContent,DialogDescription,DialogHeader,DialogTitle} from '@/components/ui/dialog';
import {LarkCardPreview,type LarkPreviewCard} from './MinutesCardPreview';
import {AttendanceEvidence} from './AttendanceEvidence';
import type {AttendanceMetadata} from '@/hooks/meeting-details/useMeetingSummarySettings';
type Review={chatId:string;title:string;meetingTime:string;participants:string;absent:string;host:string;summary:string;project:string;documentIds?:string[];messageTemplate?:string;includeMarkdown?:boolean;attendanceConfirmed?:boolean;participantsEdited?:boolean;absentEdited?:boolean};
type SavedDocument={id:string;title:string;project:string;documentUrl:string;available:boolean};
type Delivery={status:string;detail:string};
type Preview={id:string;chatName:string;card:LarkPreviewCard;documents?:{id:string;documentUrl:string}[];includeMarkdown?:boolean;markdown?:{name:string;size:number}};
const initial:Review={chatId:'',title:'',meetingTime:'',participants:'',absent:'',host:'',summary:'',project:''};
const simpleTemplate='{会议名称}\n会议时间：{会议时间}\n\n{概要}';
const customTemplate='{会议名称}\n会议时间：{会议时间}\n出席人员：{出席人员}\n缺席人员：{缺席人员}\n主持人：{主持人}\n\n{概要}\n\n云文档：{云文档链接}';
export function ManualLarkSend({meetingId,saveEdits,disabled,compact=false,documentId}:{meetingId:string;saveEdits:()=>Promise<void>;disabled?:boolean;compact?:boolean;documentId?:string}){
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState('');
  const [groups,setGroups]=useState<{id:string;name:string}[]>([]),[review,setReview]=useState<Review>(initial);
  const [preview,setPreview]=useState<Preview|null>(null),[last,setLast]=useState<Delivery|null>(null);
  const [sent,setSent]=useState(false),[ack,setAck]=useState(false);const lock=useRef(false);
  const [templates,setTemplates]=useState<{name:string;body:string}[]>([]),[template,setTemplate]=useState('standard'),[templateName,setTemplateName]=useState('');
  const [documents,setDocuments]=useState<SavedDocument[]>([]),[includeCloud,setIncludeCloud]=useState(false),[markdownName,setMarkdownName]=useState('');
  const selectedDocuments=review.documentIds||[];
  const [attendance,setAttendance]=useState<AttendanceMetadata|null>(null);
  const label=documentId?'发送到群':'选群发送纪要';
  async function run(action:()=>Promise<void>){if(lock.current)return;lock.current=true;setBusy(true);setError('');try{await action();}catch(e){setError(String(e));}finally{lock.current=false;setBusy(false);}}
  function change(patch:Partial<Review>){setReview(v=>({...v,...patch,...('participants' in patch?{participantsEdited:true,attendanceConfirmed:false}:{}),...('absent' in patch?{absentEdited:true,attendanceConfirmed:false}:{})}));setPreview(null);setSent(false);}
  async function prepare(){if(lock.current)return;setOpen(true);setPreview(null);setSent(false);setAck(false);setLast(null);setGroups([]);setReview(initial);setAttendance(null);setTemplate('standard');setTemplateName('');setIncludeCloud(false);setDocuments([]);setMarkdownName('');await run(async()=>{
    await saveEdits();if(!documentId)await invoke('summary_export_markdown',{meetingId});
    const [context,items,savedTemplates]=await Promise.all([invoke<Review&{attendance?:AttendanceMetadata;lastDelivery?:Delivery;documents?:SavedDocument[];markdown?:{name:string}}>('lark_manual_context',{meetingId}),invoke<{id:string;name:string}[]>('lark_manual_groups'),invoke<{name:string;body:string}[]>('lark_message_templates')]);
    setAttendance(context.attendance||null);
    setTemplates(savedTemplates||[]);
    const docs=context.documents||[];setDocuments([...docs].sort((a,b)=>Number(b.id===documentId)-Number(a.id===documentId)));setMarkdownName(context.markdown?.name||'');
    setReview({chatId:'',title:context.title,meetingTime:context.meetingTime,participants:context.participants,absent:context.absent,host:context.host,summary:context.summary,project:''});setGroups(items);setLast(context.lastDelivery??null);
  });}
  return <><Button size="sm" variant={compact?'ghost':'outline'} className={compact?'h-8 w-8 rounded-full p-0':undefined} aria-label={label} title={`${label}（预览后确认）`} disabled={disabled||busy} onClick={()=>void prepare()}>{compact?<><SendHorizontal className="h-4 w-4"/><span className="sr-only">{label}</span></>:label}</Button>
    <Dialog open={open} onOpenChange={v=>{if(!lock.current)setOpen(v);}}><DialogContent className="max-w-3xl max-h-[90vh] overflow-y-auto">
      <DialogHeader><DialogTitle>会议助手 · 选群发送</DialogTitle><DialogDescription>{documentId?'发送已保存的云文档链接和概要，不会修改云文档。':'核对后发送概要卡片；云文档请从工具栏单独保存。'}</DialogDescription></DialogHeader>
      {!busy&&!groups.length&&<p>未加载到群，请确认机器人已加入目标群且有读取群信息权限。</p>}
      <label>目标群<select aria-label="发送目标群" className="w-full rounded border p-2" disabled={busy||sent} value={review.chatId} onChange={e=>change({chatId:e.target.value})}><option value="">请选择目标群</option>{groups.map(g=><option key={g.id} value={g.id}>{g.name}（{g.id.slice(-6)}）</option>)}</select></label>
      <div className="grid gap-3 sm:grid-cols-2"><label>会议名称<input aria-label="发送会议名称" className="w-full rounded border p-2" maxLength={100} disabled={busy||sent} value={review.title} onChange={e=>change({title:e.target.value})}/></label><label>会议时间（北京时间）<input aria-label="发送会议时间" className="w-full rounded border p-2" type="datetime-local" step={1} disabled={busy||sent} value={review.meetingTime} onChange={e=>change({meetingTime:e.target.value})}/></label></div>
      <div className="grid gap-3 sm:grid-cols-3">{([['participants','出席人员'],['absent','缺席人员'],['host','主持人']] as const).map(([key,name])=><label key={key}>{name}<textarea aria-label={`发送${name}`} className="w-full rounded border p-2" rows={2} maxLength={1000} disabled={busy||sent} value={review[key]} onChange={e=>change({[key]:e.target.value})}/></label>)}</div>
      <AttendanceEvidence attendance={attendance?{...attendance,participantsManual:attendance.participantsManual||!!review.participantsEdited,absentManual:attendance.absentManual||!!review.absentEdited}:null}/>
      <label className="block rounded border bg-muted/30 p-3 text-sm"><input type="checkbox" aria-label="确认出席和缺席名单" checked={!!review.attendanceConfirmed} disabled={busy||sent} onChange={e=>change({attendanceConfirmed:e.target.checked})}/> 我已核对出席、缺席名单；未明确提到的人员保持留空</label>
      <p className="text-xs text-gray-500">修改出席、缺席名单后需重新确认；点击预览时保留到本场会议，不改写云文档正文。</p>
      <label>消息概要<textarea aria-label="发送消息概要" className="w-full rounded border p-2" rows={5} maxLength={1500} disabled={busy||sent} value={review.summary} onChange={e=>change({summary:e.target.value})}/></label><p className="text-xs text-gray-500">修改仅用于群消息，不会修改完整纪要或云文档。</p>
      <section aria-label="发送附件" className="space-y-2 rounded border p-3"><p className="font-medium">附带内容（默认都不勾选）</p>
        <label className="block"><input type="checkbox" aria-label="附云文档链接" checked={includeCloud} disabled={busy||sent||!documents.some(d=>d.available)} onChange={e=>{setIncludeCloud(e.target.checked);change({documentIds:[]});}}/> 附云文档链接（云空间 / 知识库）</label>
        {!documents.some(d=>d.available)&&<p className="text-xs text-gray-500">请先从“云文档”保存并同步正文，保存后重新打开此窗口。</p>}
        {includeCloud&&<fieldset aria-label="发送云文档" className="space-y-2"><legend className="text-xs text-gray-500">可同时选择两篇已保存的纪要，默认不选。未同步最新正文的文档需先重新保存。</legend>{documents.map(d=><label key={d.id} className="flex items-start gap-2 rounded border p-2 text-sm"><input type="checkbox" aria-label={`附上文档：${d.title}（${d.project}）`} checked={selectedDocuments.includes(d.id)} disabled={busy||sent||!d.available||!d.documentUrl||selectedDocuments.length>=2&&!selectedDocuments.includes(d.id)} onChange={e=>change({documentIds:e.target.checked?[...selectedDocuments,d.id]:selectedDocuments.filter(id=>id!==d.id)})}/><span className="min-w-0"><span>{d.documentUrl.includes('/wiki/')?'知识库':'云空间'} · {d.title} · {d.project}</span><span className="block break-all text-xs text-gray-500">{d.documentUrl}</span>{!d.available&&<span className="block text-xs text-amber-700">未保存完成或正文未同步，暂不可选</span>}</span></label>)}</fieldset>}
        <label className="block"><input type="checkbox" aria-label="附 Markdown 文件" checked={!!review.includeMarkdown} disabled={busy||sent||!markdownName} onChange={e=>change({includeMarkdown:e.target.checked})}/> 附 Markdown 文件{markdownName?`（${markdownName}）`:''}</label>
        <p className="text-xs text-gray-500">可同时勾选。Markdown 会作为单独的文件消息发送到同一个群；只在确认发送后上传。</p>
      </section>
      <label>消息模板<select aria-label="Lark 消息模板" className="w-full rounded border p-2" value={template} disabled={busy||sent} onChange={e=>{const next=e.target.value;setTemplate(next);change({messageTemplate:next==='standard'?undefined:next==='simple'?simpleTemplate:next==='custom'?customTemplate:templates.find(t=>`saved:${t.name}`===next)?.body});}}><option value="standard">标准纪要</option><option value="simple">简短通知</option><option value="custom">自定义模板</option>{templates.map(t=><option key={t.name} value={`saved:${t.name}`}>{t.name}</option>)}</select></label>
      {template!=='standard'&&<section className="space-y-2 rounded border p-3"><textarea aria-label="消息模板正文" className="w-full rounded border p-2" rows={6} maxLength={4000} disabled={busy||sent} value={review.messageTemplate||''} onChange={e=>change({messageTemplate:e.target.value})}/><p className="text-xs text-gray-500">可用字段：{'{会议名称}、{会议时间}、{出席人员}、{缺席人员}、{主持人}、{概要}、{云文档链接}'}。预览时自动填入，缺席人员按行显示。</p><div className="flex gap-2"><input aria-label="消息模板名称" className="min-w-0 flex-1 rounded border p-2" placeholder="保存为常用模板的名称" value={templateName} maxLength={80} disabled={busy||sent} onChange={e=>setTemplateName(e.target.value)}/><Button variant="outline" disabled={busy||sent||!templateName.trim()||!review.messageTemplate?.trim()} onClick={()=>void run(async()=>{await invoke('lark_message_template_save',{name:templateName,body:review.messageTemplate});setTemplates(await invoke<{name:string;body:string}[]>('lark_message_templates'));setTemplate(`saved:${templateName.trim()}`);setTemplateName('');})}>保存消息模板</Button></div></section>}
      {last&&<div className="rounded border p-2"><p>这场会议已有发送记录。{last.detail}</p><label><input type="checkbox" aria-label="确认本地会议再次发送" checked={ack} disabled={busy||sent} onChange={e=>setAck(e.target.checked)}/> 已核对，确认需要再次发送</label></div>}
      {!sent&&<Button variant="outline" disabled={busy||!review.attendanceConfirmed||!review.chatId||!review.title.trim()||!review.meetingTime||!review.summary.trim()||includeCloud&&!selectedDocuments.length} onClick={()=>void run(async()=>{setPreview(null);const next=await invoke<Preview>('lark_manual_preview',{meetingId,review});const docs=next.documents||[];if(docs.length!==selectedDocuments.length||selectedDocuments.some(id=>!docs.some(d=>d.id===id&&d.documentUrl)))throw '返回的云文档链接与所选文档不一致，请重新预览。';if(review.includeMarkdown&&!next.includeMarkdown)throw 'Markdown 附件预览缺失，请重新预览。';setPreview(next);})}>预览发送内容</Button>}
      {preview?.includeMarkdown&&<p>将另发 Markdown 文件：{preview.markdown?.name}（{Math.ceil((preview.markdown?.size||0)/1024)} KB）</p>}
      {preview&&<div className="space-y-3"><p>即将发送到：<strong>{preview.chatName}</strong></p>{!!preview.documents?.length&&<div className="space-y-1 text-sm"><p>附上 {preview.documents.length} 篇已保存的纪要，不会再次创建或修改文档。</p>{preview.documents.map(d=><p key={d.id} className="break-all text-xs text-gray-500">{d.documentUrl.includes('/wiki/')?'知识库':'云空间'}：{d.documentUrl}</p>)}</div>}<LarkCardPreview card={preview.card} fallback={review.summary}/><Button disabled={busy||sent||!!last&&!ack} onClick={()=>void run(async()=>{try{await invoke('lark_manual_send',{draftId:preview.id,acknowledgeDuplicateRisk:ack});setSent(true);}catch(e){setPreview(null);setLast({status:'unknown',detail:'请核对群消息后再决定是否重新发送。'});setAck(false);throw e;}})}>{sent?'已发送':busy?'正在发送…':'确认发送到所选群'}</Button></div>}
      {error&&<p role="alert" className="text-red-700">{error}</p>}{sent&&<p role="status" className="text-green-700">已发送到所选群。</p>}<Button variant="ghost" disabled={busy} onClick={()=>setOpen(false)}>关闭</Button>
    </DialogContent></Dialog></>;
}
