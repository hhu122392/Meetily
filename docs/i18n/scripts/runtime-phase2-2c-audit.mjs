#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9555);
const outputDirectory = path.resolve(
  process.argv[3] || "docs/i18n/audit/phase-2-react/2C/runtime-final",
);
const meetingId = "phase2-2c-meeting";
const meetingTitle = "2C 集成验收会议";
const updatedMeetingTitle = "2C 集成验收会议（已编辑）";
const transcriptFixture = "这是 2C 会议详情与摘要验收转写。";
const initialSummaryFixture = "已完成 2C 会议详情初始摘要。";
const regeneratedSummaryFixture = "2C 重新生成摘要已通过验收。";
const backendSentinel = "PHASE2_2C_RAW_BACKEND_ERROR_SENTINEL";

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
      if (!message.id) {
        this.events.push(message);
        return;
      }
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

  close() {
    this.socket.close();
  }
}

async function findTarget() {
  let lastError;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
      const target = targets.find((candidate) => candidate.type === "page");
      if (target?.webSocketDebuggerUrl) return target;
    } catch (error) {
      lastError = error;
    }
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
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  }
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
  await waitFor(
    client,
    "Boolean(document.body && document.querySelector('[data-i18n-ready=\"true\"]'))",
    "the i18n provider",
  );
}

async function setLocaleAndReload(client, locale) {
  await evaluate(client, `localStorage.setItem('meetily.uiLocale',${JSON.stringify(locale)})`);
  await client.send("Page.reload", { ignoreCache: true });
  await waitForReady(client);
  await waitFor(
    client,
    `document.documentElement.lang===${JSON.stringify(locale)}`,
    `${locale} after reload`,
  );
}

async function setLocaleLive(client, locale) {
  await evaluate(
    client,
    `(()=>{localStorage.setItem('meetily.uiLocale',${JSON.stringify(locale)});window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:${JSON.stringify(locale)}}));})()`,
  );
  await waitFor(
    client,
    `document.documentElement.lang===${JSON.stringify(locale)}`,
    `${locale} without reload`,
  );
}

async function clickMatching(client, text, contains = false) {
  await evaluate(
    client,
    `(()=>{const wanted=${JSON.stringify(text)};const candidates=[...document.querySelectorAll('button,a,[role="button"]')];const element=candidates.find((item)=>{const rect=item.getBoundingClientRect();const label=item.getAttribute('aria-label')||'';const value=(item.textContent||'').trim();const match=${contains ? "value.includes(wanted)||label.includes(wanted)" : "value===wanted||label===wanted"};return match&&!item.disabled&&item.getAttribute('aria-disabled')!=='true'&&rect.width>0&&rect.height>0;});if(!element)return false;element.scrollIntoView({block:'center',inline:'center'});return true;})()`,
  );
  await new Promise((resolve) => setTimeout(resolve, 150));
  const point = await evaluate(
    client,
    `(()=>{const wanted=${JSON.stringify(text)};const candidates=[...document.querySelectorAll('button,a,[role="button"]')];const element=candidates.find((item)=>{const rect=item.getBoundingClientRect();const label=item.getAttribute('aria-label')||'';const value=(item.textContent||'').trim();const match=${contains ? "value.includes(wanted)||label.includes(wanted)" : "value===wanted||label===wanted"};return match&&!item.disabled&&item.getAttribute('aria-disabled')!=='true'&&rect.width>0&&rect.height>0;});if(!element)return null;const rect=element.getBoundingClientRect();return{x:rect.left+rect.width/2,y:rect.top+rect.height/2};})()`,
  );
  if (!point) throw new Error(`Clickable control not found: ${text}`);
  await client.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: point.x,
    y: point.y,
    button: "left",
    clickCount: 1,
  });
  await client.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: point.x,
    y: point.y,
    button: "left",
    clickCount: 1,
  });
}

async function clickText(client, text) {
  return clickMatching(client, text, false);
}

async function clickContains(client, text) {
  return clickMatching(client, text, true);
}

