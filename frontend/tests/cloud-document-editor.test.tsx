import {afterEach,expect,mock,test} from 'bun:test';
import {act,create,type ReactTestRenderer} from 'react-test-renderer';
import type {ReactNode} from 'react';
let file={path:'D:/纪要.md',hash:'h1',content:'# 会议\n\n| 事项 | 人员 |\n| --- | --- |\n| 验收 | 甲 |'};
let document:any=null;
let projects:any[]=[{name:'战略会',parentUrl:'https://team.larksuite.com/wiki/project'}];
let saveError='';
const invoke=mock(async(command:string,args?:any):Promise<any>=>{
  if(command==='summary_markdown_read')return {...file};
  if(command==='lark_archive_projects')return projects;
  if(command==='lark_location_context')return {projects,lastProject:projects.at(-1)?.name};
  if(command==='lark_document_status')return document;
  if(command==='summary_markdown_save'){file={...file,content:args.content,hash:args.content};return {...file};}
  if(command==='lark_document_save'){if(saveError)throw saveError;document={id:'binding',title:args.title,documentUrl:'https://team.larksuite.com/wiki/doc',revision:document?8:4,savedHash:file.hash,status:'ready',detail:''};return {...document};}
  if(command==='lark_archive_project_save'){projects=[{name:args.name,parentUrl:args.parentUrl}];}
  if(command==='lark_manual_context')return {title:'会议',meetingTime:'2026-11-16T10:00',participants:'甲',absent:'',host:'甲',summary:'核对验收',markdown:{name:'纪要.md'},documents:document?[{...document,title:document.title||'会议',project:'战略会',available:true}]:[]};
  if(command==='lark_manual_groups')return [{id:'oc_test',name:'测试群'}];
  if(command==='lark_manual_preview')return {id:'draft',chatName:'测试群',card:{header:{title:{tag:'plain_text',content:'会议'}},elements:[]},documents:args.review.documentIds?.map((id:string)=>({id,documentUrl:document.documentUrl}))||[]};
});
mock.module('@tauri-apps/api/core',()=>({invoke}));
const Box=({children}:{children?:ReactNode})=><div>{children}</div>;
mock.module('../src/components/ui/dialog',()=>({Dialog:({open,children}:{open:boolean;children?:ReactNode})=>open?<div>{children}</div>:null,DialogContent:Box,DialogHeader:Box,DialogTitle:Box,DialogDescription:Box}));
const {CloudDocumentEditor}=await import('../src/components/MeetingDetails/CloudDocumentEditor');
let renderer:ReactTestRenderer;
const button=(label:string)=>renderer.root.findAllByType('button').find(b=>b.props['aria-label']===label||b.children.includes(label))!;
const field=(label:string)=>renderer.root.findByProps({'aria-label':label});
async function mount(){await act(async()=>{renderer=create(<CloudDocumentEditor meetingId="m1" saveEdits={async()=>{}}/>);});await act(async()=>button('云文档').props.onClick());}
afterEach(async()=>{if(renderer)await act(async()=>renderer.unmount());invoke.mockClear();document=null;saveError='';projects=[{name:'战略会',parentUrl:'https://team.larksuite.com/wiki/project'}];});
test('默认预览完整表格，不加载群、不保存云端、不发送',async()=>{
  await mount();expect(renderer.root.findAllByType('table')).toHaveLength(1);
  expect(field('云文档归档项目').props.value).toBe('战略会');
  expect(invoke.mock.calls.some(c=>['lark_manual_groups','lark_manual_send','lark_document_save'].includes(c[0]))).toBe(false);
  expect(button('发送到群')).toBeUndefined();
});
test('编辑与预览共用未保存草稿，本地保存不需要项目或群',async()=>{
  projects=[];await mount();expect(renderer.root.findByType('details').props.open).toBe(false);
  await act(async()=>button('编辑正文').props.onClick());
  await act(async()=>field('云文档正文').props.onChange({target:{value:'# 修改\n\n|A|B|\n|-|-|\n|1|2|'}}));
  await act(async()=>button('预览正文').props.onClick());expect(renderer.root.findAllByType('table')).toHaveLength(1);
  await act(async()=>button('关闭').props.onClick());expect(JSON.stringify(renderer.toJSON())).toContain('有未保存的修改');
  expect(button('创建并保存云文档').props.disabled).toBe(true);
  await act(async()=>button('保存草稿').props.onClick());
  expect(invoke.mock.calls.some(c=>['lark_document_save','lark_manual_send'].includes(c[0]))).toBe(false);
});
test('保存云端独立完成，后续使用同一文档版本更新，不会自动发群',async()=>{
  await mount();await act(async()=>button('创建并保存云文档').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_document_save')?.[1]?.expectedRevision).toBeNull();
  expect(field('发送 Lark 消息').props.checked).toBe(false);expect(button('发送到群')).toBeUndefined();
  await act(async()=>field('发送 Lark 消息').props.onChange({target:{checked:true}}));
  expect(button('发送到群').props.disabled).toBe(false);
  await act(async()=>button('编辑正文').props.onClick());await act(async()=>field('云文档正文').props.onChange({target:{value:'# 第二版'}}));
  expect(button('发送到群').props.disabled).toBe(true);
  await act(async()=>button('保存到云文档').props.onClick());
  const saves=invoke.mock.calls.filter(c=>c[0]==='lark_document_save');expect(saves[1][1]?.expectedRevision).toBe(4);
  expect(document.id).toBe('binding');expect(button('打开 Lark 云文档')).toBeDefined();
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_groups'||c[0]==='lark_manual_send')).toBe(false);
});
test('云端冲突保留本地草稿，并禁止发送未同步内容',async()=>{
  await mount();await act(async()=>button('创建并保存云文档').props.onClick());
  await act(async()=>field('发送 Lark 消息').props.onChange({target:{checked:true}}));
  await act(async()=>button('编辑正文').props.onClick());await act(async()=>field('云文档正文').props.onChange({target:{value:'# 冲突稿'}}));
  saveError='云文档已被其他操作修改，本次未覆盖';
  await act(async()=>button('保存到云文档').props.onClick());
  expect(field('云文档正文').props.value).toBe('# 冲突稿');expect(file.content).toBe('# 冲突稿');
  expect(button('发送到群').props.disabled).toBe(true);expect(JSON.stringify(renderer.toJSON())).toContain('本次未覆盖');
});
test('发送需单独打开选群和确认，预览只附已保存链接',async()=>{
  await mount();await act(async()=>button('创建并保存云文档').props.onClick());invoke.mockClear();
  await act(async()=>field('发送 Lark 消息').props.onChange({target:{checked:true}}));
  await act(async()=>button('发送到群').props.onClick());
  expect(field('附云文档链接').props.checked).toBe(false);
  await act(async()=>field('附云文档链接').props.onChange({target:{checked:true}}));
  expect(field(`附上文档：${document.title}（战略会）`).props.checked).toBe(false);
  await act(async()=>field(`附上文档：${document.title}（战略会）`).props.onChange({target:{checked:true}}));
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>field('确认出席和缺席名单').props.onChange({target:{checked:true}}));
  await act(async()=>button('预览发送内容').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]?.review.documentIds).toEqual(['binding']);
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send'||c[0]==='lark_document_save'||c[0]==='summary_export_markdown')).toBe(false);
  await act(async()=>button('确认发送到所选群').props.onClick());expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_send')).toHaveLength(1);
});

