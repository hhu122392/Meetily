import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { I18nextProvider } from 'react-i18next';
import { i18n } from '../../frontend/src/i18n';
import { TranscriptCorrectionPanel } from '../../frontend/src/components/MeetingDetails/TranscriptCorrectionPanel';
import { RecordingMeetingSetup } from '../../frontend/src/components/MeetingContext/RecordingMeetingSetup';
import { HelpHint } from '../../frontend/src/components/ui/help-hint';
import type { RecordingMeetingSetupState } from '../../frontend/src/hooks/useRecordingMeetingSetup';
import type { RecordingMeetingContextDraft, MeetingContextProfile } from '../../frontend/src/types/summary-template';
import type { CorrectionCandidate, ProofreadResponse, ProofreadTarget } from '../../frontend/src/lib/transcript-revision';

const profile: MeetingContextProfile = {
  schema_version: 1, fixed_meeting_mechanism: '每周五核对进展，备注放在问号中。', terms: [],
  people: Array.from({ length: 40 }, (_, i) => ({ person_id: `p${i}`, display_name: `测试人员${i + 1}`, department: '研发', role: '参与人', aliases: [], enabled: true })),
};
const candidates: CorrectionCandidate[] = [
  { segment_id:'s1', segment_index:0, audio_start_time:0, original:'店', suggested:'4.1', start_char:9, end_char:10, reason:'rules', confidence:'high', segment_text:'店里的人正在讨论V店。', proposed_text:'店里的人正在讨论V4.1。', sourceKind:'rules' },
  { segment_id:'s1', segment_index:0, audio_start_time:0, original:'V店', suggested:'V4.1', start_char:8, end_char:10, reason:'term', confidence:'high', segment_text:'店里的人正在讨论V店。', proposed_text:'店里的人正在讨论V4.1。', sourceKind:'local' },
];
function Fixture() {
  const [locale, setLocale] = useState('zh-CN');
  const [target, setTarget] = useState<ProofreadTarget>({kind:'summary'});
  const [applied, setApplied] = useState<unknown>(null);
  const [draft, setDraft] = useState<RecordingMeetingContextDraft>({ expectedProfileSha256:'test', attendance:profile.people.map(person => ({personId:person.person_id,attendance:'attending'})), hostPersonId:'p0', guests:[], additionalTerms:[] });
  const setup = {
    templates:[], details:{template:{id:'qa',name:'受控测试模板',version:1}}, profile, draft, isCustomized:false,isLoading:false,error:null,
    updateDraft: (update: (value: RecordingMeetingContextDraft) => RecordingMeetingContextDraft) => setDraft(update),
    selectTemplate:async()=>{},resetAdjustments:async()=>{},prepareRecordingMetadata:async()=>({}),
  } as unknown as RecordingMeetingSetupState;
  const run: ProofreadResponse = {provider:'builtin-ai',model:'QA-local',total_segments:100,reviewed_segments:60,truncated:true,candidates:[],elapsed_ms:1000,warnings:[],prompt_version:'fixture',missing_segments:[],diagnostics_file:null,target:{kind:'summary'},start_index:0,next_start_index:60,source_hash:'fixture'};
  return <main style={{maxWidth:720,margin:'auto',padding:20}}>
    <div className="mb-4 flex flex-wrap items-center justify-between gap-2"><h1 className="font-semibold">受控组件交互测试</h1><button onClick={()=>{const next=locale==='zh-CN'?'en':'zh-CN';setLocale(next);void i18n.changeLanguage(next);}}>中 / EN</button></div>
    <div className="mb-4 flex items-center gap-1"><span>普通说明</span><HelpHint label="测试说明" text="这段说明默认收起，点击问号或使用键盘可展开。" /></div>
    <RecordingMeetingSetup setup={setup} isRecording={false} />
    <TranscriptCorrectionPanel candidates={candidates} ruleCandidateCount={1} appliedRules={['v4.1']} contextPersonCount={40} contextTermCount={0} aiRuns={[run]} isRunningAi={false} isApplying={false} selectedTarget={target}
      modelOptions={[{target:{kind:'summary'},provider:'builtin-ai',model:'QA-local',is_local:true},{target:{kind:'provider',provider:'custom-openai',model:'QA-api'},provider:'custom-openai',model:'QA-api',is_local:false}]}
      onTargetChange={setTarget} onRunAi={()=>setApplied({continued:true})} onApply={setApplied} onCancel={()=>{}} />
    <output id="applied" style={{display:'none'}}>{JSON.stringify(applied)}</output>
    <output id="attendance" style={{display:'none'}}>{JSON.stringify(draft)}</output>
  </main>;
}
void i18n.changeLanguage('zh-CN');
createRoot(document.getElementById('root')!).render(<I18nextProvider i18n={i18n}><Fixture /></I18nextProvider>);
