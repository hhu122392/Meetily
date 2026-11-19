import { afterAll,afterEach,beforeEach,expect,mock,test } from 'bun:test';
import { act,create,type ReactTestRenderer } from 'react-test-renderer';
import type { ReactNode } from 'react';
import { testInvoke as invoke, setTestInvoke, restoreTestInvoke } from './lib/tauri-test-invoke';
const originalDialog={...await import('../src/components/ui/dialog')};
let respond:(command:string,args?:Record<string,unknown>)=>Promise<unknown>;
beforeEach(()=>{setTestInvoke((command,args)=>respond(command,args));});
const Container=({children}:{children?:ReactNode})=><div>{children}</div>;
mock.module('../src/components/ui/dialog',()=>({...originalDialog,Dialog:({open,children}:{open:boolean;children?:ReactNode})=>open?<div>{children}</div>:null,DialogContent:Container,DialogHeader:Container,DialogTitle:Container,DialogDescription:Container}));
const {ManualLarkSend}=await import('../src/components/MeetingDetails/ManualLarkSend');
const {TemplatePeople}=await import('../src/components/MeetingDetails/TemplatePeople');
let renderer:ReactTestRenderer|undefined;
afterEach(async()=>{if(renderer)await act(async()=>renderer!.unmount());renderer=undefined;invoke.mockClear();});
afterAll(()=>{restoreTestInvoke();mock.module('../src/components/ui/dialog',()=>originalDialog);});
const button=(text:string)=>renderer!.root.findAllByType('button').find(b=>b.children.filter(x=>typeof x==='string').join('').includes(text))!;
const field=(label:string)=>renderer!.root.findByProps({'aria-label':label});
async function preview(){await act(async()=>field('确认出席和缺席名单').props.onChange({target:{checked:true}}));await act(async()=>button('预览发送内容').props.onClick());}
const context={title:'测试会',meetingTime:'2026-11-14T09:30',participants:'甲',absent:'乙',host:'甲',summary:'确认测试范围'};
const card={header:{title:{tag:'plain_text',content:'测试会'}},elements:[]};
async function mount(lastDelivery?:unknown,documents=[{id:'doc1',title:'项目周会',project:'p',documentUrl:'https://team.larksuite.com/docx/doc',available:true}]){
  respond=async(command)=>({lark_manual_context:{...context,lastDelivery,markdown:{name:'周会.md'},documents},lark_manual_groups:[{id:'oc_test',name:'测试群'}],lark_archive_projects:[],lark_manual_preview:{id:'draft',chatName:'测试群',card},lark_manual_send:{sentAt:1}}[command]);
  await act(async()=>{renderer=create(<ManualLarkSend meetingId="local" saveEdits={async()=>{}}/>);});
  await act(async()=>button('选群发送纪要').props.onClick());
}
test('本地录音无需远端会议绑定；选择群和预览不会发送',async()=>{
  await mount();
  expect(field('发送目标群').props.value).toBe('');
  expect(button('预览发送内容').props.disabled).toBe(true);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  expect(field('确认出席和缺席名单').props.checked).toBe(false);expect(button('预览发送内容').props.disabled).toBe(true);
  await preview();
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]).toEqual({meetingId:'local',review:{...context,chatId:'oc_test',project:'',attendanceConfirmed:true}});
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_send')?.[1]).toEqual({draftId:'draft',acknowledgeDuplicateRisk:false});
});

test('每次打开自动加载群，默认关联群可手动改选且不会自动发送',async()=>{
  await mount();
  await act(async()=>button('关闭').props.onClick());
  const previous=respond;
  respond=async(c,args)=>c==='lark_manual_context'?{...context,defaultChatId:'oc_test'}:c==='lark_manual_groups'?[{id:'oc_other',name:'另一个群'},{id:'oc_test',name:'关联群'}]:previous(c,args);
  await act(async()=>button('选群发送纪要').props.onClick());
  expect(field('发送目标群').props.value).toBe('oc_test');
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_groups')).toHaveLength(2);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_other'}}));
  await preview();
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]?.review).toMatchObject({chatId:'oc_other'});
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
});