test('云文档名称可独立修改，保存时沿用原版本与链接',async()=>{
  await mount();await act(async()=>field('云文档名称').props.onChange({target:{value:'战略周会纪要'}}));
  await act(async()=>button('创建并保存云文档').props.onClick());
  expect(document.title).toBe('战略周会纪要');const url=document.documentUrl;
  await act(async()=>field('云文档名称').props.onChange({target:{value:'战略周会修订稿'}}));
  await act(async()=>button('保存到云文档').props.onClick());expect(document.title).toBe('战略周会修订稿');expect(document.documentUrl).toBe(url);
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_document_save').at(-1)?.[1]).toMatchObject({title:'战略周会修订稿',expectedRevision:4});
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
});

test('优先恢复上次成功保存的位置，重新打开不记忆发送勾选',async()=>{
  projects=[{name:'wiki',label:'知识库 / 战略会'},{name:'drive',label:'云空间 / 会议'}];await mount();
  expect(field('云文档归档项目').props.value).toBe('drive');
  await act(async()=>field('发送 Lark 消息').props.onChange({target:{checked:true}}));
  await act(async()=>button('关闭').props.onClick());await act(async()=>button('云文档').props.onClick());
  expect(field('发送 Lark 消息').props.checked).toBe(false);
  expect(invoke.mock.calls.some(c=>c[0]==='lark_directory_list'||c[0]==='lark_manual_groups'||c[0]==='lark_manual_send')).toBe(false);
});
