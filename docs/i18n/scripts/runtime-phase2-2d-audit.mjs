#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9560);
const outputDirectory = path.resolve(process.argv[3] || "docs/i18n/audit/phase-2-react/2D/runtime-final");
const backendSentinel = "PHASE2_2D_RAW_BACKEND_ERROR_SENTINEL";
const initialMic = "Audit Microphone (input)";
const alternateMic = "Backup Microphone (input)";
const systemDevice = "Audit Speakers (output)";
const saveFolder = "D:\\2D Audit Recordings";
const endpoint = "http://127.0.0.1:8800/v1";
const model = "audit-summary-model";

class CdpClient {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
  }
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
  for (let attempt = 0; attempt < 120; attempt += 1) {
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
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  return result.result.value;
}

async function waitFor(client, expression, description, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(client, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${description}`);
}

async function waitForReady(client) {
  await waitFor(client, "Boolean(document.body&&document.querySelector('[data-i18n-ready=\"true\"]'))", "the i18n provider");
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

async function clickText(client, text, contains = false) {
  const match = contains ? "value.includes(wanted)||label.includes(wanted)" : "value===wanted||label===wanted";
  return clickElement(client, `(()=>{const wanted=${JSON.stringify(text)};return [...document.querySelectorAll('button,a,[role=\"button\"],[role=\"tab\"]')].find((item)=>{const value=(item.textContent||'').trim();const label=item.getAttribute('aria-label')||'';const rect=item.getBoundingClientRect();return (${match})&&!item.disabled&&item.getAttribute('aria-disabled')!=='true'&&rect.width>0&&rect.height>0;})})()`, text);
}

async function chooseSelectOption(client, triggerSelector, optionText) {
  await clickElement(client, `document.querySelector(${JSON.stringify(triggerSelector)})`, triggerSelector);
  await waitFor(client, `Boolean([...document.querySelectorAll('[role=\"option\"]')].find((item)=>(item.textContent||'').includes(${JSON.stringify(optionText)})))`, `${optionText} option`);
  await clickElement(client, `[...document.querySelectorAll('[role="option"]')].find((item)=>(item.textContent||'').includes(${JSON.stringify(optionText)}))`, optionText);
}

async function invoke(client, command, args = {}) {
  return evaluate(client, `(async()=>{try{return{resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return{resolved:false,error:String(error)}}})()`);
}

async function snapshot(client, name) {
  await new Promise((resolve) => setTimeout(resolve, 500));
  const state = await evaluate(client, `(()=>{const all=[...document.querySelectorAll('button,a,input,[role="button"],[role="tab"],[role="radio"]')].map((element)=>{const rect=element.getBoundingClientRect();const intersects=rect.right>0&&rect.bottom>0&&rect.left<innerWidth&&rect.top<innerHeight;const type=element.getAttribute('type');const rawValue=element.value??null;return{tag:element.tagName.toLowerCase(),type,text:(element.textContent||'').trim(),value:type==='password'?(rawValue?'[redacted]':''):rawValue,ariaLabel:element.getAttribute('aria-label'),title:element.getAttribute('title'),descendantAlt:element.querySelector('[alt]')?.getAttribute('alt')||null,placeholder:element.getAttribute('placeholder'),checked:Boolean(element.checked),disabled:Boolean(element.disabled)||element.getAttribute('aria-disabled')==='true',intersects,rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};});const controls=all.filter((item)=>item.intersects);const mock=window.__PHASE2_2D_MOCK__;return{url:location.href,htmlLang:document.documentElement.lang,bodyText:document.body.innerText,viewport:{width:innerWidth,height:innerHeight},documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},controls,formValues:all.filter((item)=>item.tag==='input').map((item)=>({type:item.type,value:item.value,placeholder:item.placeholder})),horizontalClipping:controls.filter((item)=>item.rect.left<0||item.rect.right>innerWidth+1),unnamedButtons:controls.filter((item)=>item.tag==='button'&&!item.text&&!item.ariaLabel&&!item.title&&!item.descendantAlt),mock:mock?{recordingPreferences:mock.recordingPreferences,currentBackend:mock.currentBackend,modelConfig:mock.modelConfig,customConfig:{...mock.customConfig,apiKey:mock.customConfig.apiKey?'[redacted]':null},savedModelCount:mock.savedModelCount,failedDeviceSaves:mock.failedDeviceSaves,connectionTests:mock.connectionTests,failNextDeviceSave:mock.failNextDeviceSave}:null};})()`);
  const image = await client.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(state, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, "base64")),
  ]);
  return state;
}

function mockBootstrapSource() {
  const initialState = {
    recordingPreferences: { save_folder: saveFolder, auto_save: true, file_format: "mp4", preferred_mic_device: initialMic, preferred_system_device: systemDevice },
    modelConfig: { provider: "custom-openai", model, whisperModel: "large-v3", apiKey: null, ollamaEndpoint: null },
    customConfig: { endpoint, apiKey: "phase2-2d-secret", model, maxTokens: 4096, temperature: 0.7, topP: 0.9 },
    transcriptConfig: { provider: "parakeet", model: "parakeet-tdt-0.6b-v3-int8", apiKey: null },
    currentBackend: "coreaudio",
    savedModelCount: 0,
    failedDeviceSaves: 0,
    connectionTests: 0,
    failNextDeviceSave: false,
  };
  const models = [
    { name: "parakeet-tdt-0.6b-v3-int8", path: "audit/parakeet-v3-int8", size_mb: 670, accuracy: "High", speed: "Ultra Fast", status: "Available", quantization: "Int8" },
    { name: "parakeet-tdt-0.6b-v2-int8", path: "audit/parakeet-v2-int8", size_mb: 661, accuracy: "High", speed: "Very Fast", status: "Missing", quantization: "Int8" },
    { name: "parakeet-tdt-0.6b-v3-fp32", path: "audit/parakeet-v3-fp32", size_mb: 2554, accuracy: "High", speed: "Fast", status: "Missing", quantization: "FP32" },
  ];
  return `(()=>{if(window.__PHASE2_2D_MOCK_INSTALLED__)return;window.__PHASE2_2D_MOCK_INSTALLED__=true;const nativeFetch=window.fetch.bind(window);const state=window.__PHASE2_2D_MOCK__=${JSON.stringify(initialState)};const parakeetModels=${JSON.stringify(models)};const notificationSettings={recording_notifications:true,time_based_reminders:true,meeting_reminders:true,respect_do_not_disturb:true,notification_sound:true,system_permission_granted:true,consent_given:true,manual_dnd_mode:false,notification_preferences:{show_recording_started:true,show_recording_stopped:true,show_post_processing_completed:true,show_post_processing_failed:true}};const ok=(value)=>new Response(JSON.stringify(value??null),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});const fail=(value)=>new Response(JSON.stringify(value),{status:500,headers:{'Content-Type':'application/json','Tauri-Response':'error'}});window.fetch=async(input,init={})=>{const raw=typeof input==='string'?input:input?.url;let url;try{url=new URL(raw,location.href);}catch{return nativeFetch(input,init);}if(url.hostname!=='ipc.localhost')return nativeFetch(input,init);const command=decodeURIComponent(url.pathname.slice(1));let args={};if(typeof init?.body==='string'){try{args=JSON.parse(init.body)||{};}catch{}}if(command==='api_get_model_config')return ok(state.modelConfig);if(command==='api_get_custom_openai_config')return ok(state.customConfig);if(command==='api_get_api_key'||command==='api_get_transcript_api_key')return ok(null);if(command==='api_get_auto_generate_setting')return ok(true);if(command==='api_get_transcript_config')return ok(state.transcriptConfig);if(command==='api_save_transcript_config'){state.transcriptConfig={provider:args.provider,model:args.model,apiKey:args.apiKey??null};return ok(null);}if(command==='api_save_custom_openai_config'){state.customConfig={endpoint:args.endpoint,apiKey:args.apiKey??null,model:args.model,maxTokens:args.maxTokens??null,temperature:args.temperature??null,topP:args.topP??null};return ok({status:'ok',message:'saved'});}if(command==='api_test_custom_openai_connection'){state.connectionTests+=1;return fail(${JSON.stringify(backendSentinel)});}if(command==='api_save_model_config'){state.modelConfig={provider:args.provider,model:args.model,whisperModel:args.whisperModel,apiKey:args.apiKey??null,ollamaEndpoint:args.ollamaEndpoint??null};state.savedModelCount+=1;return ok(null);}if(command==='get_ollama_models')return ok([{id:'audit',name:'audit-ollama',size:'1 GB',modified:'today'}]);if(command==='builtin_ai_list_models')return ok([]);if(command==='get_recording_preferences')return ok(state.recordingPreferences);if(command==='set_recording_preferences'){if(state.failNextDeviceSave){state.failNextDeviceSave=false;state.failedDeviceSaves+=1;return fail(${JSON.stringify(backendSentinel)});}state.recordingPreferences={...args.preferences};return ok(null);}if(command==='get_default_recordings_folder_path')return ok(${JSON.stringify(saveFolder)});if(command==='get_database_directory')return ok('D:\\\\2D Audit Data');if(command==='whisper_get_models_directory')return ok('D:\\\\2D Audit Models');if(command==='get_notification_settings')return ok(notificationSettings);if(command==='set_notification_settings'||command==='set_language_preference')return ok(null);if(command==='get_audio_devices')return ok([{name:'Audit Microphone',device_type:'Input'},{name:'Backup Microphone',device_type:'Input'},{name:'Audit Speakers',device_type:'Output'}]);if(command==='get_audio_backend_info')return ok([{id:'screencapturekit',name:'ScreenCaptureKit',description:'RAW_BACKEND_DESCRIPTION'},{id:'coreaudio',name:'Core Audio',description:'RAW_BACKEND_DESCRIPTION'}]);if(command==='get_current_audio_backend')return ok(state.currentBackend);if(command==='set_audio_backend'){state.currentBackend=args.backend;return ok(null);}if(command==='start_audio_level_monitoring'||command==='stop_audio_level_monitoring')return ok(null);if(command==='parakeet_init')return ok(null);if(command==='parakeet_get_available_models')return ok(parakeetModels);if(command==='parakeet_has_available_models')return ok(true);if(command==='api_list_templates_v2')return ok({templates:[],diagnostics:[],deletedTemplates:[],defaultTemplateId:'standard_meeting'});if(command==='api_get_templates_directory')return ok('D:\\\\2D Audit Templates');if(command.startsWith('open_'))return ok(null);return nativeFetch(input,init);};})();`;
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
    await evaluate(client, "localStorage.setItem('meetily.uiLocale','zh-CN')");
    const onboarding = await invoke(client, "save_onboarding_status_cmd", { status: { version: "1.0", completed: true, current_step: 4, model_status: { parakeet: "downloaded", summary: "downloaded", selected_summary_model: model }, last_updated: new Date().toISOString() } });
    if (!onboarding.resolved) throw new Error(`Unable to seed onboarding: ${onboarding.error}`);
    await client.send("Page.addScriptToEvaluateOnNewDocument", { source: mockBootstrapSource() });
    await client.send("Page.navigate", { url: "http://tauri.localhost/settings" });
    await waitForReady(client);
    await waitFor(client, `window.__PHASE2_2D_MOCK__&&document.documentElement.lang==='zh-CN'&&document.body.innerText.includes('数据存储位置')&&document.body.innerText.includes(${JSON.stringify(saveFolder)})`, "Chinese general settings", 45000);
    const generalZh = await snapshot(client, "01-general-zh-CN");

    await clickText(client, "录音");
    await waitFor(client, `document.body.innerText.includes('录音设置')&&document.body.innerText.includes('Audit Microphone')&&document.body.innerText.includes('Core Audio')`, "recording and device settings");
    const recordingZh = await snapshot(client, "02-recording-devices-zh-CN");

    await evaluate(client, "window.__PHASE2_2D_MOCK__.failNextDeviceSave=true");
    await chooseSelectOption(client, "#mic-selection", "Backup Microphone");
    await waitFor(client, `window.__PHASE2_2D_MOCK__.failedDeviceSaves===1&&document.body.innerText.includes('无法保存设备偏好设置')`, "safe device save failure");
    await waitFor(client, `document.querySelector('#mic-selection')?.textContent.includes('Audit Microphone')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "failed device choice rollback");
    const deviceFailure = await snapshot(client, "03-device-save-failure-rollback-zh-CN");

    await chooseSelectOption(client, "#mic-selection", "Backup Microphone");
    await waitFor(client, `window.__PHASE2_2D_MOCK__.recordingPreferences.preferred_mic_device===${JSON.stringify(alternateMic)}&&document.body.innerText.includes('设备偏好设置已保存')`, "device preference persistence");
    const deviceSaved = await snapshot(client, "04-device-save-success-zh-CN");

    await clickText(client, "转写");
    await waitFor(client, "document.body.innerText.includes('转写模型')&&document.body.innerText.includes('闪电版')&&document.body.innerText.includes('推荐')", "localized transcription models");
    const transcriptionZh = await snapshot(client, "05-transcription-models-zh-CN");
    await waitFor(client, "!document.body.innerText.includes('设备偏好设置已保存')&&!document.body.innerText.includes('无法保存设备偏好设置')", "device toasts to close before summary-model evidence", 12000);

    await clickText(client, "摘要");
    await waitFor(client, `document.body.innerText.includes('摘要模型配置')&&document.querySelector('#custom-endpoint')?.value===${JSON.stringify(endpoint)}&&document.querySelector('#custom-model')?.value===${JSON.stringify(model)}`, "custom summary model settings", 45000);
    await evaluate(client, "document.querySelector('#custom-endpoint')?.scrollIntoView({block:'center'})");
    const summaryZh = await snapshot(client, "06-summary-custom-model-zh-CN");
    await clickText(client, "测试连接");
    await waitFor(client, `window.__PHASE2_2D_MOCK__.connectionTests===1&&document.body.innerText.includes('无法使用当前配置建立连接')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "safe custom model connection error");
    const connectionFailure = await snapshot(client, "07-model-safe-error-zh-CN");
    await clickText(client, "保存");
    await waitFor(client, "window.__PHASE2_2D_MOCK__.savedModelCount===1&&document.body.innerText.includes('模型设置已保存')", "summary model persistence");
    const modelSaved = await snapshot(client, "08-model-save-zh-CN");
    await waitFor(client, "!document.body.innerText.includes('模型设置已保存')&&!document.body.innerText.includes('无法使用当前配置建立连接')", "model toasts to close before locale switching", 12000);

    await setLocaleLive(client, "en");
    await waitFor(client, `document.body.innerText.includes('Summary model configuration')&&document.querySelector('#custom-endpoint')?.value===${JSON.stringify(endpoint)}&&document.querySelector('#custom-model')?.value===${JSON.stringify(model)}&&window.__PHASE2_2D_MOCK__.recordingPreferences.preferred_mic_device===${JSON.stringify(alternateMic)}`, "English locale with preserved settings state");
    const statePreservedEn = await snapshot(client, "09-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await waitFor(client, "document.body.innerText.includes('摘要模型配置')", "Chinese locale restoration");
    const restoredZh = await snapshot(client, "10-locale-zh-CN-restored");

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(path.join(outputDirectory, "11-runtime-diagnostics.json"), `${JSON.stringify(runtimeDiagnostics, null, 2)}\n`);
    const snapshots = [generalZh, recordingZh, deviceFailure, deviceSaved, transcriptionZh, summaryZh, connectionFailure, modelSaved, statePreservedEn, restoredZh];
    const checks = [
      { id: "P2D-RUN-GENERAL-ZH", pass: generalZh.htmlLang === "zh-CN" && includesAll(generalZh.bodyText, ["设置", "常规", "显示语言", "数据存储位置", saveFolder]), evidence: "01-general-zh-CN.json/png" },
      { id: "P2D-RUN-RECORDING-DEVICE-ZH", pass: includesAll(recordingZh.bodyText, ["录音设置", "默认音频设备", "麦克风", "系统音频", "系统音频后端", "Audit Microphone", "Audit Speakers"]) && !recordingZh.bodyText.includes("RAW_BACKEND_DESCRIPTION"), evidence: "02-recording-devices-zh-CN.json/png" },
      { id: "P2D-RUN-DEVICE-FAILURE-ROLLBACK", pass: deviceFailure.mock?.failedDeviceSaves === 1 && deviceFailure.mock?.recordingPreferences.preferred_mic_device === initialMic && deviceFailure.controls.some((item) => item.text.includes("Audit Microphone")) && deviceFailure.bodyText.includes("无法保存设备偏好设置") && !deviceFailure.bodyText.includes(backendSentinel), evidence: "03-device-save-failure-rollback-zh-CN.json/png" },
      { id: "P2D-RUN-DEVICE-SAVE", pass: deviceSaved.mock?.recordingPreferences.preferred_mic_device === alternateMic && deviceSaved.bodyText.includes("设备偏好设置已保存"), evidence: "04-device-save-success-zh-CN.json/png" },
      { id: "P2D-RUN-TRANSCRIPTION-MODELS-ZH", pass: includesAll(transcriptionZh.bodyText, ["转写模型", "Parakeet", "闪电版", "精简版", "精确版", "推荐"]), evidence: "05-transcription-models-zh-CN.json/png" },
      { id: "P2D-RUN-SUMMARY-MODEL-STATE", pass: includesAll(summaryZh.bodyText, ["摘要模型配置", "自定义服务器", "端点 URL", "模型名称"]) && summaryZh.formValues.some((item) => item.value === endpoint) && summaryZh.formValues.some((item) => item.value === model), evidence: "06-summary-custom-model-zh-CN.json/png" },
      { id: "P2D-RUN-SAFE-MODEL-ERROR", pass: connectionFailure.mock?.connectionTests === 1 && connectionFailure.bodyText.includes("无法使用当前配置建立连接") && !connectionFailure.bodyText.includes(backendSentinel), evidence: "07-model-safe-error-zh-CN.json/png" },
      { id: "P2D-RUN-MODEL-SAVE", pass: modelSaved.mock?.savedModelCount === 1 && modelSaved.mock?.modelConfig.provider === "custom-openai" && modelSaved.mock?.modelConfig.model === model && modelSaved.bodyText.includes("模型设置已保存"), evidence: "08-model-save-zh-CN.json/png" },
      { id: "P2D-RUN-LOCALE-STATE-PRESERVATION", pass: statePreservedEn.htmlLang === "en" && includesAll(statePreservedEn.bodyText, ["Settings", "Summary", "Summary model configuration"]) && statePreservedEn.formValues.some((item) => item.value === endpoint) && statePreservedEn.formValues.some((item) => item.value === model) && statePreservedEn.mock?.recordingPreferences.preferred_mic_device === alternateMic && statePreservedEn.mock?.customConfig.endpoint === endpoint && restoredZh.htmlLang === "zh-CN", evidence: "09-10 snapshots" },
      { id: "P2D-RUN-SECRETS-NOT-EXPOSED", pass: snapshots.every((item) => !item.bodyText.includes("phase2-2d-secret")), evidence: "01-10 snapshots" },
      { id: "P2D-RUN-A11Y-LAYOUT", pass: snapshots.every((item) => item.documentSize.width <= item.viewport.width + 1 && item.horizontalClipping.length === 0 && item.unnamedButtons.length === 0), evidence: "01-10 snapshots" },
      { id: "P2D-RUN-NO-UNEXPECTED-RUNTIME-OR-CSP-ERRORS", pass: runtimeDiagnostics.exceptions === 0 && runtimeDiagnostics.unexpectedConsoleErrors === 0 && runtimeDiagnostics.logErrors === 0 && runtimeDiagnostics.csp === 0, evidence: "11-runtime-diagnostics.json", diagnostics: runtimeDiagnostics },
    ];
    const report = { phase: "15.5-stage-2-react-frontend-migration", batch: "2D", scope: "settings-models-audio-devices", generatedAt: new Date().toISOString(), target: { title: target.title, url: target.url }, fixtures: { initialMic, alternateMic, systemDevice, saveFolder, endpoint, model, backendSentinel }, checks, summary: { passed: checks.filter((item) => item.pass).length, failed: checks.filter((item) => !item.pass).length, total: checks.length } };
    await fs.writeFile(path.join(outputDirectory, "runtime-2D-report.json"), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed > 0) process.exitCode = 1;
  } finally { client.close(); }
}

await main();