async function chooseLatestRegenerationTemplate(client) {
  await waitFor(
    client,
    "document.body.innerText.includes('选择重新生成所用的模板版本')&&document.body.innerText.includes('使用当前最新模板')",
    "the regeneration template-version dialog",
  );
  await clickContains(client, "使用当前最新模板");
}

async function invoke(client, command, args = {}) {
  return evaluate(
    client,
    `(async()=>{try{return{resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return{resolved:false,error:String(error)}}})()`,
  );
}

async function snapshot(client, name) {
  await new Promise((resolve) => setTimeout(resolve, 650));
  const state = await evaluate(
    client,
    `(()=>{const controls=[...document.querySelectorAll('button,a,input,[role="button"]')].map((element)=>{const rect=element.getBoundingClientRect();return{tag:element.tagName.toLowerCase(),text:element.textContent?.trim()||'',ariaLabel:element.getAttribute('aria-label'),title:element.getAttribute('title'),placeholder:element.getAttribute('placeholder'),disabled:Boolean(element.disabled)||element.getAttribute('aria-disabled')==='true',visible:rect.width>0&&rect.height>0,rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};});const visibleControls=controls.filter((item)=>item.visible);const mock=window.__PHASE2_2C_MOCK__;return{url:location.href,htmlLang:document.documentElement.lang,htmlDir:document.documentElement.dir,bodyText:document.body.innerText,viewport:{width:innerWidth,height:innerHeight},documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},controls:visibleControls,clippedControls:visibleControls.filter((item)=>item.rect.left<0||item.rect.top<0||item.rect.right>innerWidth+1||item.rect.bottom>innerHeight+1),unnamedButtons:visibleControls.filter((item)=>item.tag==='button'&&!item.text&&!item.ariaLabel&&!item.title),i18nReady:document.querySelector('[data-i18n-ready]')?.getAttribute('data-i18n-ready'),mock:mock?{meetingId:mock.meeting.id,meetingTitle:mock.meeting.title,templateId:mock.templateId,summaryLanguage:mock.summaryLanguage,summaryPhase:mock.summaryPhase,processCalls:mock.processCalls,cancelCalls:mock.cancelCalls,clipboard:mock.clipboard,failNextTemplateSave:mock.failNextTemplateSave}:null};})()`,
  );
  const image = await client.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(state, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, "base64")),
  ]);
  return state;
}

