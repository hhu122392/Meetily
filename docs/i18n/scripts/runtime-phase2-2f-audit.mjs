#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9563);
const outputDirectory = path.resolve(process.argv[3] || "docs/i18n/audit/phase-2-react/2F/runtime-final");
const backendSentinel = "PHASE2_2F_RAW_BACKEND_ERROR_SENTINEL";

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
  const debug = await evaluate(client, `({url:location.href,lang:document.documentElement.lang,text:document.body?.innerText?.slice(0,4000),mock:window.__PHASE2_2F_MOCK__||null,ready:document.readyState})`).catch((error) => ({ evaluationError: String(error) }));
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
  const found = await evaluate(client, `(()=>{const element=${expression};if(!element)return false;element.scrollIntoView({block:'center',inline:'center',behavior:'instant'});return true;})()`);
  if (!found) throw new Error(`Clickable control not found: ${description}`);
  // Nested About/analytics panels have their own scroll containers. Wait for
  // layout and focus restoration to settle, then compute the physical point;
  // calculating it in the same task can target the pre-scroll coordinates.
  await new Promise((resolve) => setTimeout(resolve, 250));
  const point = await evaluate(client, `(()=>{const element=${expression};if(!element)return null;const rect=element.getBoundingClientRect();return{x:rect.left+rect.width/2,y:rect.top+rect.height/2};})()`);
  if (!point) throw new Error(`Clickable control not found: ${description}`);
  await client.send("Input.dispatchMouseEvent", { type: "mousePressed", x: point.x, y: point.y, button: "left", clickCount: 1 });
  await client.send("Input.dispatchMouseEvent", { type: "mouseReleased", x: point.x, y: point.y, button: "left", clickCount: 1 });
}

async function clickText(client, wanted, contains = false) {
  const match = contains ? "value.includes(wanted)||label.includes(wanted)||title.includes(wanted)" : "value===wanted||label===wanted||title===wanted";
  return clickElement(client, `(()=>{const wanted=${JSON.stringify(wanted)};return [...document.querySelectorAll('button,a,[role=\"button\"],[role=\"tab\"],[role=\"switch\"]')].find((item)=>{const value=(item.textContent||'').trim();const label=item.getAttribute('aria-label')||'';const title=item.getAttribute('title')||'';const rect=item.getBoundingClientRect();return (${match})&&!item.disabled&&item.getAttribute('aria-disabled')!=='true'&&rect.width>0&&rect.height>0;})})()`, wanted);
}

async function pressEscape(client) {
  await client.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
  await client.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27, nativeVirtualKeyCode: 27 });
}

async function invoke(client, command, args = {}) {
  return evaluate(client, `(async()=>{try{return{resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return{resolved:false,error:String(error)}}})()`);
}

async function snapshot(client, name) {
  await new Promise((resolve) => setTimeout(resolve, 400));
  const state = await evaluate(client, `(()=>{const all=[...document.querySelectorAll('button,a,input,[role="button"],[role="tab"],[role="switch"],[role="dialog"]')].map((element)=>{const rect=element.getBoundingClientRect();const intersects=rect.right>0&&rect.bottom>0&&rect.left<innerWidth&&rect.top<innerHeight;return{tag:element.tagName.toLowerCase(),role:element.getAttribute('role'),text:(element.textContent||'').trim(),ariaLabel:element.getAttribute('aria-label'),ariaChecked:element.getAttribute('aria-checked'),title:element.getAttribute('title'),disabled:Boolean(element.disabled)||element.getAttribute('aria-disabled')==='true',intersects,rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};});const controls=all.filter((item)=>item.intersects);const mock=window.__PHASE2_2F_MOCK__;return{url:location.href,htmlLang:document.documentElement.lang,bodyText:document.body.innerText,viewport:{width:innerWidth,height:innerHeight},documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},controls,horizontalClipping:controls.filter((item)=>item.rect.left<0||item.rect.right>innerWidth+1),unnamedButtons:controls.filter((item)=>item.tag==='button'&&!item.text&&!item.ariaLabel&&!item.title),mock:mock?{updateMode:mock.updateMode,updateChecks:mock.updateChecks,analyticsOptedIn:mock.store.analyticsOptedIn,migration:mock.store.analyticsDefaultOffMigrationV1,userId:mock.store.user_id,analyticsInitCalls:mock.analyticsInitCalls,analyticsDisableCalls:mock.analyticsDisableCalls,failNextAnalyticsInit:mock.failNextAnalyticsInit}:null,betaFeatures:JSON.parse(localStorage.getItem('betaFeatures')||'null')};})()`);
  const image = await client.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(state, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, "base64")),
  ]);
  return state;
}

