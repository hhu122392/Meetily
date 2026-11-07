import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { newTemplate, validateTemplateDraft, type TemplateDocument } from '../src/lib/summary-template';

const events = new EventTarget();
Object.assign(globalThis, { window: events });
const builtin: TemplateDocument = {...newTemplate(), id:'standard_meeting', name:'标准会议', description:'用于会议', source:{type:'builtin'}};
builtin.sections[0] = {...builtin.sections[0],title:'摘要',instruction:'根据转写概括'};
let docs = new Map<string, TemplateDocument>();
let defaultId = 'standard_meeting';
let rejectSave = false;
const invoke = mock(async (command: string, args: any) => {
  if(command==='template_library_list')return {templates:[builtin,...docs.values()].map(d=>({...d,readonly:d.id==='standard_meeting'})),defaultTemplateId:defaultId,warnings:[]};
  if(command==='template_library_read')return structuredClone(args.templateId==='standard_meeting'?builtin:docs.get(args.templateId));
  if(command==='template_library_duplicate') { const copy={...structuredClone(builtin),id:'custom_copy',name:'标准会议（副本）'};docs.set(copy.id,copy);return structuredClone(copy); }
  if(command==='template_library_save') { if(rejectSave)throw Error('模板已在其他窗口修改');const doc={...args.document,version:args.expectedVersion+1};docs.set(doc.id,doc);return doc; }
  if(command==='template_library_set_default'){defaultId=args.templateId;return;}
  if(command==='template_library_export')return null;
  if(command==='template_library_import'){const doc={...structuredClone(builtin),id:'custom_import'};docs.set(doc.id,doc);return doc;}
  if(command==='template_library_delete'){docs.delete(args.templateId);return;}
  throw Error(command);
});
mock.module('@tauri-apps/api/core',()=>({invoke}));
mock.module('@/components/ui/button',()=>({Button:({children,...props}:any)=><button {...props}>{children}</button>}));
mock.module('@/components/ui/dialog',()=>({Dialog:({open,children}:any)=>open?<div>{children}</div>:null,DialogContent:({children}:any)=><div>{children}</div>,DialogTitle:({children}:any)=><h2>{children}</h2>,DialogDescription:({children}:any)=><p>{children}</p>}));
const { TemplateLibrary } = await import('../src/components/templates/TemplateLibrary');
let view:ReactTestRenderer;
function text(node:any):string{return typeof node==='string'?node:node.children?.map(text).join('')??'';}
function button(label:string){return view.root.findAllByType('button').find(b=>text(b)===label)!;}
afterEach(async()=>{await act(async()=>view?.unmount());docs.clear();defaultId='standard_meeting';rejectSave=false;invoke.mockClear();});

test('复制内置模板后编辑和设默认；并发冲突保留编辑内容',async()=>{
  await act(async()=>{view=create(<TemplateLibrary/>);});
  await act(async()=>{await button('复制').props.onClick();});
  expect(docs.size).toBe(1);
  const name=view.root.findAllByType('input').find(n=>n.props.value==='标准会议（副本）')!;
  await act(async()=>name.props.onChange({target:{value:'研发周会'}}));
  rejectSave=true;
  await act(async()=>{await button('保存模板').props.onClick();});
  expect(view.root.findAllByType('input').some(n=>n.props.value==='研发周会')).toBe(true);
  expect(view.root.findAllByProps({role:'alert'}).some(n=>text(n).includes('其他窗口'))).toBe(true);
  rejectSave=false;
  await act(async()=>{await button('保存模板').props.onClick();});
  expect(docs.get('custom_copy')?.name).toBe('研发周会');
  expect(builtin.name).toBe('标准会议');
  const setDefault=view.root.findAllByType('button').find(b=>text(b)==='设为默认'&&!b.props.disabled)!;
  await act(async()=>{await setDefault.props.onClick();});
  expect(defaultId).toBe('custom_copy');
  const saveCall=invoke.mock.calls.find(([cmd])=>cmd==='template_library_save')!;
  expect(saveCall[1].expectedVersion).toBe(1);
});

test('超大 JSON 在读取前拒绝，导入失败不会覆盖已有模板',async()=>{
  await act(async()=>{view=create(<TemplateLibrary/>);});
  const fileInput=view.root.findAllByType('input').find(n=>n.props.type==='file')!;
  const read=mock(async()=>'{}');
  await act(async()=>fileInput.props.onChange({target:{files:[{size:800001,text:read}],value:'file'}}));
  expect(read).not.toHaveBeenCalled();expect(docs.size).toBe(0);
  expect(view.root.findAllByProps({role:'alert'}).some(n=>text(n).includes('800KB'))).toBe(true);
});

test('章节标题重复及空指令阻止保存',()=>{
  const doc=structuredClone(builtin);doc.sections.push({...doc.sections[0],id:'second'});
  expect(validateTemplateDraft(doc)).toContain('重复');doc.sections.pop();doc.sections[0].instruction=' ';
  expect(validateTemplateDraft(doc)).toContain('生成要求');
});

test('取消原生导出不会显示导出成功',async()=>{
  await act(async()=>{view=create(<TemplateLibrary/>);});
  await act(async()=>{await button('导出').props.onClick();});
  expect(view.root.findByProps({role:'status'}).children).toEqual(['已取消导出']);
  expect(invoke.mock.calls.some(([cmd])=>cmd==='template_library_export')).toBe(true);
});