test('关联群不在最新可发送列表中时留空，不能默认选第一个群',async()=>{
  await mount();await act(async()=>button('关闭').props.onClick());
  const previous=respond;
  respond=async(c,args)=>c==='lark_manual_context'?{...context,defaultChatId:'oc_removed'}:previous(c,args);
  await act(async()=>button('选群发送纪要').props.onClick());
  expect(field('发送目标群').props.value).toBe('');
  expect(button('预览发送内容').props.disabled).toBe(true);
});

test('关闭重开会刷新群列表，加载失败不沿用上次目标群',async()=>{
  await mount();await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>button('关闭').props.onClick());
  const previous=respond;
  respond=async(c,args)=>{if(c==='lark_manual_groups')throw '群列表加载失败，请重新打开重试。';return previous(c,args);};
  await act(async()=>button('选群发送纪要').props.onClick());
  expect(field('发送目标群').props.value).toBe('');
  expect(button('预览发送内容').props.disabled).toBe(true);
  expect(renderer!.root.findByProps({role:'alert'}).children.join('')).toContain('群列表加载失败');
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
});

test('两个附件默认不选，独立组合，改选必须重新预览且预览不上传',async()=>{
  await mount();expect(field('附云文档链接').props.checked).toBe(false);expect(field('附 Markdown 文件').props.checked).toBe(false);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  const previous=respond;respond=async(c,args)=>c==='lark_manual_preview'?{id:'draft',chatName:'测试群',card,documents:((args?.review as any).documentIds||[]).map((id:string)=>({id,documentUrl:'https://team.larksuite.com/docx/doc'})),includeMarkdown:(args?.review as any).includeMarkdown,markdown:{name:'周会.md',size:120}}:previous(c,args);
  for(const [cloud,markdown] of [[false,true],[true,false],[true,true],[false,false]]){
    await act(async()=>field('附云文档链接').props.onChange({target:{checked:cloud}}));
    if(cloud)await act(async()=>field('附上文档：项目周会（p）').props.onChange({target:{checked:true}}));
    await act(async()=>field('附 Markdown 文件').props.onChange({target:{checked:markdown}}));
    expect(button('确认发送到所选群')).toBeUndefined();
    await preview();
    const review=(invoke.mock.calls.filter(c=>c[0]==='lark_manual_preview').at(-1)?.[1] as any).review;
    expect(review.documentIds).toEqual(cloud?['doc1']:[]);expect(review.includeMarkdown).toBe(markdown);
    expect(button('确认发送到所选群')).toBeDefined();
  }
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send'||c[0].includes('upload'))).toBe(false);
  await act(async()=>button('关闭').props.onClick());await act(async()=>button('选群发送纪要').props.onClick());
  expect(field('附云文档链接').props.checked).toBe(false);expect(field('附 Markdown 文件').props.checked).toBe(false);
});
test('两种保存位置可同时选择，未同步文档不可选，预览缺少任一链接都不能发送',async()=>{
  const docs=[
    {id:'drive',title:'周会',project:'云盘目录',documentUrl:'https://team.larksuite.com/docx/drive',available:true},
    {id:'wiki',title:'周会',project:'知识库目录',documentUrl:'https://team.larksuite.com/wiki/wiki',available:true},
    {id:'stale',title:'旧版',project:'未同步',documentUrl:'https://team.larksuite.com/docx/stale',available:false},
  ];
  await mount(undefined,docs);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>field('附云文档链接').props.onChange({target:{checked:true}}));
  expect(button('预览发送内容').props.disabled).toBe(true);
  expect(field('附上文档：周会（云盘目录）').props.checked).toBe(false);
  expect(field('附上文档：周会（知识库目录）').props.checked).toBe(false);
  expect(field('附上文档：旧版（未同步）').props.disabled).toBe(true);
  await act(async()=>field('附上文档：周会（云盘目录）').props.onChange({target:{checked:true}}));
  await act(async()=>field('附上文档：周会（知识库目录）').props.onChange({target:{checked:true}}));
  let missing=true;const previous=respond;
  respond=async(c,args)=>c==='lark_manual_preview'?{id:'draft',chatName:'测试群',card,documents:missing?[docs[0]]:docs.filter(d=>(args?.review as any).documentIds.includes(d.id))}:previous(c,args);
  await preview();
  expect(button('确认发送到所选群')).toBeUndefined();
  expect(renderer!.root.findByProps({role:'alert'}).children.join('')).toContain('所选文档不一致');
  missing=false;await preview();
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_preview').at(-1)?.[1]?.review).toMatchObject({documentIds:['drive','wiki']});
  expect(button('确认发送到所选群')).toBeDefined();
  expect(JSON.stringify(renderer!.toJSON())).toContain('https://team.larksuite.com/wiki/wiki');
  await act(async()=>field('附上文档：周会（云盘目录）').props.onChange({target:{checked:false}}));
  expect(button('确认发送到所选群')).toBeUndefined();
  expect(field('附上文档：周会（知识库目录）').props.checked).toBe(true);
  await preview();
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send'||c[0]==='lark_document_save')).toBe(false);
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_send')).toHaveLength(1);
});

