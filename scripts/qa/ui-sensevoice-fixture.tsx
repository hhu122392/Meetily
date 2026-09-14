// Actual components with an explicit fake IPC transport. No model files or user data are written.
import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { I18nextProvider } from 'react-i18next';
import { i18n } from '../../frontend/src/i18n';
import { SenseVoiceModelManager } from '../../frontend/src/components/SenseVoiceModelManager';
import { OnboardingProvider } from '../../frontend/src/contexts/OnboardingContext';
import { DownloadProgressStep } from '../../frontend/src/components/onboarding/steps/DownloadProgressStep';
import { SENSEVOICE_BYTES } from '../../frontend/src/lib/sensevoice';
const calls: {command:string;args:unknown}[] = [];
let state = { status:'missing', downloaded_bytes:0,total_bytes:SENSEVOICE_BYTES };
let pending: {resolve:()=>void;reject:(e:Error)=>void} | null = null;
Object.assign(window, {
  qaCalls:calls,
  qaProgress:()=>{state={...state, downloaded_bytes:Math.floor(SENSEVOICE_BYTES / 2)};},
  qaFail:()=>{state={...state,status:'error'};pending?.reject(new Error('QA offline'));pending=null;},
  qaComplete:()=>{state={...state,status:'available',downloaded_bytes:SENSEVOICE_BYTES};pending?.resolve();pending=null;},
  qaReset:()=>{state={status:'missing',downloaded_bytes:0,total_bytes:SENSEVOICE_BYTES};calls.length=0;},
  __TAURI_INTERNALS__:{
    transformCallback:()=>1,
    unregisterCallback:()=>{},
    invoke:async(command:string,args:unknown)=>{
      calls.push({command,args});
      if(command==='sensevoice_get_download_state') return {...state};
      if(command==='sensevoice_download_model') {
        state={...state,status:'downloading'};
        return new Promise<void>((resolve,reject)=>{pending={resolve,reject};});
      }
      if(command==='builtin_ai_get_recommended_model') return 'qwen3.5:2b';
      if(command==='builtin_ai_is_model_ready'||command==='check_first_launch') return false;
      if(command==='get_onboarding_status') return null;
      return 1;
    },
  },
  __TAURI_EVENT_PLUGIN_INTERNALS__:{unregisterListener:()=>{}},
});
function Fixture() {
  const [mode,setMode]=useState<'settings'|'closed'|'onboarding'>('settings');
  const [selected,setSelected]=useState<string>();
  return <main style={{maxWidth:850,padding:24,margin:'auto'}}>
    <p className="mb-4 text-sm text-gray-500">QA · 真实组件，模拟下载传输</p>
    <nav className="mb-6 flex flex-wrap gap-4">
      <button onClick={()=>setMode('settings')}>设置场景</button><button onClick={()=>setMode('closed')}>关闭场景</button>
      <button onClick={()=>setMode('onboarding')}>首次设置场景</button>
      <button onClick={()=>void i18n.changeLanguage(i18n.language==='en'?'zh-CN':'en')}>中 / EN</button>
    </nav>
    {mode==='settings'&&<SenseVoiceModelManager selectedModel={selected} onModelSelect={setSelected}/>}
    {mode==='onboarding'&&<OnboardingProvider><DownloadProgressStep/></OnboardingProvider>}
    <output id="selected" hidden>{selected}</output>
  </main>;
}
void i18n.changeLanguage('zh-CN');
createRoot(document.getElementById('root')!).render(<I18nextProvider i18n={i18n}><Fixture/></I18nextProvider>);