function mockBootstrapSource() {
  const transcripts = [
    {
      id: "phase2-2c-transcript-1",
      text: transcriptFixture,
      timestamp: "2026-08-23T00:00:00.000Z",
      sequence_id: 1,
      chunk_start_time: 0,
      is_partial: false,
      confidence: 0.98,
      audio_start_time: 0,
      audio_end_time: 4.2,
      duration: 4.2,
    },
    {
      id: "phase2-2c-transcript-2",
      text: "决定继续完成多语言与会议模板集成。",
      timestamp: "2026-08-23T00:00:05.000Z",
      sequence_id: 2,
      chunk_start_time: 5,
      is_partial: false,
      confidence: 0.97,
      audio_start_time: 5,
      audio_end_time: 9.5,
      duration: 4.5,
    },
  ];
  const template = (id, name, origin, isDefault) => ({
    id,
    name,
    description: `${name}验收模板`,
    origin,
    schemaVersion: 2,
    version: 1,
    locale: "zh-CN",
    tags: ["audit"],
    sectionCount: 3,
    sourceType: null,
    updatedAt: "2026-08-23T00:00:00.000Z",
    fileSha256: `${id}-sha256`,
    semanticSha256: `${id}-semantic`,
    isDefault,
    readOnly: origin === "builtin",
    overridesBuiltin: false,
    valid: true,
    validationSummary: { errorCount: 0, warningCount: 0 },
  });
  const stateLiteral = {
    meeting: {
      id: meetingId,
      title: meetingTitle,
      created_at: "2026-08-23T00:00:00.000Z",
      updated_at: "2026-08-23T00:10:00.000Z",
      folder_path: "phase2-2c-audit-folder",
    },
    transcripts,
    templates: [
      template("customer-review", "客户项目复盘", "custom", false),
      template("standard_meeting", "标准会议", "builtin", true),
    ],
    templateId: "standard_meeting",
    templateMode: "inherit",
    summaryLanguage: "zh",
    summaryPhase: "completed",
    summaryMarkdown: `# 会议摘要\n\n- ${initialSummaryFixture}\n- 行动项：完成运行时严格验收。`,
    processCalls: 0,
    cancelCalls: 0,
    clipboard: "",
    failNextTemplateSave: false,
  };
  return `(()=>{if(window.__PHASE2_2C_MOCK_INSTALLED__)return;window.__PHASE2_2C_MOCK_INSTALLED__=true;const nativeFetch=window.fetch.bind(window);const state=window.__PHASE2_2C_MOCK__=${JSON.stringify(stateLiteral)};const ok=(value)=>new Response(JSON.stringify(value??null),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});const fail=(value)=>new Response(JSON.stringify(value),{status:500,headers:{'Content-Type':'application/json','Tauri-Response':'error'}});const preference=()=>{const selected=state.templates.find((item)=>item.id===state.templateId)||state.templates[1];return{preference:{schemaVersion:1,mode:state.templateMode,templateId:state.templateMode==='inherit'?null:state.templateId,templateVersion:state.templateMode==='inherit'?null:1,templateFileSha256:state.templateMode==='inherit'?null:selected.fileSha256,selectedAt:'2026-08-23T00:00:00.000Z'},storage:'metadata',resolved:{templateId:selected.id,name:selected.name,version:1,fileSha256:selected.fileSha256,origin:selected.origin,source:state.templateMode==='inherit'?'global_default':'meeting_override'}}};window.fetch=async(input,init={})=>{const rawUrl=typeof input==='string'?input:input?.url;let url;try{url=new URL(rawUrl,location.href);}catch{return nativeFetch(input,init);}if(url.hostname!=='ipc.localhost')return nativeFetch(input,init);const command=decodeURIComponent(url.pathname.slice(1));let args={};if(typeof init?.body==='string'){try{args=JSON.parse(init.body)||{};}catch{}}if(command==='parakeet_init')return ok(null);if(command==='parakeet_has_available_models')return ok(true);if(command==='is_recording')return ok(false);if(command==='get_recording_state')return ok({is_recording:false,is_paused:false,is_active:false,recording_duration:null,active_duration:null});if(command==='api_get_meetings')return ok([{id:state.meeting.id,title:state.meeting.title}]);if(command==='api_search_transcripts')return ok([]);if(command==='api_get_meeting_metadata')return ok(state.meeting);if(command==='api_get_meeting_transcripts')return ok({transcripts:state.transcripts,total_count:state.transcripts.length,has_more:false});if(command==='api_get_summary'){if(state.summaryPhase==='completed')return ok({status:'completed',data:{markdown:state.summaryMarkdown}});if(state.summaryPhase==='error')return ok({status:'error',error:${JSON.stringify(backendSentinel)},data:null});return ok({status:state.summaryPhase,data:null});}if(command==='api_get_model_config')return ok({provider:'openai',model:'audit-model',whisperModel:'large-v3',apiKey:null,ollamaEndpoint:null});if(command==='api_get_api_key')return ok(null);if(command==='get_ollama_models')return ok([{name:'audit-model'}]);if(command==='api_list_templates_v2')return ok({templates:state.templates,diagnostics:[],deletedTemplates:[],defaultTemplateId:'standard_meeting'});if(command==='api_get_meeting_template_preference')return ok(preference());if(command==='api_save_meeting_template_preference'){if(state.failNextTemplateSave){state.failNextTemplateSave=false;return fail(${JSON.stringify(backendSentinel)});}const request=args.request||{};state.templateMode=request.preference?.mode||'inherit';state.templateId=state.templateMode==='inherit'?'standard_meeting':request.preference?.templateId||'standard_meeting';return ok(preference());}if(command==='api_get_meeting_summary_language')return ok({language:state.summaryLanguage,storage:'metadata'});if(command==='api_save_meeting_summary_language'){state.summaryLanguage=args.summaryLanguage??args.summary_language??null;return ok({language:state.summaryLanguage,storage:'metadata'});}if(command==='api_get_meeting_detected_summary_language')return ok({language:'zh',storage:'metadata'});if(command==='api_save_meeting_detected_summary_language')return ok({language:'zh',storage:'metadata'});if(command==='api_detect_transcript_summary_language')return ok({language:'zh',reason:'detected'});if(command==='api_save_meeting_title'){state.meeting.title=args.title||state.meeting.title;return ok(null);}if(command==='api_save_meeting_summary')return ok(null);if(command==='open_meeting_folder')return ok(null);if(command==='api_process_transcript'){state.processCalls+=1;state.summaryPhase='processing';return ok({process_id:'phase2-2c-process-'+state.processCalls});}if(command==='api_cancel_summary'){state.cancelCalls+=1;state.summaryPhase='completed';return ok(null);}if(command==='api_save_model_config')return ok(null);return nativeFetch(input,init);};try{Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async(text)=>{state.clipboard=String(text);},readText:async()=>state.clipboard}});}catch{};})();`;
}

