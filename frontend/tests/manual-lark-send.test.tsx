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
const context={title:'测试会',meetingTime:'2026-11-14T09:30',participants:'甲',absent:'乙',host:'甲',summary:'确认测试范围'};
const card={header:{title:{tag:'plain_text',content:'测试会'}},elements:[]};
async function mount(lastDelivery?:unknown){
  respond=async(command)=>({lark_manual_context:{...context,lastDelivery,markdown:{name:'周会.md'},documents:[{id:'doc1',title:'项目周会',project:'p',documentUrl:'https://team.larksuite.com/docx/doc',available:true}]},lark_manual_groups:[{id:'oc_test',name:'测试群'}],lark_archive_projects:[],lark_manual_preview:{id:'draft',chatName:'测试群',card},lark_manual_send:{sentAt:1}}[command]);
  await act(async()=>{renderer=create(<ManualLarkSend meetingId="local" saveEdits={async()=>{}}/>);});
  await act(async()=>button('选群发送纪要').props.onClick());
}
test('本地录音无需远端会议绑定；选择群和预览不会发送',async()=>{
  await mount();
  expect(field('发送目标群').props.value).toBe('');
  expect(button('预览发送内容').props.disabled).toBe(true);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>button('预览发送内容').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]).toEqual({meetingId:'local',review:{...context,chatId:'oc_test',project:''}});
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_send')?.[1]).toEqual({draftId:'draft',acknowledgeDuplicateRisk:false});
});

test('两个附件默认不选，独立组合，改选必须重新预览且预览不上传',async()=>{
  await mount();expect(field('附云文档链接').props.checked).toBe(false);expect(field('附 Markdown 文件').props.checked).toBe(false);
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  const previous=respond;respond=async(c,args)=>c==='lark_manual_preview'?{id:'draft',chatName:'测试群',card,document:(args?.review as any).documentId?{documentUrl:'https://team.larksuite.com/docx/doc'}:null,includeMarkdown:(args?.review as any).includeMarkdown,markdown:{name:'周会.md',size:120}}:previous(c,args);
  for(const [cloud,markdown] of [[false,true],[true,false],[true,true],[false,false]]){
    await act(async()=>field('附云文档链接').props.onChange({target:{checked:cloud}}));
    await act(async()=>field('附 Markdown 文件').props.onChange({target:{checked:markdown}}));
    expect(button('确认发送到所选群')).toBeUndefined();
    await act(async()=>button('预览发送内容').props.onClick());
    const review=(invoke.mock.calls.filter(c=>c[0]==='lark_manual_preview').at(-1)?.[1] as any).review;
    expect(review.documentId).toBe(cloud?'doc1':undefined);expect(review.includeMarkdown).toBe(markdown);
    expect(button('确认发送到所选群')).toBeDefined();
  }
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send'||c[0].includes('upload'))).toBe(false);
  await act(async()=>button('关闭').props.onClick());await act(async()=>button('选群发送纪要').props.onClick());
  expect(field('附云文档链接').props.checked).toBe(false);expect(field('附 Markdown 文件').props.checked).toBe(false);
});
test('修改群或人员后必须重新预览，发送失败不自动重试',async()=>{
  await mount();
  await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>button('预览发送内容').props.onClick());
  await act(async()=>field('发送缺席人员').props.onChange({target:{value:'丙'}}));
  expect(button('确认发送到所选群')).toBeUndefined();
  await act(async()=>button('预览发送内容').props.onClick());
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
  await act(async()=>button('预览发送内容').props.onClick());
  expect(button('确认发送到所选群').props.disabled).toBe(true);
  await act(async()=>field('确认本地会议再次发送').props.onChange({target:{checked:true}}));
  await act(async()=>button('确认发送到所选群').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_send')?.[1]?.acknowledgeDuplicateRisk).toBe(true);
});
test('模板常用人员只有点击带入时才替换当前名单',async()=>{
  respond=async()=>({participants:'甲、乙、丙',host:'甲'});
  const update=mock(()=>{});
  const settings={templateId:'standard_meeting',customPrompt:'',participants:'本次人员',absent:'乙',host:''};
  await act(async()=>{renderer=create(<TemplatePeople meetingId="one" settings={settings} disabled={false} save={async()=>settings} update={update}/>);});
  expect(update).not.toHaveBeenCalled();
  await act(async()=>button('带入常用参会人').props.onClick());
  expect(update).toHaveBeenCalledWith({participants:'甲、乙、丙',host:'甲',absent:''});
});

test('消息模板切换自动失效旧预览，保存自定义模板不会发送',async()=>{
  await mount();await act(async()=>field('发送目标群').props.onChange({target:{value:'oc_test'}}));
  await act(async()=>field('Lark 消息模板').props.onChange({target:{value:'simple'}}));
  await act(async()=>button('预览发送内容').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_manual_preview')?.[1]?.review).toMatchObject({messageTemplate:'{会议名称}\n会议时间：{会议时间}\n\n{概要}'});
  await act(async()=>field('消息模板正文').props.onChange({target:{value:'会后提醒：{概要}\n{云文档链接}'}}));
  expect(button('确认发送到所选群')).toBeUndefined();
  await act(async()=>field('消息模板名称').props.onChange({target:{value:'项目通知'}}));
  const previous=respond;respond=async(command,args)=>command==='lark_message_templates'?[{name:'项目通知',body:'会后提醒：{概要}\n{云文档链接}'}]:previous(command,args);
  await act(async()=>button('保存消息模板').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['lark_message_template_save',{name:'项目通知',body:'会后提醒：{概要}\n{云文档链接}'}]);
  expect(invoke.mock.calls.some(c=>c[0]==='lark_manual_send')).toBe(false);
});
