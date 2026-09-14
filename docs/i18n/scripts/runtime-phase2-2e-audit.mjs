#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9562);
const outputDirectory = path.resolve(process.argv[3] || "docs/i18n/audit/phase-2-react/2E/runtime-final");
const backendSentinel = "PHASE2_2E_RAW_BACKEND_ERROR_SENTINEL";
const audioFile = "D:\\2E Audit\\board-meeting.mp4";
const audioTitle = "Q3 Board Meeting 审计";
const meetingId = "phase2-2e-meeting";
const recoveryId = "phase2-2e-recovery";

class CdpClient {
  constructor(url) { this.url = url; this.nextId = 1; this.pending = new Map(); this.events = []; }
  async connect() {
    this.socket = new WebSocket(this.url);
    await new Promise((resolve, reject) => {
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) return void this.events.push(message);
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
      else pending.resolve(message.result);
    });
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }
  close() { this.socket.close(); }
}

async function findTarget() {
  let lastError;
  for (let attempt = 0; attempt < 160; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
      const target = targets.find((candidate) => candidate.type === "page");
      if (target?.webSocketDebuggerUrl) return target;
    } catch (error) { lastError = error; }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`No page target on port ${port}: ${String(lastError || "timeout")}`);
}

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  return result.result.value;
}

