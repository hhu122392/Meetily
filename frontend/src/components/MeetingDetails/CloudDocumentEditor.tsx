'use client';
import {useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {CloudUpload} from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {Button} from '@/components/ui/button';
import {Dialog,DialogContent,DialogDescription,DialogHeader,DialogTitle} from '@/components/ui/dialog';
import {ManualLarkSend} from './ManualLarkSend';
import {LarkDirectoryPicker} from './LarkDirectoryPicker';
type FileData={content:string;hash:string;path:string;name?:string};
type DocumentState={id:string;documentUrl?:string;revision:number|null;savedHash:string|null;status:string;detail:string;title?:string|null};
type Project={name:string;parentUrl:string;label?:string};
type LocationContext={projects:Project[];lastProject?:string|null};
export function CloudDocumentEditor({meetingId,saveEdits,disabled}:{meetingId:string;saveEdits:()=>Promise<void>;disabled?:boolean}){
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  const [file,setFile]=useState<FileData|null>(null),[content,setContent]=useState(''),[preview,setPreview]=useState(true);
  const [projects,setProjects]=useState<Project[]>([]),[project,setProject]=useState('');
  const [document,setDocument]=useState<DocumentState|null>(null);
  const [configure,setConfigure]=useState(false),[name,setName]=useState(''),[url,setUrl]=useState('');
  const [sendMessage,setSendMessage]=useState(false);
  const [title,setTitle]=useState(''),[savedTitle,setSavedTitle]=useState(''),[filename,setFilename]=useState('');
  const lock=useRef(false);
  const dirty=!!file&&(file.content!==content||filename!==(file.name||file.path.split(/[\\/]/).pop()||'会议纪要.md'));
  const titleDirty=title!==savedTitle;
  const synced=!!file&&!dirty&&!titleDirty&&document?.status==='ready'&&document.savedHash===file.hash;
  async function run(action:()=>Promise<void>){
    if(lock.current)return;lock.current=true;setBusy(true);setError('');setNotice('');
    try{await action();}catch(e){setError(String(e));}finally{lock.current=false;setBusy(false);}
  }
  async function prepare(){
    if(lock.current)return;
    setOpen(true);setFile(null);setContent('');setDocument(null);setProject('');setPreview(true);setSendMessage(false);
    await run(async()=>{
      await saveEdits();await invoke('summary_export_markdown',{meetingId});
      const [local,context]=await Promise.all([invoke<FileData>('summary_markdown_read',{meetingId}),invoke<LocationContext>('lark_location_context')]);
      const items=context.projects;setFile(local);setContent(local.content);setFilename(local.name||local.path.split(/[\\/]/).pop()||'会议纪要.md');setProjects(items);setConfigure(false);
      const defaultTitle=/^#\s+(.+)$/m.exec(local.content)?.[1]||'会议纪要';setTitle(defaultTitle);setSavedTitle(defaultTitle);
      const selected=items.some(p=>p.name===context.lastProject)?context.lastProject:items.length===1?items[0].name:'';
      if(selected){setProject(selected);const doc=await invoke<DocumentState|null>('lark_document_status',{meetingId,project:selected});setDocument(doc);if(doc?.title){setTitle(doc.title);setSavedTitle(doc.title);}}
    });
  }
  async function loadDocument(next:string){
    const doc=await invoke<DocumentState|null>('lark_document_status',{meetingId,project:next});setDocument(doc);
    if(doc?.title){setTitle(doc.title);setSavedTitle(doc.title);}
  }
  async function saveLocal(){
    if(!file)throw '本地纪要尚未加载。';
    let updated=await invoke<FileData>('summary_markdown_save',{meetingId,expectedHash:file.hash,content});
    setFile(updated);setContent(updated.content);
    const oldName=updated.name||updated.path.split(/[\\/]/).pop()||'会议纪要.md';
    if(filename!==oldName)updated=await invoke<FileData>('summary_markdown_rename',{meetingId,expectedHash:updated.hash,expectedName:oldName,name:filename});
    setFile(updated);setFilename(updated.name||filename);return updated;
  }
  function close(){if(busy)return;if(dirty||titleDirty){setError('有未保存的修改，请先保存草稿；云文档名称需保存到云端，或放弃未保存修改。');return;}setOpen(false);}
  return <>
    <Button size="sm" variant="outline" className="h-8 gap-1.5 rounded-full px-3" aria-label="云文档" disabled={disabled||busy} onClick={()=>void prepare()}><CloudUpload className="h-4 w-4"/>云文档</Button>
    <Dialog open={open} onOpenChange={next=>{if(!next)close();}}>
      <DialogContent className="max-w-5xl max-h-[90vh] overflow-y-auto" onEscapeKeyDown={e=>{if(dirty||titleDirty||busy)e.preventDefault();}} onPointerDownOutside={e=>{if(dirty||titleDirty||busy)e.preventDefault();}}>
        <DialogHeader><DialogTitle>云文档 · 编辑与预览</DialogTitle><DialogDescription>编辑完整 Markdown 纪要。预览和保存草稿不上传；保存到云文档不发群。只有单独选择发送到群并确认，才会发送消息。</DialogDescription></DialogHeader>
        {file&&<>
          <div className="grid gap-3 sm:grid-cols-2"><label>云文档名称<input aria-label="云文档名称" className="w-full rounded border p-2" value={title} maxLength={200} disabled={busy} onChange={e=>setTitle(e.target.value)}/></label><label>Markdown 文件名<input aria-label="Markdown 文件名" className="w-full rounded border p-2" value={filename} maxLength={120} disabled={busy} onChange={e=>setFilename(e.target.value)}/></label></div>
          <p className="text-xs text-gray-500">两个名称可分别修改。文件名随草稿保存；云文档名称随云端保存更新，不改变正文中的标题。</p>
          <div className="flex gap-2"><Button variant={!preview?'default':'outline'} onClick={()=>setPreview(false)}>编辑正文</Button><Button variant={preview?'default':'outline'} onClick={()=>setPreview(true)}>预览正文</Button></div>
          {preview?<div aria-label="云文档正文预览" className="prose prose-sm dark:prose-invert max-w-none min-h-64 overflow-x-auto rounded border p-4 [&_table]:w-full [&_table]:border-collapse [&_th]:border [&_th]:p-2 [&_td]:border [&_td]:p-2"><ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={{img:({alt})=><span>［图片：{alt||'未命名'}］</span>,a:({children})=><span className="text-blue-600 underline">{children}</span>}}>{content}</ReactMarkdown></div>:<textarea aria-label="云文档正文" className="min-h-72 w-full rounded border p-3 font-mono text-sm" value={content} disabled={busy} onChange={e=>{setContent(e.target.value);setNotice('');}}/>}
          <p className="text-xs text-gray-500">这是本地草稿预览。表格使用 Markdown 表格语法编辑，云端实际排版可在保存后打开 Lark 核对。</p>
          <label>保存位置<select aria-label="云文档归档项目" className="w-full rounded border p-2" disabled={busy} value={project} onChange={e=>{const next=e.target.value;setProject(next);setDocument(null);setSendMessage(false);void run(async()=>{if(next)await loadDocument(next);});}}><option value="">请选择保存位置</option>{projects.map(p=><option key={p.name} value={p.name}>{p.label||p.name}</option>)}</select></label>
          <LarkDirectoryPicker disabled={busy} onSelected={async(next)=>{setProject(next);setDocument(null);setSendMessage(false);await run(async()=>{const context=await invoke<LocationContext>('lark_location_context');setProjects(context.projects);await loadDocument(next);});}}/>
          <p className="text-xs text-gray-500">保存成功后会记住此位置。更换位置会在新目录建立另一篇文档，不移动原文档。</p>
          <details open={configure} onToggle={e=>setConfigure(e.currentTarget.open)} className="rounded border p-2"><summary>手动填写知识库链接（备用）</summary>
            <input aria-label="归档项目名称" className="my-2 w-full rounded border p-2" placeholder="项目名称，如战略会" value={name} disabled={busy} onChange={e=>setName(e.target.value)}/>
            <input aria-label="知识库目录链接" className="w-full rounded border p-2" placeholder="Lark 知识库目录链接" value={url} disabled={busy} onChange={e=>setUrl(e.target.value)}/>
            <Button variant="outline" disabled={busy||!name.trim()||!url.trim()} onClick={()=>void run(async()=>{await invoke('lark_archive_project_save',{name,parentUrl:url});setProjects((await invoke<LocationContext>('lark_location_context')).projects);setProject(name.trim());setSendMessage(false);await loadDocument(name.trim());setConfigure(false);setName('');setUrl('');})}>保存项目目录</Button>
          </details>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" disabled={busy||!content.trim()} onClick={()=>void run(async()=>{await saveLocal();setNotice('草稿已保存到本地，未上传、未发群。');})}>保存草稿</Button>
            <Button disabled={busy||!project||!title.trim()||!content.trim()||!!document&&document.status!=='ready'} onClick={()=>void run(async()=>{
              const local=await saveLocal();
              try{const doc=await invoke<DocumentState>('lark_document_save',{meetingId,project,expectedHash:local.hash,expectedRevision:document?.revision??null,title});setDocument(doc);setTitle(doc.title||title);setSavedTitle(doc.title||title);setNotice('已保存到云文档，未发送到群。');}
              catch(e){try{setDocument(await invoke<DocumentState|null>('lark_document_status',{meetingId,project}));}catch{}throw e;}
            })}>{document?.documentUrl?'保存到云文档':'创建并保存云文档'}</Button>
            {document?.documentUrl&&<Button variant="outline" disabled={busy} onClick={()=>void run(async()=>{await invoke('open_external_url',{url:document.documentUrl});})}>打开 Lark 云文档</Button>}
          </div>
          <section aria-label="Lark 消息" className="space-y-2 rounded border p-3"><p className="font-medium">Lark 消息</p><label><input type="checkbox" aria-label="发送 Lark 消息" checked={sendMessage} disabled={busy} onChange={e=>setSendMessage(e.target.checked)}/> 需要发送到群（默认不发送）</label><p className="text-xs text-gray-500">勾选只显示发送入口；保存云文档不会发送。仍需选群、选模板、预览并确认。</p>{sendMessage&&(document?<ManualLarkSend key={document.id} meetingId={meetingId} documentId={document.id} saveEdits={async()=>{}} disabled={busy||!synced}/>:<p>请先保存云文档，再单独发送链接。</p>)}</section>
          {document&&<p className="text-sm">{document.status!=='ready'?`上次保存结果未确认，请打开 Lark 核对。${document.detail}`:synced?'本地草稿与上次保存版本一致。再次保存和发送前会检查云端版本。':'草稿有未同步到云端的修改，请先保存到云文档。'}</p>}
        </>}
        {busy&&<p role="status">正在处理，请稍候…</p>}{notice&&<p role="status" className="text-green-700">{notice}</p>}{error&&<p role="alert" className="text-red-700">{error}</p>}
        <div className="flex gap-2"><Button variant="ghost" disabled={busy} onClick={close}>关闭</Button>{(dirty||titleDirty)&&<Button variant="ghost" disabled={busy} onClick={()=>setOpen(false)}>放弃未保存修改</Button>}</div>
      </DialogContent>
    </Dialog>
  </>;
}