function includesAll(text, values) {
  return values.every((value) => text.includes(value));
}

function diagnostics(events) {
  const exceptions = events.filter((event) => event.method === "Runtime.exceptionThrown");
  const consoleErrors = events.filter(
    (event) => event.method === "Runtime.consoleAPICalled" && event.params?.type === "error",
  );
  const expectedConsoleErrors = consoleErrors.filter((event) =>
    JSON.stringify(event).includes(backendSentinel),
  );
  const unexpectedConsoleErrors = consoleErrors.filter((event) =>
    !JSON.stringify(event).includes(backendSentinel),
  );
  const logErrors = events.filter(
    (event) => event.method === "Log.entryAdded" && event.params?.entry?.level === "error",
  );
  const serialized = JSON.stringify(events);
  return {
    exceptions: exceptions.length,
    consoleErrors: consoleErrors.length,
    expectedConsoleErrors: expectedConsoleErrors.length,
    unexpectedConsoleErrors: unexpectedConsoleErrors.length,
    logErrors: logErrors.length,
    csp: (serialized.match(/content security policy|blocked by csp/gi) || []).length,
    exceptionEvents: exceptions,
    expectedConsoleErrorEvents: expectedConsoleErrors,
    unexpectedConsoleErrorEvents: unexpectedConsoleErrors,
    logErrorEvents: logErrors,
  };
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const target = await findTarget();
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([
    client.send("Runtime.enable"),
    client.send("Page.enable"),
    client.send("Log.enable"),
    client.send("Network.enable"),
  ]);
  await client.send("Emulation.setDeviceMetricsOverride", {
    width: 1440,
    height: 900,
    deviceScaleFactor: 1,
    mobile: false,
  });

  try {
    await waitForReady(client);
    await setLocaleAndReload(client, "zh-CN");
    const onboardingStatus = await invoke(client, "save_onboarding_status_cmd", {
      status: {
        version: "1.0",
        completed: true,
        current_step: 4,
        model_status: {
          parakeet: "downloaded",
          summary: "downloaded",
          selected_summary_model: "phase2-2c-audit",
        },
        last_updated: new Date().toISOString(),
      },
    });
    if (!onboardingStatus.resolved) {
      throw new Error(`Unable to seed onboarding: ${onboardingStatus.error}`);
    }

    const bootstrap = mockBootstrapSource();
    await client.send("Page.addScriptToEvaluateOnNewDocument", { source: bootstrap });
    await client.send("Page.navigate", {
      url: `http://tauri.localhost/meeting-details?id=${meetingId}`,
    });
    await waitForReady(client);
    await waitFor(
      client,
      `window.__PHASE2_2C_MOCK__.meeting.title===${JSON.stringify(meetingTitle)}&&document.body.innerText.includes(${JSON.stringify(transcriptFixture)})&&document.body.innerText.includes(${JSON.stringify(initialSummaryFixture)})`,
      "the Chinese meeting detail fixture",
      45000,
    );
    const zhMeeting = await snapshot(client, "01-meeting-details-zh-CN");

    await clickText(client, "编辑会议标题");
    await waitFor(
      client,
      `Boolean([...document.querySelectorAll('textarea')].find((item)=>item.value===${JSON.stringify(meetingTitle)}))`,
      "the editable meeting title",
    );
    await evaluate(
      client,
      `(()=>{const element=[...document.querySelectorAll('textarea')].find((item)=>item.value===${JSON.stringify(meetingTitle)});if(!element)return false;element.focus();element.select();return true;})()`,
    );
    await client.send("Input.insertText", { text: updatedMeetingTitle });
    await client.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter" });
    await client.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter" });
    await waitFor(
      client,
      `document.body.innerText.includes(${JSON.stringify(updatedMeetingTitle)})`,
      "the edited meeting title",
    );
    await clickText(client, "保存更改");
    await waitFor(
      client,
      `window.__PHASE2_2C_MOCK__.meeting.title===${JSON.stringify(updatedMeetingTitle)}`,
      "meeting title persistence",
    );
    const titleEdited = await snapshot(client, "01b-meeting-title-edited-zh-CN");

    await clickText(client, "复制转写内容");
    await waitFor(
      client,
      `window.__PHASE2_2C_MOCK__.clipboard.includes(${JSON.stringify(transcriptFixture)})&&document.body.innerText.includes('转写内容已复制到剪贴板')`,
      "localized transcript copy",
    );
    const transcriptCopied = await snapshot(client, "02-transcript-copy-zh-CN");

    await clickText(client, "复制摘要");
    await waitFor(
      client,
      `window.__PHASE2_2C_MOCK__.clipboard.includes(${JSON.stringify(initialSummaryFixture)})&&document.body.innerText.includes('摘要已复制到剪贴板')`,
      "localized summary copy",
    );
    const summaryCopied = await snapshot(client, "03-summary-copy-zh-CN");

    await clickText(client, "选择会议总结模板");
    await waitFor(
      client,
      "document.body.innerText.includes('会议总结模板')&&document.body.innerText.includes('客户项目复盘')",
      "the localized template picker",
    );
    const templatePicker = await snapshot(client, "04-template-picker-zh-CN");
    await clickContains(client, "客户项目复盘");
    await waitFor(
      client,
      "window.__PHASE2_2C_MOCK__.templateId==='customer-review'&&document.body.innerText.includes('会议模板已保存')",
      "meeting template persistence",
    );
    const templateSaved = await snapshot(client, "05-template-saved-zh-CN");

    await evaluate(client, "window.__PHASE2_2C_MOCK__.failNextTemplateSave=true");
    await clickText(client, "选择会议总结模板");
    await waitFor(client, "document.body.innerText.includes('标准会议')", "the built-in template option");
    await clickContains(client, "标准会议");
    await waitFor(
      client,
      "document.body.innerText.includes('无法保存本会议模板')&&document.body.innerText.includes('请重新加载，原选择没有被更改。')",
      "safe localized template failure",
    );
    const templateFailure = await snapshot(client, "06-template-safe-error-zh-CN");
    await clickText(client, "选择会议总结模板");
    await clickText(client, "重新加载");
    await waitFor(
      client,
      "Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='选择会议总结模板'&&!item.disabled))",
      "template error recovery",
    );
    await waitFor(
      client,
      "!document.body.innerText.includes('无法保存本会议模板')",
      "the recovered template error toast to close before locale switching",
      10000,
    );

    await clickText(client, "设置摘要语言");
    await waitFor(client, "Boolean([...document.querySelectorAll('input')].find((item)=>(item.placeholder||'').includes('搜索语言')))", "the summary language picker");
    await clickContains(client, "(ja)");
    await waitFor(
      client,
      "window.__PHASE2_2C_MOCK__.summaryLanguage==='ja'&&document.body.innerText.includes('日语')",
      "Japanese summary-language persistence",
    );
    const languageZh = await snapshot(client, "07-summary-language-ja-zh-CN");

    await setLocaleLive(client, "en");
    await waitFor(
      client,
      `document.body.innerText.includes('Japanese')&&document.body.innerText.includes(${JSON.stringify(transcriptFixture)})&&document.body.innerText.includes(${JSON.stringify(initialSummaryFixture)})`,
      "English meeting details with preserved business state",
    );
    const statePreservedEn = await snapshot(client, "08-locale-en-state-preserved");
    await setLocaleLive(client, "zh-CN");

    await clickText(client, "重新生成摘要");
    await chooseLatestRegenerationTemplate(client);
    await waitFor(
      client,
      "window.__PHASE2_2C_MOCK__.processCalls===1&&Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='停止生成摘要'))",
      "summary generation start",
    );
    const generating = await snapshot(client, "09-summary-generating-zh-CN");
    await clickText(client, "停止生成摘要");
    await waitFor(
      client,
      "window.__PHASE2_2C_MOCK__.cancelCalls===1&&document.body.innerText.includes('已停止生成摘要')",
      "summary generation stop",
    );
    const stopped = await snapshot(client, "10-summary-stopped-zh-CN");

    await clickText(client, "重新生成摘要");
    await chooseLatestRegenerationTemplate(client);
    await waitFor(client, "window.__PHASE2_2C_MOCK__.processCalls===2", "second summary generation");
    await evaluate(client, "window.__PHASE2_2C_MOCK__.summaryPhase='error'");
    await waitFor(
      client,
      "document.body.innerText.includes('生成摘要失败')&&!document.body.innerText.includes('PHASE2_2C_RAW_BACKEND_ERROR_SENTINEL')",
      "safe localized summary error",
      12000,
    );
    const safeSummaryError = await snapshot(client, "11-summary-safe-error-zh-CN");

    await clickText(client, "重新生成摘要");
    await chooseLatestRegenerationTemplate(client);
    await waitFor(client, "window.__PHASE2_2C_MOCK__.processCalls===3", "summary regeneration retry");
    await evaluate(
      client,
      `(()=>{window.__PHASE2_2C_MOCK__.summaryMarkdown=${JSON.stringify(`# 重新生成摘要\n\n- ${regeneratedSummaryFixture}`)};window.__PHASE2_2C_MOCK__.summaryPhase='completed';})()`,
    );
    await waitFor(
      client,
      `document.body.innerText.includes(${JSON.stringify(regeneratedSummaryFixture)})&&document.body.innerText.includes('摘要已成功生成')`,
      "successful summary regeneration",
      12000,
    );
    const regenerated = await snapshot(client, "12-summary-regenerated-zh-CN");

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(
      path.join(outputDirectory, "13-runtime-diagnostics.json"),
      `${JSON.stringify(runtimeDiagnostics, null, 2)}\n`,
    );

    const snapshots = [
      zhMeeting,
      titleEdited,
      transcriptCopied,
      summaryCopied,
      templatePicker,
      templateSaved,
      templateFailure,
      languageZh,
      statePreservedEn,
      generating,
      stopped,
      safeSummaryError,
      regenerated,
    ];
    const checks = [
      {
        id: "P2C-RUN-ZH-MEETING-DETAILS",
        pass: zhMeeting.htmlLang === "zh-CN" && zhMeeting.mock?.meetingTitle === meetingTitle && includesAll(zhMeeting.bodyText, [meetingTitle, transcriptFixture, initialSummaryFixture]),
        evidence: "01-meeting-details-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-MEETING-TITLE-EDIT-AND-SAVE",
        pass: titleEdited.mock?.meetingTitle === updatedMeetingTitle && titleEdited.bodyText.includes(updatedMeetingTitle),
        evidence: "01b-meeting-title-edited-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-TRANSCRIPT-COPY-AND-INTL",
        pass: transcriptCopied.mock?.clipboard.includes(transcriptFixture) && transcriptCopied.bodyText.includes("转写内容已复制到剪贴板"),
        evidence: "02-transcript-copy-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-SUMMARY-RENDER-AND-COPY",
        pass: summaryCopied.mock?.clipboard.includes(initialSummaryFixture) && summaryCopied.bodyText.includes("摘要已复制到剪贴板"),
        evidence: "03-summary-copy-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-TEMPLATE-PICKER-LOCALIZED",
        pass: includesAll(templatePicker.bodyText, ["会议总结模板", "自定义模板", "内置模板"]),
        evidence: "04-template-picker-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-TEMPLATE-PREFERENCE-PERSISTS",
        pass: templateSaved.mock?.templateId === "customer-review" && templateSaved.bodyText.includes("客户项目复盘"),
        evidence: "05-template-saved-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-SAFE-TEMPLATE-ERROR",
        pass: includesAll(templateFailure.bodyText, ["无法保存本会议模板", "请重新加载"]) && !templateFailure.bodyText.includes(backendSentinel) && templateFailure.mock?.templateId === "customer-review",
        evidence: "06-template-safe-error-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-SUMMARY-LANGUAGE-PERSISTS",
        pass: languageZh.mock?.summaryLanguage === "ja" && languageZh.bodyText.includes("日语"),
        evidence: "07-summary-language-ja-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-LOCALE-SWITCH-PRESERVES-BUSINESS-STATE",
        pass: statePreservedEn.htmlLang === "en" && statePreservedEn.url.includes(meetingId) && statePreservedEn.mock?.templateId === "customer-review" && statePreservedEn.mock?.summaryLanguage === "ja" && includesAll(statePreservedEn.bodyText, ["Japanese", transcriptFixture, initialSummaryFixture]),
        evidence: "08-locale-en-state-preserved.json/png",
      },
      {
        id: "P2C-RUN-SUMMARY-GENERATION-STOP",
        pass: generating.mock?.processCalls === 1 && stopped.mock?.cancelCalls === 1 && stopped.bodyText.includes("已停止生成摘要"),
        evidence: "09-10 snapshots",
      },
      {
        id: "P2C-RUN-SAFE-SUMMARY-ERROR",
        pass: safeSummaryError.bodyText.includes("生成摘要失败") && !safeSummaryError.bodyText.includes(backendSentinel),
        evidence: "11-summary-safe-error-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-SUMMARY-REGENERATION-SUCCESS",
        pass: regenerated.mock?.processCalls === 3 && regenerated.bodyText.includes(regeneratedSummaryFixture),
        evidence: "12-summary-regenerated-zh-CN.json/png",
      },
      {
        id: "P2C-RUN-A11Y-AND-LAYOUT",
        pass: snapshots.every((item) => item.documentSize.width <= item.viewport.width + 1 && item.clippedControls.length === 0) && zhMeeting.controls.some((item) => item.ariaLabel === "复制转写内容") && zhMeeting.controls.some((item) => item.ariaLabel === "复制摘要") && zhMeeting.controls.some((item) => item.ariaLabel === "设置摘要语言") && zhMeeting.controls.some((item) => item.ariaLabel === "选择会议总结模板"),
        evidence: "01-12 snapshots",
      },
      {
        id: "P2C-RUN-NO-UNEXPECTED-RUNTIME-OR-CSP-ERRORS",
        pass: runtimeDiagnostics.exceptions === 0 && runtimeDiagnostics.unexpectedConsoleErrors === 0 && runtimeDiagnostics.logErrors === 0 && runtimeDiagnostics.csp === 0,
        evidence: "13-runtime-diagnostics.json",
        diagnostics: runtimeDiagnostics,
      },
      {
        id: "P2C-RUN-MOCK-BOUNDARY-AND-CLEANUP",
        pass: regenerated.mock?.summaryPhase === "completed" && regenerated.mock?.failNextTemplateSave === false,
        evidence: "runtime report",
      },
    ];
    const report = {
      phase: "15.5-stage-2-react-frontend-migration",
      batch: "2C",
      scope: "meetings-details-summary-templates-integration",
      generatedAt: new Date().toISOString(),
      target: { title: target.title, url: target.url },
      fixtures: { meetingId, meetingTitle, updatedMeetingTitle, transcriptFixture, initialSummaryFixture, regeneratedSummaryFixture, backendSentinel },
      checks,
      summary: {
        passed: checks.filter((item) => item.pass).length,
        failed: checks.filter((item) => !item.pass).length,
        total: checks.length,
      },
    };
    await fs.writeFile(
      path.join(outputDirectory, "runtime-2C-report.json"),
      `${JSON.stringify(report, null, 2)}\n`,
    );
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed > 0) process.exitCode = 1;
  } finally {
    client.close();
  }
}

await main();