async function waitFor(client, expression, description, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(client, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const debug = await evaluate(client, `({url:location.href,lang:document.documentElement.lang,text:document.body?.innerText?.slice(0,3000),mock:Boolean(window.__PHASE2_2E_MOCK__),ready:document.readyState})`).catch((error) => ({ evaluationError: String(error) }));
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(debug)}`);
}

async function waitForReady(client) {
  await waitFor(client, "Boolean(document.body&&document.querySelector('[data-i18n-ready=\"true\"]'))", "the i18n provider", 45000);
}

async function setLocaleLive(client, locale) {
  await evaluate(client, `(()=>{localStorage.setItem('meetily.uiLocale',${JSON.stringify(locale)});window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:${JSON.stringify(locale)}}));})()`);
  await waitFor(client, `document.documentElement.lang===${JSON.stringify(locale)}`, `${locale} without reload`);
}

async function clickElement(client, expression, description) {
  const point = await evaluate(client, `(()=>{const element=${expression};if(!element)return null;element.scrollIntoView({block:'center',inline:'center'});const rect=element.getBoundingClientRect();return{x:rect.left+rect.width/2,y:rect.top+rect.height/2};})()`);
  if (!point) throw new Error(`Clickable control not found: ${description}`);
  await new Promise((resolve) => setTimeout(resolve, 100));
  await client.send("Input.dispatchMouseEvent", { type: "mousePressed", x: point.x, y: point.y, button: "left", clickCount: 1 });
  await client.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: point.x, y: point.y, button: "left", clickCount: 1 });
}

async function clickText(client, wanted, contains = false) {
  const match = contains ? "value.includes(wanted)||label.includes(wanted)" : "value===wanted||label===wanted";
  return clickElement(client, `(()=>{const wanted=${JSON.stringify(wanted)};return [...document.querySelectorAll('button,a,[role=\"button\"],[role=\"tab\"]')].find((item)=>{const value=(item.textContent||'').trim();const label=item.getAttribute('aria-label')||'';const rect=item.getBoundingClientRect();return (${match})&&!item.disabled&&item.getAttribute('aria-disabled')!=='true'&&rect.width>0&&rect.height>0;})})()`, wanted);
}

async function invoke(client, command, args = {}) {
  return evaluate(client, `(async()=>{try{return{resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return{resolved:false,error:String(error)}}})()`);
}

async function emit(client, event, payload) {
  const result = await invoke(client, "plugin:event|emit", { event, payload });
  if (!result.resolved) throw new Error(`Unable to emit ${event}: ${result.error}`);
}

async function snapshot(client, name) {
  await new Promise((resolve) => setTimeout(resolve, 400));
  const state = await evaluate(client, `(()=>{const all=[...document.querySelectorAll('button,a,input,[role="button"],[role="tab"],[role="option"],[role="combobox"]')].map((element)=>{const rect=element.getBoundingClientRect();const intersects=rect.right>0&&rect.bottom>0&&rect.left<innerWidth&&rect.top<innerHeight;return{tag:element.tagName.toLowerCase(),type:element.getAttribute('type'),text:(element.textContent||'').trim(),value:element.value??null,ariaLabel:element.getAttribute('aria-label'),title:element.getAttribute('title'),placeholder:element.getAttribute('placeholder'),disabled:Boolean(element.disabled)||element.getAttribute('aria-disabled')==='true',intersects,rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};});const controls=all.filter((item)=>item.intersects);const mock=window.__PHASE2_2E_MOCK__;return{url:location.href,htmlLang:document.documentElement.lang,bodyText:document.body.innerText,viewport:{width:innerWidth,height:innerHeight},documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},controls,formValues:all.filter((item)=>item.tag==='input').map((item)=>({type:item.type,value:item.value,placeholder:item.placeholder})),horizontalClipping:controls.filter((item)=>item.rect.left<0||item.rect.right>innerWidth+1),unnamedButtons:controls.filter((item)=>item.tag==='button'&&!item.text&&!item.ariaLabel&&!item.title),mock:mock?{importStarts:mock.importStarts,importCancels:mock.importCancels,retranscriptionStarts:mock.retranscriptionStarts,retranscriptionCancels:mock.retranscriptionCancels,recoverySaves:mock.recoverySaves,failNextImportCancel:mock.failNextImportCancel,failNextRetranscriptionCancel:mock.failNextRetranscriptionCancel,lastImportArgs:mock.lastImportArgs,lastRetranscriptionArgs:mock.lastRetranscriptionArgs}:null};})()`);
  const image = await client.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(state, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, "base64")),
  ]);
  return state;
}

function mockBootstrapSource() {
  const meeting = { id: meetingId, title: "2E 审计会议", created_at: "2026-08-23T01:00:00.000Z", updated_at: "2026-08-23T01:05:00.000Z", folder_path: "D:\\2E Audit\\meeting" };
  const transcripts = [{ id: "2e-segment-1", text: "This is controlled audit transcript content.", timestamp: "2026-08-23T01:00:10.000Z", confidence: 0.94, audio_start_time: 10, audio_end_time: 14 }];
  const state = { importStarts: 0, importCancels: 0, retranscriptionStarts: 0, retranscriptionCancels: 0, recoverySaves: 0, failNextValidation: false, failNextImportCancel: false, failNextRetranscriptionCancel: false, lastImportArgs: null, lastRetranscriptionArgs: null };
  const fileInfo = { path: audioFile, filename: "board-meeting.mp4", duration_seconds: 3723, size_bytes: 73400320, format: "MP4" };
  const models = [{ name: "large-v3-turbo", size_mb: 1550, status: "Available" }];
  return `(()=>{if(window.__PHASE2_2E_MOCK_INSTALLED__)return;window.__PHASE2_2E_MOCK_INSTALLED__=true;const nativeFetch=window.fetch.bind(window);const state=window.__PHASE2_2E_MOCK__=${JSON.stringify(state)};const meeting=${JSON.stringify(meeting)};const transcripts=${JSON.stringify(transcripts)};const fileInfo=${JSON.stringify(fileInfo)};const models=${JSON.stringify(models)};const ok=(value)=>new Response(JSON.stringify(value??null),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});const fail=(value)=>new Response(JSON.stringify(value),{status:500,headers:{'Content-Type':'application/json','Tauri-Response':'error'}});window.fetch=async(input,init={})=>{const raw=typeof input==='string'?input:input?.url;let url;try{url=new URL(raw,location.href);}catch{return nativeFetch(input,init);}if(url.hostname!=='ipc.localhost')return nativeFetch(input,init);const command=decodeURIComponent(url.pathname.slice(1));let args={};if(typeof init?.body==='string'){try{args=JSON.parse(init.body)||{};}catch{}}if(command==='parakeet_init')return ok(null);if(command==='parakeet_has_available_models')return ok(true);if(command==='parakeet_get_available_models')return ok([]);if(command==='whisper_get_available_models')return ok(models);if(command==='is_recording')return ok(false);if(command==='get_recording_state')return ok({is_recording:false,is_paused:false,is_active:false,recording_duration:null,active_duration:null});if(command==='api_get_meetings')return ok([{id:meeting.id,title:meeting.title}]);if(command==='api_search_transcripts')return ok([]);if(command==='api_get_meeting_metadata')return ok(meeting);if(command==='api_get_meeting_transcripts')return ok({transcripts,total_count:transcripts.length,has_more:false});if(command==='api_get_summary')return ok({status:'completed',data:{markdown:'# Audit summary'}});if(command==='api_get_model_config')return ok({provider:'ollama',model:'audit-model',whisperModel:'large-v3',apiKey:null,ollamaEndpoint:null});if(command==='api_get_api_key'||command==='api_get_transcript_api_key')return ok(null);if(command==='api_get_auto_generate_setting')return ok(false);if(command==='api_get_transcript_config')return ok({provider:'localWhisper',model:'large-v3-turbo',apiKey:null});if(command==='get_ollama_models')return ok([{name:'audit-model'}]);if(command==='get_recording_preferences')return ok({save_folder:'D:\\2E Audit',auto_save:true,file_format:'mp4',preferred_mic_device:null,preferred_system_device:null});if(command==='get_notification_settings')return ok({recording_notifications:false,time_based_reminders:false,meeting_reminders:false,respect_do_not_disturb:true,notification_sound:false,system_permission_granted:true,consent_given:true,manual_dnd_mode:false,notification_preferences:{}});if(command==='get_audio_devices')return ok([]);if(command==='api_list_templates_v2')return ok({templates:[],diagnostics:[],deletedTemplates:[],defaultTemplateId:'standard_meeting'});if(command==='api_get_meeting_template_preference')return ok({preference:{schemaVersion:1,mode:'inherit',templateId:null,templateVersion:null,templateFileSha256:null,selectedAt:'2026-08-23T00:00:00.000Z'},storage:'metadata',resolved:{templateId:'standard_meeting',name:'Standard Meeting',version:1,fileSha256:'audit',origin:'builtin',source:'global_default'}});if(command==='api_get_meeting_summary_language'||command==='api_get_meeting_detected_summary_language')return ok({language:'zh',storage:'metadata'});if(command==='select_and_validate_audio_command'||command==='validate_audio_file_command'){if(state.failNextValidation){state.failNextValidation=false;return fail(${JSON.stringify(backendSentinel)});}return ok(fileInfo);}if(command==='start_import_audio_command'){state.importStarts+=1;state.lastImportArgs=args;return ok({message:'started'});}if(command==='cancel_import_command'){if(state.failNextImportCancel){state.failNextImportCancel=false;return fail(${JSON.stringify(backendSentinel)});}state.importCancels+=1;return ok(null);}if(command==='start_retranscription_command'){state.retranscriptionStarts+=1;state.lastRetranscriptionArgs=args;return ok({meeting_id:meeting.id,message:'started'});}if(command==='cancel_retranscription_command'){if(state.failNextRetranscriptionCancel){state.failNextRetranscriptionCancel=false;return fail(${JSON.stringify(backendSentinel)});}state.retranscriptionCancels+=1;return ok(null);}if(command==='has_audio_checkpoints')return ok(false);if(command==='get_meeting_folder_path')return fail(${JSON.stringify(backendSentinel)});if(command==='api_save_transcript'){state.recoverySaves+=1;return ok({meeting_id:'phase2-2e-recovered'});}if(command==='open_meeting_folder'||command==='api_save_meeting_summary'||command==='api_save_meeting_title'||command==='api_save_model_config'||command==='set_recording_preferences'||command==='set_notification_settings')return ok(null);return nativeFetch(input,init);};})();`;
}

async function seedRecoveryFixture(client) {
  return evaluate(client, `(async()=>{const meeting=${JSON.stringify({ meetingId: recoveryId, title: "崩溃恢复审计会议", startTime: Date.now() - 120000, lastUpdated: Date.now() - 60000, transcriptCount: 2, savedToSQLite: false })};const transcripts=${JSON.stringify([
    { meetingId: recoveryId, text: "第一段恢复内容", timestamp: "2026-08-23T01:00:00.000Z", confidence: 0.95, sequenceId: 1, storedAt: Date.now() - 60000, audio_start_time: 0 },
    { meetingId: recoveryId, text: "Second recovery segment", timestamp: "2026-08-23T01:00:05.000Z", confidence: 0.92, sequenceId: 2, storedAt: Date.now() - 59000, audio_start_time: 5 },
  ])};const db=await new Promise((resolve,reject)=>{const request=indexedDB.open('MeetilyRecoveryDB',1);request.onupgradeneeded=()=>{const database=request.result;if(!database.objectStoreNames.contains('meetings')){const store=database.createObjectStore('meetings',{keyPath:'meetingId'});store.createIndex('lastUpdated','lastUpdated',{unique:false});store.createIndex('savedToSQLite','savedToSQLite',{unique:false});}if(!database.objectStoreNames.contains('transcripts')){const store=database.createObjectStore('transcripts',{keyPath:'id',autoIncrement:true});store.createIndex('meetingId','meetingId',{unique:false});store.createIndex('storedAt','storedAt',{unique:false});}};request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});await new Promise((resolve,reject)=>{const transaction=db.transaction(['meetings','transcripts'],'readwrite');const meetingsStore=transaction.objectStore('meetings');const transcriptsStore=transaction.objectStore('transcripts');meetingsStore.clear();transcriptsStore.clear();meetingsStore.put(meeting);for(const transcript of transcripts)transcriptsStore.add(transcript);transaction.oncomplete=resolve;transaction.onerror=()=>reject(transaction.error);});db.close();sessionStorage.removeItem('recovery_dialog_shown');return true;})()`);
}

function includesAll(text, values) { return values.every((value) => text.includes(value)); }

function diagnostics(events) {
  const exceptions = events.filter((event) => event.method === "Runtime.exceptionThrown");
  const consoleErrors = events.filter((event) => event.method === "Runtime.consoleAPICalled" && event.params?.type === "error");
  const expectedConsoleErrors = consoleErrors.filter((event) => JSON.stringify(event).includes(backendSentinel));
  const unexpectedConsoleErrors = consoleErrors.filter((event) => !JSON.stringify(event).includes(backendSentinel));
  const logErrors = events.filter((event) => event.method === "Log.entryAdded" && event.params?.entry?.level === "error");
  const serialized = JSON.stringify(events);
  return { exceptions: exceptions.length, consoleErrors: consoleErrors.length, expectedConsoleErrors: expectedConsoleErrors.length, unexpectedConsoleErrors: unexpectedConsoleErrors.length, logErrors: logErrors.length, csp: (serialized.match(/content security policy|blocked by csp/gi) || []).length, exceptionEvents: exceptions, unexpectedConsoleErrorEvents: unexpectedConsoleErrors, logErrorEvents: logErrors };
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const target = await findTarget();
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([client.send("Runtime.enable"), client.send("Page.enable"), client.send("Log.enable"), client.send("Network.enable")]);
  await client.send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  try {
    await waitForReady(client);
    await evaluate(client, "localStorage.setItem('meetily.uiLocale','zh-CN');localStorage.setItem('betaFeatures',JSON.stringify({importAndRetranscribe:true}))");
    const onboarding = await invoke(client, "save_onboarding_status_cmd", { status: { version: "1.0", completed: true, current_step: 4, model_status: { parakeet: "downloaded", summary: "downloaded", selected_summary_model: "audit-model" }, last_updated: new Date().toISOString() } });
    if (!onboarding.resolved) throw new Error(`Unable to seed onboarding: ${onboarding.error}`);
    await client.send("Page.addScriptToEvaluateOnNewDocument", { source: mockBootstrapSource() });
    await client.send("Page.navigate", { url: "http://tauri.localhost/" });
    await waitForReady(client);
    await waitFor(client, "window.__PHASE2_2E_MOCK__&&document.documentElement.lang==='zh-CN'&&Boolean(document.querySelector('button[aria-label=\"导入音频\"]'))", "Chinese home with import action", 45000);

    await emit(client, "tauri://drag-enter", {});
    await waitFor(client, "document.body.innerText.includes('拖放音频文件以导入')", "localized drag overlay");
    const dropOverlayZh = await snapshot(client, "00a-drag-overlay-zh-CN");
    await emit(client, "tauri://drag-leave", {});
    await waitFor(client, "!document.body.innerText.includes('拖放音频文件以导入')", "drag overlay dismissal");

    await clickText(client, "导入音频");
    await waitFor(client, "document.body.innerText.includes('导入音频文件')&&document.body.innerText.includes('选择音频文件')", "Chinese import dialog");
    const importEmptyZh = await snapshot(client, "01-import-empty-zh-CN");
    await evaluate(client, "window.__PHASE2_2E_MOCK__.failNextValidation=true");
    await clickText(client, "选择音频文件");
    await waitFor(client, `document.body.innerText.includes('无法验证所选音频文件')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "safe invalid-file error");
    const invalidFileZh = await snapshot(client, "01a-import-invalid-file-safe-zh-CN");
    await clickText(client, "重试");
    await waitFor(client, "document.body.innerText.includes('选择音频文件')", "import retry state");
    await clickText(client, "选择音频文件");
    await waitFor(client, "document.querySelector('input[placeholder=\"输入会议标题\"]')?.value==='board-meeting.mp4'", "validated audio file");
    await evaluate(client, `(()=>{const input=document.querySelector('input[placeholder="输入会议标题"]');const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(input,${JSON.stringify(audioTitle)});input.dispatchEvent(new Event('input',{bubbles:true}));})()`);
    await clickText(client, "高级选项");
    await waitFor(client, "document.body.innerText.includes('语言')&&document.body.innerText.includes('模型')&&document.body.innerText.includes('Whisper')", "advanced import settings");
    const importConfiguredZh = await snapshot(client, "02-import-configured-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, `document.body.innerText.includes('Import Audio File')&&document.querySelector('input[placeholder="Enter meeting title"]')?.value===${JSON.stringify(audioTitle)}`, "English import dialog with preserved state");
    const importStateEn = await snapshot(client, "03-import-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await clickText(client, "导入");
    await waitFor(client, "window.__PHASE2_2E_MOCK__.importStarts===1", "import start");
    await emit(client, "import-progress", { stage: "transcribing", progress_percentage: 63, message: backendSentinel });
    await waitFor(client, `document.body.innerText.includes('正在转写语音')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "localized import progress");
    const importProgressZh = await snapshot(client, "04-import-progress-safe-zh-CN");
    await evaluate(client, "window.__PHASE2_2E_MOCK__.failNextImportCancel=true");
    await clickText(client, "取消");
    await waitFor(client, "document.body.innerText.includes('无法取消导入')&&document.body.innerText.includes('正在导入音频')", "failed import cancellation remains active");
    const importCancelFailure = await snapshot(client, "05-import-cancel-failure-stays-active-zh-CN");
    await clickText(client, "取消");
    await waitFor(client, "window.__PHASE2_2E_MOCK__.importCancels===1&&!document.body.innerText.includes('正在导入音频')", "successful import cancellation");

    await client.send("Page.navigate", { url: `http://tauri.localhost/meeting-details?id=${meetingId}` });
    await waitForReady(client);
    await waitFor(client, "window.__PHASE2_2E_MOCK__&&document.body.innerText.includes('This is controlled audit transcript content.')", "meeting details fixture", 45000);
    await clickText(client, "增强");
    await waitFor(client, "document.body.innerText.includes('重新转写会议')&&document.body.innerText.includes('开始重新转写')", "Chinese retranscription dialog");
    const retranscribeZh = await snapshot(client, "06-retranscribe-ready-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, "document.body.innerText.includes('Retranscribe Meeting')&&document.body.innerText.includes('Start Retranscription')", "English retranscription dialog");
    const retranscribeEn = await snapshot(client, "07-retranscribe-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await clickText(client, "开始重新转写");
    await waitFor(client, "window.__PHASE2_2E_MOCK__.retranscriptionStarts===1", "retranscription start");
    await emit(client, "retranscription-progress", { meeting_id: meetingId, stage: "vad", progress_percentage: 22, message: backendSentinel });
    await waitFor(client, `document.body.innerText.includes('正在检测语音')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "localized retranscription progress");
    const retranscribeProgress = await snapshot(client, "08-retranscribe-progress-safe-zh-CN");
    await evaluate(client, "window.__PHASE2_2E_MOCK__.failNextRetranscriptionCancel=true");
    await clickText(client, "取消");
    await waitFor(client, "document.body.innerText.includes('无法取消重新转写')&&document.body.innerText.includes('正在重新转写')", "failed retranscription cancellation remains active");
    const retranscribeCancelFailure = await snapshot(client, "09-retranscribe-cancel-failure-stays-active-zh-CN");
    await clickText(client, "取消");
    await waitFor(client, "window.__PHASE2_2E_MOCK__.retranscriptionCancels===1&&!document.body.innerText.includes('正在重新转写')", "successful retranscription cancellation");

    await seedRecoveryFixture(client);
    await client.send("Page.navigate", { url: "http://tauri.localhost/" });
    await waitForReady(client);
    await waitFor(client, "window.__PHASE2_2E_MOCK__&&document.body.innerText.includes('恢复中断的会议')&&document.body.innerText.includes('崩溃恢复审计会议')", "Chinese recovery dialog", 45000);
    const recoveryZh = await snapshot(client, "10-recovery-preview-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, "document.body.innerText.includes('Recover Interrupted Meetings')&&document.body.innerText.includes('Crash')===false&&document.body.innerText.includes('崩溃恢复审计会议')", "English recovery shell with preserved business title");
    const recoveryEn = await snapshot(client, "11-recovery-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await clickText(client, "恢复");
    await waitFor(client, "window.__PHASE2_2E_MOCK__.recoverySaves===1&&document.body.innerText.includes('会议已成功恢复')", "successful recovery", 30000);
    const recoveredZh = await snapshot(client, "12-recovery-complete-zh-CN");
    const recoveryStored = await evaluate(client, `(async()=>{const db=await new Promise((resolve,reject)=>{const request=indexedDB.open('MeetilyRecoveryDB',1);request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});const value=await new Promise((resolve,reject)=>{const request=db.transaction('meetings','readonly').objectStore('meetings').get(${JSON.stringify(recoveryId)});request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error);});db.close();return value;})()`);

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(path.join(outputDirectory, "13-runtime-diagnostics.json"), `${JSON.stringify(runtimeDiagnostics, null, 2)}\n`);
    const snapshots = [dropOverlayZh, importEmptyZh, invalidFileZh, importConfiguredZh, importStateEn, importProgressZh, importCancelFailure, retranscribeZh, retranscribeEn, retranscribeProgress, retranscribeCancelFailure, recoveryZh, recoveryEn, recoveredZh];
    const checks = [
      { id: "P2E-RUN-DRAG-OVERLAY-ZH", pass: includesAll(dropOverlayZh.bodyText, ["拖放音频文件以导入", "支持的格式：", "MP4", "WAV", "WebM", "WMA"]), evidence: "00a-drag-overlay-zh-CN.json/png" },
      { id: "P2E-RUN-IMPORT-ZH", pass: includesAll(importConfiguredZh.bodyText, ["导入音频文件", "会议标题", "高级选项", "语言", "模型", "board-meeting.mp4"]), evidence: "01-02 snapshots" },
      { id: "P2E-RUN-INVALID-FILE-SAFE", pass: invalidFileZh.bodyText.includes("无法验证所选音频文件") && !invalidFileZh.bodyText.includes(backendSentinel), evidence: "01a-import-invalid-file-safe-zh-CN.json/png" },
      { id: "P2E-RUN-IMPORT-LOCALE-STATE", pass: importStateEn.htmlLang === "en" && importStateEn.formValues.some((item) => item.value === audioTitle) && includesAll(importStateEn.bodyText, ["Import Audio File", "Advanced Options", "Language", "Model"]), evidence: "03-import-locale-en-state-preserved.json/png" },
      { id: "P2E-RUN-IMPORT-PROGRESS-SAFE", pass: importProgressZh.mock?.importStarts === 1 && importProgressZh.bodyText.includes("正在转写语音") && !importProgressZh.bodyText.includes(backendSentinel), evidence: "04-import-progress-safe-zh-CN.json/png" },
      { id: "P2E-RUN-IMPORT-CANCEL-FAILURE", pass: importCancelFailure.bodyText.includes("无法取消导入") && importCancelFailure.bodyText.includes("正在导入音频") && importCancelFailure.mock?.importCancels === 0 && !importCancelFailure.bodyText.includes(backendSentinel), evidence: "05-import-cancel-failure-stays-active-zh-CN.json/png" },
      { id: "P2E-RUN-RETRANSCRIBE-ZH", pass: includesAll(retranscribeZh.bodyText, ["重新转写会议", "语言", "模型", "开始重新转写"]), evidence: "06-retranscribe-ready-zh-CN.json/png" },
      { id: "P2E-RUN-RETRANSCRIBE-LOCALE-STATE", pass: retranscribeEn.htmlLang === "en" && includesAll(retranscribeEn.bodyText, ["Retranscribe Meeting", "Language", "Model", "Start Retranscription"]), evidence: "07-retranscribe-locale-en-state-preserved.json/png" },
      { id: "P2E-RUN-RETRANSCRIBE-PROGRESS-SAFE", pass: retranscribeProgress.mock?.retranscriptionStarts === 1 && retranscribeProgress.bodyText.includes("正在检测语音") && !retranscribeProgress.bodyText.includes(backendSentinel), evidence: "08-retranscribe-progress-safe-zh-CN.json/png" },
      { id: "P2E-RUN-RETRANSCRIBE-CANCEL-FAILURE", pass: retranscribeCancelFailure.bodyText.includes("无法取消重新转写") && retranscribeCancelFailure.bodyText.includes("正在重新转写") && retranscribeCancelFailure.mock?.retranscriptionCancels === 0 && !retranscribeCancelFailure.bodyText.includes(backendSentinel), evidence: "09-retranscribe-cancel-failure-stays-active-zh-CN.json/png" },
      { id: "P2E-RUN-RECOVERY-ZH", pass: includesAll(recoveryZh.bodyText, ["恢复中断的会议", "崩溃恢复审计会议", "第一段恢复内容", "无音频", "恢复"]), evidence: "10-recovery-preview-zh-CN.json/png" },
      { id: "P2E-RUN-RECOVERY-LOCALE-STATE", pass: recoveryEn.htmlLang === "en" && includesAll(recoveryEn.bodyText, ["Recover Interrupted Meetings", "崩溃恢复审计会议", "No audio", "Recover"]), evidence: "11-recovery-locale-en-state-preserved.json/png" },
      { id: "P2E-RUN-RECOVERY-COMMIT", pass: recoveredZh.mock?.recoverySaves === 1 && recoveryStored?.savedToSQLite === true && recoveredZh.bodyText.includes("会议已成功恢复"), evidence: "12-recovery-complete-zh-CN.json/png + IndexedDB state" },
      { id: "P2E-RUN-NO-RAW-BACKEND-BUBBLING", pass: snapshots.every((item) => !item.bodyText.includes(backendSentinel)), evidence: "00a-12 snapshots" },
      { id: "P2E-RUN-A11Y-LAYOUT", pass: snapshots.every((item) => item.documentSize.width <= item.viewport.width + 1 && item.horizontalClipping.length === 0 && item.unnamedButtons.length === 0), evidence: "00a-12 snapshots" },
      { id: "P2E-RUN-NO-UNEXPECTED-RUNTIME-OR-CSP-ERRORS", pass: runtimeDiagnostics.exceptions === 0 && runtimeDiagnostics.unexpectedConsoleErrors === 0 && runtimeDiagnostics.logErrors === 0 && runtimeDiagnostics.csp === 0, evidence: "13-runtime-diagnostics.json", diagnostics: runtimeDiagnostics },
    ];
    const report = { phase: "15.5-stage-2-react-frontend-migration", batch: "2E", scope: "import-retranscribe-recovery", generatedAt: new Date().toISOString(), target: { title: target.title, url: target.url }, fixtures: { audioFile, audioTitle, meetingId, recoveryId, backendSentinel }, checks, summary: { passed: checks.filter((item) => item.pass).length, failed: checks.filter((item) => !item.pass).length, total: checks.length } };
    await fs.writeFile(path.join(outputDirectory, "runtime-2E-report.json"), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed > 0) process.exitCode = 1;
  } finally { client.close(); }
}

await main();
