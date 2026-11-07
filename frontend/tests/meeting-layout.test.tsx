import { afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
import { useEffect } from 'react';
import { initializeCompanyScope } from '../src/lib/company-scope';
initializeCompanyScope('layout-test-company');
let desktop=true;
let notify:()=>void=()=>{};
const values=new Map<string,string>();
Object.assign(globalThis,{localStorage:{getItem:(k:string)=>values.get(k)??null,setItem:(k:string,v:string)=>values.set(k,v)},window:{matchMedia:()=>({get matches(){return desktop;},addEventListener:(_:string,cb:()=>void)=>{notify=cb;},removeEventListener:()=>{}})}});
mock.module('@/i18n',()=>({t:(s:string)=>s,useUiLanguage:()=> 'zh-CN'}));
const {MeetingDetailsSplitView}=await import('../src/components/MeetingDetails/MeetingDetailsSplitView');
let mounts=0;let view:ReactTestRenderer;
function Editor(){useEffect(()=>{mounts++;},[]);return <textarea defaultValue="未保存的纪要"/>;}
const render=(tab:'split'|'summary'|'transcript')=><MeetingDetailsSplitView transcript={<div>转写</div>} summary={<Editor/>} activeTab={tab} onTabChange={()=>{}}/>;
afterEach(async()=>{await act(async()=>view?.unmount());mounts=0;desktop=true;values.clear();});
test('三种视图切换和窄窗口保持编辑器挂载，不丢失未保存内容',async()=>{
  await act(async()=>{view=create(render('split'));});
  expect(view.root.findAllByProps({role:'separator'})).toHaveLength(1);
  await act(async()=>view.update(render('transcript')));
  expect(view.root.findByProps({'aria-label':'Summary'}).props.style.display).toBe('none');
  await act(async()=>view.update(render('summary')));
  expect(view.root.findByProps({'aria-label':'Transcript'}).props.style.display).toBe('none');
  await act(async()=>{view.update(render('split'));desktop=false;notify();});
  expect(view.root.findAllByProps({role:'separator'})).toHaveLength(0);
  expect(view.root.findByProps({'aria-label':'Summary'}).props.style.display).toBe('flex');
  expect(mounts).toBe(1);
});
test('键盘调整分栏比例保存到当前公司',async()=>{
  await act(async()=>{view=create(render('split'));});
  await act(async()=>view.root.findByProps({role:'separator'}).props.onKeyDown({key:'ArrowRight',preventDefault(){}}));
  expect(values.get('meetily.meetingDetails.transcriptPaneRatio:company:layout-test-company')).toBe('0.35');
});