function mockBootstrapSource() {
  const initial = {
    updateMode: "latest",
    updateChecks: 0,
    analyticsInitCalls: 0,
    analyticsDisableCalls: 0,
    failNextAnalyticsInit: false,
    store: {
      analyticsOptedIn: false,
      analyticsDefaultOffMigrationV1: true,
      user_id: "audit-user-id-2f",
      is_first_launch: false,
    },
  };
  return `(()=>{if(window.__PHASE2_2F_MOCK_INSTALLED__)return;window.__PHASE2_2F_MOCK_INSTALLED__=true;const nativeFetch=window.fetch.bind(window);const state=window.__PHASE2_2F_MOCK__=${JSON.stringify(initial)};const ok=(value)=>new Response(JSON.stringify(value??null),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});const fail=(value)=>new Response(JSON.stringify(value),{status:500,headers:{'Content-Type':'application/json','Tauri-Response':'error'}});const update=()=>({rid:2601,currentVersion:'0.4.0',version:'9.8.7',date:'2026-08-23T00:00:00.000Z',body:'受控审计版本说明',rawJson:{version:'9.8.7'}});window.fetch=async(input,init={})=>{const raw=typeof input==='string'?input:input?.url;let url;try{url=new URL(raw,location.href);}catch{return nativeFetch(input,init);}if(url.hostname!=='ipc.localhost')return nativeFetch(input,init);const command=decodeURIComponent(url.pathname.slice(1));let args={};if(typeof init?.body==='string'){try{args=JSON.parse(init.body)||{};}catch{}}if(command==='plugin:app|version')return ok('0.4.0');if(command==='plugin:store|load'||command==='plugin:store|get_store')return ok(2501);if(command==='plugin:store|has')return ok(Object.prototype.hasOwnProperty.call(state.store,args.key));if(command==='plugin:store|get'){const exists=Object.prototype.hasOwnProperty.call(state.store,args.key);return ok([exists?state.store[args.key]:null,exists]);}if(command==='plugin:store|set'){state.store[args.key]=args.value;return ok(null);}if(command==='plugin:store|save'||command==='plugin:store|reload')return ok(null);if(command==='plugin:store|delete'){const existed=Object.prototype.hasOwnProperty.call(state.store,args.key);delete state.store[args.key];return ok(existed);}if(command==='plugin:updater|check'){state.updateChecks+=1;if(state.updateMode==='fail')return fail(${JSON.stringify(backendSentinel)});if(state.updateMode==='latest')return ok(null);if(state.updateMode==='prepareFail'){state.updateMode='fail';return ok(update());}return ok(update());}if(command==='init_analytics'){state.analyticsInitCalls+=1;if(state.failNextAnalyticsInit){state.failNextAnalyticsInit=false;return fail(${JSON.stringify(backendSentinel)});}return ok(null);}if(command==='disable_analytics'){state.analyticsDisableCalls+=1;return ok(null);}if(command==='start_analytics_session')return ok('audit-session-2f');if(command==='is_analytics_enabled'||command==='is_analytics_session_active')return ok(state.store.analyticsOptedIn===true);if(command==='get_device_info')return ok({platform:'windows',os_version:'audit',architecture:'x86_64'});if(command==='identify_user'||command==='track_event'||command==='track_analytics_enabled'||command==='track_analytics_disabled'||command==='track_analytics_transparency_viewed'||command==='track_daily_active_user'||command==='track_user_first_launch'||command==='end_analytics_session')return ok(null);if(command==='open_external_url')return ok(null);if(command==='parakeet_init')return ok(null);if(command==='parakeet_has_available_models')return ok(true);if(command==='parakeet_get_available_models')return ok([]);if(command==='whisper_get_available_models')return ok([{name:'large-v3-turbo',size_mb:1550,status:'Available'}]);if(command==='is_recording')return ok(false);if(command==='get_recording_state')return ok({is_recording:false,is_paused:false,is_active:false,recording_duration:null,active_duration:null});if(command==='api_get_meetings'||command==='api_search_transcripts'||command==='get_audio_devices'||command==='get_ollama_models')return ok([]);if(command==='api_get_model_config')return ok({provider:'ollama',model:'audit-model',whisperModel:'large-v3',apiKey:null,ollamaEndpoint:null});if(command==='api_get_api_key'||command==='api_get_transcript_api_key')return ok(null);if(command==='api_get_auto_generate_setting')return ok(false);if(command==='api_get_transcript_config')return ok({provider:'localWhisper',model:'large-v3-turbo',apiKey:null});if(command==='get_recording_preferences')return ok({save_folder:'D:\\\\2F Audit',auto_save:true,file_format:'mp4',preferred_mic_device:null,preferred_system_device:null});if(command==='get_notification_settings')return ok({recording_notifications:false,time_based_reminders:false,meeting_reminders:false,respect_do_not_disturb:true,notification_sound:false,system_permission_granted:true,consent_given:true,manual_dnd_mode:false,notification_preferences:{show_recording_started:false,show_recording_stopped:false,show_recording_paused:false,show_recording_resumed:false,show_transcription_complete:false,show_meeting_reminders:false,show_system_errors:false,meeting_reminder_minutes:[]}});if(command==='set_notification_settings')return ok(null);if(command==='get_database_directory')return ok('D:\\\\2F Audit\\\\database');if(command==='whisper_get_models_directory')return ok('D:\\\\2F Audit\\\\models');if(command==='get_default_recordings_folder_path')return ok('D:\\\\2F Audit\\\\recordings');if(command==='api_list_templates_v2')return ok({templates:[],diagnostics:[],deletedTemplates:[],defaultTemplateId:'standard_meeting'});return nativeFetch(input,init);};})();`;
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
    await waitFor(client, "window.__PHASE2_2F_MOCK__&&document.documentElement.lang==='zh-CN'&&Boolean(document.querySelector('button[aria-label=\"关于 Meetily\"]'))", "Chinese home and mock", 45000);

    await clickText(client, "关于 Meetily");
    await waitFor(client, "document.body.innerText.includes('使用情况分析')&&document.body.innerText.includes('除非你主动启用，否则保持关闭')", "Chinese About and default-off analytics");
    const aboutZh = await snapshot(client, "01-about-analytics-default-off-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, "document.body.innerText.includes('Usage Analytics')&&document.body.innerText.includes('Off unless you choose to enable it')", "English About without reload");
    const aboutEn = await snapshot(client, "02-about-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");

    await evaluate(client, "window.__PHASE2_2F_MOCK__.updateMode='latest'");
    await clickText(client, "检查更新");
    await waitFor(client, "document.body.innerText.includes('当前已是最新版本')", "localized latest-version toast");
    const updateLatest = await snapshot(client, "03-update-latest-zh-CN");
    await evaluate(client, "window.__PHASE2_2F_MOCK__.updateMode='fail'");
    await clickText(client, "检查更新");
    await waitFor(client, `document.body.innerText.includes('无法检查更新，请重试。')&&!document.body.innerText.includes(${JSON.stringify(backendSentinel)})`, "safe update-check failure");
    const updateFailure = await snapshot(client, "04-update-check-failure-safe-zh-CN");
    await waitFor(client, "!document.body.innerText.includes('无法检查更新，请重试。')", "update failure toast dismissal", 15000);
    await evaluate(client, "window.__PHASE2_2F_MOCK__.updateMode='available'");
    await clickText(client, "检查更新");
    await waitFor(client, "document.body.innerText.includes('发现新版本（9.8.7）')&&document.body.innerText.includes('2026年8月23日')", "Chinese available-update dialog");
    const updateZh = await snapshot(client, "05-update-available-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, "document.body.innerText.includes('A new version (9.8.7) is available')&&document.body.innerText.includes('Aug 23, 2026')", "English available-update dialog");
    const updateEn = await snapshot(client, "06-update-locale-en-format-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await clickText(client, "稍后");

    await evaluate(client, "window.__PHASE2_2F_MOCK__.failNextAnalyticsInit=true");
    await clickText(client, "启用使用情况分析");
    await waitFor(client, "document.body.innerText.includes('无法更新分析偏好，已恢复之前的设置。')&&document.querySelector('[role=\"switch\"][aria-label=\"启用使用情况分析\"]')?.getAttribute('aria-checked')==='false'&&window.__PHASE2_2F_MOCK__.store.analyticsOptedIn===false", "analytics failure rollback in UI and store");
    const analyticsRollback = await snapshot(client, "07-analytics-enable-failure-rollback-safe-zh-CN");
    await waitFor(client, "!document.body.innerText.includes('无法更新分析偏好，已恢复之前的设置。')", "analytics rollback toast dismissal", 15000);
    await clickText(client, "启用使用情况分析");
    await waitFor(client, "document.querySelector('[role=\"switch\"][aria-label=\"启用使用情况分析\"]')?.getAttribute('aria-checked')==='true'&&document.body.innerText.includes('audit-user-id-2f')&&window.__PHASE2_2F_MOCK__.store.analyticsOptedIn===true", "successful explicit analytics opt-in");
    const analyticsEnabled = await snapshot(client, "08-analytics-explicit-opt-in-zh-CN");
    await clickText(client, "启用使用情况分析");
    await waitFor(client, "document.querySelector('[role=\"dialog\"][aria-labelledby=\"analytics-transparency-title\"]')&&document.body.innerText.includes('我们不会收集：')", "Chinese analytics transparency dialog");
    const transparencyZh = await snapshot(client, "09-analytics-transparency-zh-CN");
    await setLocaleLive(client, "en");
    await waitFor(client, `document.body.innerText.includes('What Analytics Collects')&&document.body.innerText.includes("What We DON'T Collect:")`, "English analytics transparency dialog");
    const transparencyEn = await snapshot(client, "10-analytics-transparency-en-state-preserved");
    await setLocaleLive(client, "zh-CN");
    await clickText(client, "确认停用分析");
    await waitFor(client, "!document.querySelector('[role=\"dialog\"][aria-labelledby=\"analytics-transparency-title\"]')&&document.querySelector('[role=\"switch\"][aria-label=\"启用使用情况分析\"]')?.getAttribute('aria-checked')==='false'&&window.__PHASE2_2F_MOCK__.store.analyticsOptedIn===false", "confirmed analytics opt-out");
    const analyticsDisabled = await snapshot(client, "11-analytics-confirmed-opt-out-zh-CN");

    await pressEscape(client);
    await waitFor(client, "!document.body.innerText.includes('Meetily 的独特之处')", "About dialog close");
    await clickText(client, "设置");
    await waitFor(client, "location.pathname==='/settings'&&document.body.innerText.includes('测试功能')", "settings page");
    await clickText(client, "测试功能");
    await waitFor(client, "document.body.innerText.includes('Beta 功能')&&document.body.innerText.includes('导入音频与重新转写')", "Chinese Beta settings");
    const betaZh = await snapshot(client, "12-beta-enabled-zh-CN");
    await clickText(client, "导入音频与重新转写");
    await waitFor(client, "JSON.parse(localStorage.getItem('betaFeatures')).importAndRetranscribe===false&&document.querySelector('[role=\"switch\"][aria-label=\"导入音频与重新转写\"]')?.getAttribute('aria-checked')==='false'", "stable Beta toggle persistence");
    await setLocaleLive(client, "en");
    await waitFor(client, "document.body.innerText.includes('Beta Features')&&document.body.innerText.includes('Import Audio & Retranscribe')&&JSON.parse(localStorage.getItem('betaFeatures')).importAndRetranscribe===false", "English Beta state preservation");
    const betaEn = await snapshot(client, "13-beta-locale-en-state-preserved");

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(path.join(outputDirectory, "14-runtime-diagnostics.json"), `${JSON.stringify(runtimeDiagnostics, null, 2)}\n`);
    const snapshots = [aboutZh, aboutEn, updateLatest, updateFailure, updateZh, updateEn, analyticsRollback, analyticsEnabled, transparencyZh, transparencyEn, analyticsDisabled, betaZh, betaEn];
    const checks = [
      { id: "P2F-RUN-ABOUT-ZH", pass: includesAll(aboutZh.bodyText, ["Meetily 的独特之处", "隐私优先", "使用情况分析", "查看隐私政策"]), evidence: "01-about-analytics-default-off-zh-CN.json/png" },
      { id: "P2F-RUN-ABOUT-LOCALE", pass: aboutEn.htmlLang === "en" && includesAll(aboutEn.bodyText, ["What makes Meetily different", "Privacy-first", "Usage Analytics"]), evidence: "02-about-locale-en-state-preserved.json/png" },
      { id: "P2F-RUN-ANALYTICS-DEFAULT-OFF", pass: aboutZh.mock?.analyticsOptedIn === false && !aboutZh.bodyText.includes("audit-user-id-2f"), evidence: "01-about-analytics-default-off-zh-CN.json/png" },
      { id: "P2F-RUN-UPDATE-LATEST", pass: updateLatest.bodyText.includes("当前已是最新版本"), evidence: "03-update-latest-zh-CN.json/png" },
      { id: "P2F-RUN-UPDATE-FAILURE-SAFE", pass: updateFailure.bodyText.includes("无法检查更新，请重试。") && !updateFailure.bodyText.includes(backendSentinel), evidence: "04-update-check-failure-safe-zh-CN.json/png" },
      { id: "P2F-RUN-UPDATE-AVAILABLE-ZH", pass: includesAll(updateZh.bodyText, ["有可用更新", "发现新版本（9.8.7）", "2026年8月23日", "受控审计版本说明"]), evidence: "05-update-available-zh-CN.json/png" },
      { id: "P2F-RUN-UPDATE-LOCALE-FORMAT", pass: updateEn.htmlLang === "en" && includesAll(updateEn.bodyText, ["Update Available", "A new version (9.8.7) is available", "Aug 23, 2026", "受控审计版本说明"]), evidence: "06-update-locale-en-format-state-preserved.json/png" },
      { id: "P2F-RUN-ANALYTICS-ROLLBACK", pass: analyticsRollback.mock?.analyticsOptedIn === false && analyticsRollback.controls.some((item) => item.role === "switch" && item.ariaChecked === "false") && analyticsRollback.bodyText.includes("无法更新分析偏好，已恢复之前的设置。") && !analyticsRollback.bodyText.includes(backendSentinel), evidence: "07-analytics-enable-failure-rollback-safe-zh-CN.json/png" },
      { id: "P2F-RUN-ANALYTICS-EXPLICIT-OPT-IN", pass: analyticsEnabled.mock?.analyticsOptedIn === true && analyticsEnabled.mock?.analyticsInitCalls >= 2 && analyticsEnabled.bodyText.includes("audit-user-id-2f"), evidence: "08-analytics-explicit-opt-in-zh-CN.json/png" },
      { id: "P2F-RUN-TRANSPARENCY-ZH", pass: includesAll(transparencyZh.bodyText, ["分析功能会收集什么", "启用后收集的数据：", "我们不会收集：", "确认停用分析"]), evidence: "09-analytics-transparency-zh-CN.json/png" },
      { id: "P2F-RUN-TRANSPARENCY-LOCALE-STATE", pass: transparencyEn.htmlLang === "en" && transparencyEn.mock?.analyticsOptedIn === true && includesAll(transparencyEn.bodyText, ["What Analytics Collects", "Data We Collect When Enabled:", "What We DON'T Collect:"]), evidence: "10-analytics-transparency-en-state-preserved.json/png" },
      { id: "P2F-RUN-ANALYTICS-CONFIRMED-OPT-OUT", pass: analyticsDisabled.mock?.analyticsOptedIn === false && analyticsDisabled.mock?.analyticsDisableCalls >= 2, evidence: "11-analytics-confirmed-opt-out-zh-CN.json/png" },
      { id: "P2F-RUN-BETA-ZH", pass: betaZh.betaFeatures?.importAndRetranscribe === true && includesAll(betaZh.bodyText, ["Beta 功能", "导入音频与重新转写", "注意："]), evidence: "12-beta-enabled-zh-CN.json/png" },
      { id: "P2F-RUN-BETA-LOCALE-STATE", pass: betaEn.htmlLang === "en" && betaEn.betaFeatures?.importAndRetranscribe === false && includesAll(betaEn.bodyText, ["Beta Features", "Import Audio & Retranscribe", "Note:"]), evidence: "13-beta-locale-en-state-preserved.json/png" },
      { id: "P2F-RUN-NO-RAW-BACKEND-BUBBLING", pass: snapshots.every((item) => !item.bodyText.includes(backendSentinel)), evidence: "01-13 snapshots" },
      { id: "P2F-RUN-A11Y-LAYOUT", pass: snapshots.every((item) => item.documentSize.width <= item.viewport.width + 1 && item.horizontalClipping.length === 0 && item.unnamedButtons.length === 0), evidence: "01-13 snapshots" },
      { id: "P2F-RUN-NO-UNEXPECTED-RUNTIME-OR-CSP-ERRORS", pass: runtimeDiagnostics.exceptions === 0 && runtimeDiagnostics.unexpectedConsoleErrors === 0 && runtimeDiagnostics.logErrors === 0 && runtimeDiagnostics.csp === 0, evidence: "14-runtime-diagnostics.json", diagnostics: runtimeDiagnostics },
    ];
    const report = { phase: "15.5-stage-2-react-frontend-migration", batch: "2F", scope: "updates-analytics-about-beta-compliance-copy", generatedAt: new Date().toISOString(), target: { title: target.title, url: target.url }, fixtures: { backendSentinel, updateVersion: "9.8.7", userId: "audit-user-id-2f" }, limitations: ["ComplianceNotification has no product mount point in the current source and is therefore covered by static/source tests, not this runtime route audit.", "Updater download/install and relaunch are not executed; this audit controls update discovery and preparation only."], checks, summary: { passed: checks.filter((item) => item.pass).length, failed: checks.filter((item) => !item.pass).length, total: checks.length } };
    await fs.writeFile(path.join(outputDirectory, "runtime-2F-report.json"), `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed > 0) process.exitCode = 1;
  } finally { client.close(); }
}

await main();