test('没有已保存可用文档时不能勾选链接，也不会自动创建文档',async()=>{
  await mount(undefined,[]);
  expect(field('附云文档链接').props.disabled).toBe(true);
  expect(field('附云文档链接').props.checked).toBe(false);
  expect(invoke.mock.calls.some(c=>c[0]==='lark_document_save')).toBe(false);
});

test('修改群或人员后必须重新预览，发送失败不自动重试',async()=>{
  await mount();
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await preview();
  await act(async()=>field('发送缺席人员').props.onChange({target:{value:'丙'}}));
  expect(field('确认出席和缺席名单').props.checked).toBe(false);expect(button('预览发送内容').props.disabled).toBe(true);
  expect(button('确认发送到所选群')).toBeUndefined();
  await preview();
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_preview').at(-1)?.[1]?.review).toMatchObject({absentEdited:true,attendanceConfirmed:true,absent:'丙'});
  respond=async()=>{throw '网络失败，结果不确定';};
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_manual_send')).toHaveLength(1);
  expect(button('确认发送到所选群')).toBeUndefined();
  expect(field('确认本地会议再次发送').props.checked).toBe(false);
});
test('历史发送记录需要明确确认，默认不归档云文档',async()=>{
  await mount({status:'sent',detail:'已发送'});
  expect(invoke.mock.calls.some(c=>c[0]==='lark_archive_projects')).toBe(false);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await preview();
  expect(button('确认发送到所选群').props.disabled).toBe(true);
  await act(async()=>field('确认本地会议再次发送').props.onChange({target:{checked:true}}));
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_send')?.[1]?.acknowledgeDuplicateRisk).toBe(true);
});
test('常用人员仅作候选，逐人选择不会覆盖其他已填写人员',async()=>{
  respond=async()=>({participants:'甲、乙、丙',host:'甲'});
  const update=mock(()=>{});
  const settings={templateId:'standard_meeting',customPrompt:'',participants:'本次人员',absent:'乙',host:''};
  await act(async()=>{renderer=create(<TemplatePeople meetingId="one" settings={settings} disabled={false} save={async()=>settings} update={update}/>);});
  expect(update).not.toHaveBeenCalled();
  await act(async()=>field('常用人员 甲').props.onChange({target:{value:'present'}}));
  expect(update).toHaveBeenCalledWith({participants:'本次人员、甲'});
  expect(field('常用人员 丙').props.value).toBe('');
  await act(async()=>field('常用人员 乙').props.onChange({target:{value:'present'}}));
  expect(update).toHaveBeenLastCalledWith({participants:'本次人员、乙',absent:''});
});

test('消息模板切换自动失效旧预览，保存自定义模板不会发送',async()=>{
  await mount();await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>field('Lark 消息模板').props.onChange({target:{value:'simple'}}));
  await preview();
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]?.review).toMatchObject({messageTemplate:'{会议名称}\n会议时间：{会议时间}\n\n{概要}'});
  await act(async()=>field('消息模板正文').props.onChange({target:{value:'会后提醒：{概要}\n{云文档链接}'}}));
  expect(button('确认发送到所选群')).toBeUndefined();
  await act(async()=>field('消息模板名称').props.onChange({target:{value:'项目通知'}}));
  const previous=respond;respond=async(command,args)=>command==='lark_message_templates'?[{name:'项目通知',body:'会后提醒：{概要}\n{云文档链接}'}]:previous(command,args);
  await act(async()=>button('保存消息模板').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['lark_message_template_save',{name:'项目通知',body:'会后提醒：{概要}\n{云文档链接}'}]);
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
});
