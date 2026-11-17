'use client';
import {useEffect,useRef,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {Button} from '@/components/ui/button';
type Entry={token:string;name:string;space:string;origin:string};
type Folder=Entry&{kind:'spaces'|'wiki'|'drive'};
type Page={items:Entry[];nextPageToken:string};
type Tree={items:(Entry&{path:Entry[]})[];complete:boolean;notice:string;cached:boolean;requests:number;updatedAt:number};
type Recent={name:string;label?:string};
const wikiRoot:Folder={kind:'spaces',space:'',token:'',name:'知识库',origin:'https://larksuite.com'};
const driveRoot:Folder={kind:'drive',space:'',token:'',name:'我的云空间',origin:'https://larksuite.com'};
export function LarkDirectoryPicker({disabled,onSelected}:{disabled?:boolean;onSelected:(project:string)=>Promise<void>}){
  const [open,setOpen]=useState(false),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  const [folder,setFolder]=useState<Folder>(wikiRoot),[trail,setTrail]=useState<Folder[]>([wikiRoot]);
  const [page,setPage]=useState<Page>({items:[],nextPageToken:''}),[pageLoaded,setPageLoaded]=useState(false);
  const cache=useRef(new Map<string,Page>()),lock=useRef(false),scope=useRef<string|null>(null),scanRef=useRef('');
  const [tree,setTree]=useState<Tree|null>(null),[search,setSearch]=useState(''),[visible,setVisible]=useState(200),[scan,setScan]=useState(''),[progress,setProgress]=useState({requests:0,loaded:0});
  const [recent,setRecent]=useState<Recent[]>([]),[last,setLast]=useState(''),[recentChoice,setRecentChoice]=useState('');
  const treeRoot=useRef<Folder|null>(null);
  useEffect(()=>()=>{if(scanRef.current)void invoke('lark_directory_cancel',{scanId:scanRef.current}).catch(()=>{});},[]);
  useEffect(()=>{if(!scan)return;let active=true,pending=false;const timer=setInterval(()=>{if(pending)return;pending=true;void invoke<{requests:number;loaded:number}>('lark_directory_progress',{scanId:scan}).then(p=>{if(active)setProgress(p);}).catch(()=>{}).finally(()=>{pending=false;});},500);return()=>{active=false;clearInterval(timer);};},[scan]);
  async function openPicker(){
    if(lock.current)return;setOpen(true);let changed=false,hasRecent=false;
    await run(async()=>{const context=await invoke<{projects:Recent[];lastProject?:string;cacheScope?:string}>('lark_location_context');
      const current=context?.cacheScope??'';changed=scope.current!==current;if(changed){cache.current.clear();setTree(null);scope.current=current;}
      setRecent(context?.projects??[]);setLast(context?.lastProject??'');setRecentChoice(context?.lastProject??'');setOpen(true);
      hasRecent=!!context?.projects?.length;
    });
    if(changed){if(hasRecent){setPageLoaded(false);setFolder(wikiRoot);setTrail([wikiRoot]);setPage({items:[],nextPageToken:''});setNotice('可直接使用已保存位置；需要浏览其他目录时点击“知识库”或“云空间”。');}else{await start(wikiRoot);}}
  }
  async function loadTree(force=false){
    const root=trail.find(p=>p.kind==='wiki'&&!p.token);if(!root)return;
    await run(async()=>{const id=crypto.randomUUID();scanRef.current=id;setScan(id);setProgress({requests:0,loaded:0});
      try{const result=await invoke<Tree>('lark_directory_tree',{space:root.space,scanId:id,force});treeRoot.current=root;setTree(result);setFolder(root);setTrail([wikiRoot,root]);setSearch('');setVisible(200);setNotice(result.notice||(result.cached?'已使用本机缓存；需要最新目录时点击“刷新全部目录”。':'已加载当前知识库的全部可见目录。'));}
      finally{scanRef.current='';setScan('');}
    });
  }
  async function useRecent(project:string){await run(async()=>{await onSelected(project);setOpen(false);});}
  const matches=tree?.items.filter(item=>item.path.map(p=>p.name).join(' / ').toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())).sort((a,b)=>a.path.map(p=>p.name).join(' / ').localeCompare(b.path.map(p=>p.name).join(' / '),'zh-CN'))??[];
  async function run(action:()=>Promise<void>){if(lock.current)return;lock.current=true;setBusy(true);setError('');setNotice('');try{await action();}catch(e){setError(String(e));}finally{lock.current=false;setBusy(false);}}
  async function load(next:Folder,path:Folder[],force=false,more=false){
    await run(async()=>{
      setTree(null);const key=JSON.stringify([next.kind,next.space,next.token]);const cached=cache.current.get(key);
      if(cached&&!force&&!more){setFolder(next);setTrail(path);setPage(cached);setPageLoaded(true);return;}
      const cursor=more?page.nextPageToken:'';
      const result=await invoke<Page>('lark_directory_list',{kind:next.kind,space:next.space,parent:next.token,pageToken:cursor});
      if(result.nextPageToken&&result.nextPageToken===cursor)throw '目录分页未前进，请刷新目录。';
      const merged={...result,items:more?[...new Map([...page.items,...result.items].map(i=>[i.token,i])).values()]:result.items};
      cache.current.set(key,merged);setFolder(next);setTrail(path);setPage(merged);setPageLoaded(true);
    });
  }
  async function start(root:Folder){setPageLoaded(false);setFolder(root);setTrail([root]);setPage({items:[],nextPageToken:''});await load(root,[root]);}
  return <section className="space-y-2 rounded border p-3">
    <Button variant="outline" disabled={disabled||busy} onClick={()=>void openPicker()}>选择云空间或知识库目录</Button>
    {open&&<>
      {recent.length>0&&<div className="space-y-2 rounded bg-muted/40 p-2"><p className="text-sm font-medium">上次位置与最近目录</p><div className="flex flex-wrap gap-2">{last&&recent.some(p=>p.name===last)&&<Button variant="outline" disabled={busy||disabled} onClick={()=>void useRecent(last)}>使用上次保存位置</Button>}<select aria-label="已保存目录" className="min-w-0 flex-1 rounded border bg-background p-2 text-sm" disabled={busy||disabled} value={recentChoice} onChange={e=>setRecentChoice(e.target.value)}><option value="">选择已保存位置</option>{recent.map(p=><option key={p.name} value={p.name}>{p.label||p.name}</option>)}</select><Button disabled={busy||disabled||!recentChoice} onClick={()=>void useRecent(recentChoice)}>使用此位置</Button></div></div>}
      <div className="flex flex-wrap gap-2"><Button variant={folder.kind==='drive'?'default':'outline'} disabled={busy||disabled} onClick={()=>void start(driveRoot)}>云空间</Button><Button variant={folder.kind!=='drive'?'default':'outline'} disabled={busy||disabled} onClick={()=>void start(wikiRoot)}>知识库</Button><Button variant="ghost" disabled={busy||disabled} onClick={()=>void (tree?loadTree(true):load(folder,trail,true))}>{tree?'刷新全部目录':'刷新目录'}</Button>{folder.kind==='wiki'&&!tree&&<Button variant="outline" disabled={busy||disabled} onClick={()=>void loadTree()}>一键加载本知识库全部目录</Button>}</div>
      {folder.kind==='drive'&&<div><p className="text-xs text-gray-500">读取当前授权账号的云空间；群成员仍需文档访问权限。</p><Button variant="outline" disabled={busy||disabled} onClick={()=>void run(async()=>{cache.current.clear();const url=await invoke<string>('lark_app_authorize',{documentsOnly:true});await invoke('open_external_url',{url});setNotice('请在 Lark 完成授权，然后点击“刷新目录”。');})}>连接我的云空间</Button></div>}
      <nav aria-label="目录路径" className="flex flex-wrap gap-1">{trail.map((item,index)=><Button key={`${item.kind}:${item.token}:${index}`} size="sm" variant="ghost" disabled={busy||disabled} onClick={()=>void load(item,trail.slice(0,index+1))}>{item.name}{index<trail.length-1?' /':''}</Button>)}</nav>
      {tree&&<div className="space-y-2"><input aria-label="搜索知识库目录" className="w-full rounded border bg-background px-3 py-2 text-sm" placeholder="搜索目录名称或完整路径，无需逐层进入" value={search} disabled={busy} onChange={e=>{setSearch(e.target.value);setVisible(200);}}/><p className="text-xs text-gray-500">{tree.complete?'全部可见目录':'部分目录'}：{tree.items.length} 项 · {tree.cached?'本机缓存':'本次读取 '+tree.requests+' 次目录接口'}。选中目标后点击“保存到此目录”。</p><div role="tree" aria-label="知识库目录树" className="max-h-72 overflow-auto rounded border">{matches.slice(0,visible).map(item=><button type="button" role="treeitem" aria-selected={folder.token===item.token} aria-level={item.path.length} key={item.token} disabled={busy||disabled} className={'block w-full border-b py-2 pr-3 text-left text-sm hover:bg-muted '+(folder.token===item.token?'bg-muted font-medium':'')} style={{paddingLeft:12+Math.min(item.path.length-1,8)*12}} title={item.path.map(p=>p.name).join(' / ')} onClick={()=>{if(!treeRoot.current)return;setFolder({...item,kind:'wiki'});setTrail([wikiRoot,treeRoot.current,...item.path.map(p=>({...p,kind:'wiki' as const}))]);setError('');}}>📁 {item.name}<span className="block text-xs font-normal text-gray-500">{item.path.map(p=>p.name).join(' / ')}</span></button>)}{!matches.length&&<p className="p-3 text-sm">没有匹配的目录。</p>}</div>{matches.length>visible&&<Button variant="ghost" onClick={()=>setVisible(v=>v+200)}>显示更多结果（本地）</Button>}</div>}
      {!tree&&<div className="max-h-56 overflow-y-auto rounded border" aria-label="Lark 目录列表">
        {page.items.map(item=><button type="button" key={item.token} className="block w-full border-b px-3 py-2 text-left hover:bg-muted" disabled={busy||disabled} onClick={()=>{const next:Folder={...item,kind:folder.kind==='spaces'?'wiki':folder.kind,space:folder.kind==='spaces'?item.token:folder.space,token:folder.kind==='spaces'?'':item.token};void load(next,[...trail,next]);}}>📁 {item.name} ›</button>)}
        {!busy&&!page.items.length&&!error&&<p className="p-3 text-sm text-gray-500">{pageLoaded?<>当前层级没有可见子目录。{folder.kind==='spaces'?'请确认会议助手有知识库访问权。':''}</>:'请选择上方的知识库或云空间开始浏览，或直接使用已保存位置。'}</p>}
      </div>}
      {!tree&&page.nextPageToken&&<Button variant="outline" disabled={busy||disabled} onClick={()=>void load(folder,trail,false,true)}>加载更多目录</Button>}
      <div className="flex gap-2"><Button disabled={busy||disabled||folder.kind==='spaces'||!!error} onClick={()=>void run(async()=>{const target={kind:folder.kind,space:folder.space,token:folder.token,origin:folder.origin,name:trail.map(p=>p.name).join(' / ')};const project=await invoke<string>('lark_location_choose',{target});await onSelected(project);setOpen(false);})}>保存到此目录</Button><Button variant="ghost" disabled={busy} onClick={()=>setOpen(false)}>收起目录</Button></div>
      {busy&&<p role="status">{scan?`正在加载：已发现 ${progress.loaded} 个目录，已请求 ${progress.requests} 页…`:'正在读取目录…'}</p>}{scan&&<Button variant="outline" onClick={()=>void invoke('lark_directory_cancel',{scanId:scan}).catch(e=>setError(String(e)))}>取消加载</Button>}{notice&&<p role="status">{notice}</p>}{error&&<p role="alert" className="text-red-700">{error}</p>}
    </>}
  </section>;
}
