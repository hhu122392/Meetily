#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9444);
const outputDirectory = path.resolve(
  process.argv[3] || "docs/i18n/audit/phase-2-react/2B/runtime",
);
const transcriptFixture = "这是 2B 实时转写验收文本";
const backendSentinel = "PHASE2_2B_RAW_BACKEND_ERROR_SENTINEL";

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
      const targets = await fetch(`http://127.0.0.1:${port}/json`).then(
        (response) => response.json(),
      );
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
    throw new Error(
      result.exceptionDetails.exception?.description || result.exceptionDetails.text,
    );
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
  await evaluate(
    client,
    `localStorage.setItem('meetily.uiLocale',${JSON.stringify(locale)})`,
  );
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
  await waitForReady(client);
  await waitFor(
    client,
    `document.documentElement.lang===${JSON.stringify(locale)}`,
    `${locale} without reload`,
  );
}

async function clickText(client, text) {
  const point = await evaluate(
    client,
    `(()=>{const wanted=${JSON.stringify(text)};const candidates=[...document.querySelectorAll('button,a')];const element=candidates.find((item)=>{const rect=item.getBoundingClientRect();return (item.textContent.trim()===wanted||item.getAttribute('aria-label')===wanted)&&!item.disabled&&rect.width>0&&rect.height>0;});if(!element)return null;const rect=element.getBoundingClientRect();return{x:rect.left+rect.width/2,y:rect.top+rect.height/2};})()`,
  );
  if (!point) throw new Error(`Clickable text not found: ${text}`);
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

async function invoke(client, command, args = {}) {
  return evaluate(
    client,
    `(async()=>{try{return{resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return{resolved:false,error:String(error)}}})()`,
  );
}

async function snapshot(client, name) {
  await new Promise((resolve) => setTimeout(resolve, 750));
  const state = await evaluate(
    client,
    `(()=>{const controls=[...document.querySelectorAll('button,a,input')].map((element)=>{const rect=element.getBoundingClientRect();return{tag:element.tagName.toLowerCase(),text:element.textContent?.trim()||'',ariaLabel:element.getAttribute('aria-label'),title:element.getAttribute('title'),placeholder:element.getAttribute('placeholder'),disabled:Boolean(element.disabled),visible:rect.width>0&&rect.height>0,rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};});const visibleControls=controls.filter((item)=>item.visible);return{url:location.href,htmlLang:document.documentElement.lang,htmlDir:document.documentElement.dir,bodyText:document.body.innerText,viewport:{width:innerWidth,height:innerHeight},documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},controls:visibleControls,clippedControls:visibleControls.filter((item)=>item.rect.left<0||item.rect.top<0||item.rect.right>innerWidth+1||item.rect.bottom>innerHeight+1),unnamedButtons:visibleControls.filter((item)=>item.tag==='button'&&!item.text&&!item.ariaLabel&&!item.title),i18nReady:document.querySelector('[data-i18n-ready]')?.getAttribute('data-i18n-ready'),mock:window.__PHASE2_2B_MOCK__?{recording:window.__PHASE2_2B_MOCK__.recording,paused:window.__PHASE2_2B_MOCK__.paused,queue:window.__PHASE2_2B_MOCK__.transcriptionQueue}:null};})()`,
  );
  const image = await client.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(
      path.join(outputDirectory, `${name}.json`),
      `${JSON.stringify(state, null, 2)}\n`,
    ),
    fs.writeFile(
      path.join(outputDirectory, `${name}.png`),
      Buffer.from(image.data, "base64"),
    ),
  ]);
  return state;
}

function includesAll(text, values) {
  return values.every((value) => text.includes(value));
}

function diagnostics(events) {
  const exceptions = events.filter((event) => event.method === "Runtime.exceptionThrown");
  const consoleErrors = events.filter(
    (event) =>
      event.method === "Runtime.consoleAPICalled" && event.params?.type === "error",
  );
  const logErrors = events.filter(
    (event) =>
      event.method === "Log.entryAdded" && event.params?.entry?.level === "error",
  );
  const serialized = JSON.stringify(events);
  return {
    exceptions: exceptions.length,
    consoleErrors: consoleErrors.length,
    unexpectedConsoleErrors: consoleErrors.length,
    logErrors: logErrors.length,
    csp: (serialized.match(/content security policy|blocked by csp/gi) || []).length,
    exceptionEvents: exceptions,
    consoleErrorEvents: consoleErrors,
    logErrorEvents: logErrors,
  };
}

async function installRecordingMock(client) {
  return evaluate(
    client,
    `(async()=>{if(!window.__PHASE2_2B_ORIGINAL_FETCH__)window.__PHASE2_2B_ORIGINAL_FETCH__=window.fetch.bind(window);const nativeFetch=window.__PHASE2_2B_ORIGINAL_FETCH__;const internals=window.__TAURI_INTERNALS__;const mock=window.__PHASE2_2B_MOCK__={recording:false,paused:false,duration:42,transcriptionQueue:0,saveResolver:null,meetingName:'2B 审计会议'};const ok=(value)=>new Response(JSON.stringify(value??null),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});const emit=(event,payload)=>internals.invoke('plugin:event|emit',{event,payload});const auditFetch=async(input,init={})=>{const rawUrl=typeof input==='string'?input:input?.url;let url;try{url=new URL(rawUrl,location.href);}catch{return nativeFetch(input,init);}if(url.hostname!=='ipc.localhost')return nativeFetch(input,init);const command=decodeURIComponent(url.pathname.slice(1));let args={};if(typeof init?.body==='string'){try{args=JSON.parse(init.body)||{};}catch{}}if(command==='parakeet_init')return ok(null);if(command==='parakeet_has_available_models')return ok(true);if(command==='is_recording')return ok(mock.recording);if(command==='get_recording_state')return ok({is_recording:mock.recording,is_paused:mock.paused,is_active:mock.recording&&!mock.paused,recording_duration:mock.recording?mock.duration:null,active_duration:mock.recording?mock.duration:null});if(command==='start_recording_with_devices_and_meeting'){mock.recording=true;mock.paused=false;mock.meetingName=args.meeting_name||mock.meetingName;await emit('recording-started',null);return ok(null);}if(command==='pause_recording'){mock.paused=true;await emit('recording-paused',null);return ok(null);}if(command==='resume_recording'){mock.paused=false;await emit('recording-resumed',null);return ok(null);}if(command==='stop_recording'){mock.recording=false;mock.paused=false;await emit('recording-stopped',{message:'audit stop',folder_path:'phase2-2b-audit',meeting_name:mock.meetingName});return ok(null);}if(command==='get_transcription_status')return ok({chunks_in_queue:mock.transcriptionQueue,is_processing:mock.transcriptionQueue>0,last_activity_ms:0});if(command==='api_save_transcript')return new Promise((resolve)=>{mock.saveResolver=(value)=>resolve(ok(value));});if(command==='api_get_meeting')return ok({id:'phase2-2b-meeting',title:mock.meetingName});if(command==='api_get_meetings')return ok([{id:'phase2-2b-meeting',title:mock.meetingName}]);if(command==='api_detect_transcript_summary_language')return ok({language:'zh',reason:'detected'});if(command==='api_save_meeting_detected_summary_language')return ok({language:'zh',storage:'metadata'});if(command.startsWith('plugin:notification|')){if(command.endsWith('is_permission_granted'))return ok(true);if(command.endsWith('request_permission'))return ok('granted');return ok(null);}return nativeFetch(input,init);};window.fetch=auditFetch;if(window.fetch!==auditFetch)throw new Error('Audit fetch seam installation was rejected');return{installed:true,boundary:'http://ipc.localhost/<command>'};})()`,
  );
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
          selected_summary_model: "phase2-2b-audit",
        },
        last_updated: new Date().toISOString(),
      },
    });
    if (!onboardingStatus.resolved) {
      throw new Error(`Unable to seed onboarding: ${onboardingStatus.error}`);
    }
    await client.send("Page.reload", { ignoreCache: true });
    await waitForReady(client);
    await waitFor(
      client,
      "document.body.innerText.includes('欢迎使用 Meetily')",
      "Chinese home",
    );
    const zhCollapsed = await snapshot(client, "01-home-collapsed-zh-CN");

    await clickText(client, "展开侧边栏");
    await waitFor(
      client,
      "document.body.innerText.includes('首页')&&document.body.innerText.includes('会议记录')",
      "expanded Chinese navigation",
    );
    const zhNavigation = await snapshot(client, "02-navigation-zh-CN");

    await setLocaleLive(client, "en");
    await waitFor(
      client,
      "document.body.innerText.includes('Home')&&document.body.innerText.includes('Meeting Notes')",
      "English navigation without reload",
    );
    const enNavigation = await snapshot(client, "03-navigation-en");

    await setLocaleLive(client, "zh-CN");
    const mockInstall = await installRecordingMock(client);
    await clickText(client, "开始录音");
    await waitFor(
      client,
      "Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='暂停录音'))&&document.body.innerText.includes('正在录音')",
      "recording state",
    );
    const recording = await snapshot(client, "04-recording-zh-CN");

    await invoke(client, "plugin:event|emit", {
      event: "transcript-update",
      payload: {
        id: "phase2-2b-segment-1",
        text: transcriptFixture,
        timestamp: new Date().toISOString(),
        sequence_id: 1,
        chunk_start_time: 0,
        is_partial: false,
        confidence: 0.98,
        audio_start_time: 0.5,
        audio_end_time: 2,
        duration: 1.5,
      },
    });
    await waitFor(
      client,
      `document.body.innerText.includes(${JSON.stringify(transcriptFixture)})`,
      "live transcript fixture",
    );
    const liveTranscript = await snapshot(client, "05-live-transcript-zh-CN");

    await waitFor(
      client,
      "document.body.innerText.includes('🔴 已开始录音')&&document.body.innerText.includes('请告知所有参会者本次会议正在录音。')&&document.body.innerText.includes('我已通知参会者')",
      "localized recording participant notice",
    );
    await clickText(client, "我已通知参会者");
    await waitFor(
      client,
      "!document.body.innerText.includes('我已通知参会者')",
      "recording participant notice dismissal",
    );

    await clickText(client, "暂停录音");
    await waitFor(
      client,
      "document.body.innerText.includes('已暂停')&&Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='继续录音'))",
      "paused state",
    );
    const pausedZh = await snapshot(client, "06-paused-zh-CN");

    await setLocaleLive(client, "en");
    await waitFor(
      client,
      `document.body.innerText.includes('Paused')&&document.body.innerText.includes(${JSON.stringify(transcriptFixture)})`,
      "English paused state with preserved transcript",
    );
    const pausedEn = await snapshot(client, "07-paused-en-state-preserved");

    await clickText(client, "Resume recording");
    await waitFor(
      client,
      "Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='Pause recording'))&&document.body.innerText.includes('Recording')",
      "resumed English state",
    );
    const resumedEn = await snapshot(client, "08-resumed-en");

    await setLocaleLive(client, "zh-CN");
    await invoke(client, "plugin:event|emit", {
      event: "chunk-drop-warning",
      payload: backendSentinel,
    });
    await waitFor(
      client,
      "document.body.innerText.includes('转写性能警告')",
      "localized performance warning",
    );
    const safeWarning = await snapshot(client, "09-safe-warning-zh-CN");
    await clickText(client, "忽略");

    await evaluate(client, "window.__PHASE2_2B_MOCK__.transcriptionQueue=2");
    await clickText(client, "停止录音");
    await waitFor(
      client,
      "document.body.innerText.includes('正在完成转写')",
      "localized processing state",
    );
    const processing = await snapshot(client, "10-processing-zh-CN");

    await evaluate(client, "window.__PHASE2_2B_MOCK__.transcriptionQueue=0");
    await waitFor(
      client,
      "document.body.innerText.includes('正在保存转写内容')",
      "localized saving state",
      15000,
    );
    const saving = await snapshot(client, "11-saving-zh-CN");
    await evaluate(
      client,
      "window.__PHASE2_2B_MOCK__.saveResolver({meeting_id:'phase2-2b-meeting'})",
    );
    await waitFor(
      client,
      "document.body.innerText.includes('录音已成功保存')",
      "localized completion toast",
      10000,
    );
    const completed = await snapshot(client, "12-completed-zh-CN");

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(
      path.join(outputDirectory, "13-runtime-diagnostics.json"),
      `${JSON.stringify(runtimeDiagnostics, null, 2)}\n`,
    );

    const allSnapshots = [
      zhCollapsed,
      zhNavigation,
      enNavigation,
      recording,
      liveTranscript,
      pausedZh,
      pausedEn,
      resumedEn,
      safeWarning,
      processing,
      saving,
      completed,
    ];
    const checks = [
      {
        id: "P2B-RUN-ZH-HOME-NAVIGATION",
        pass:
          zhCollapsed.htmlLang === "zh-CN" &&
          includesAll(zhNavigation.bodyText, ["首页", "会议记录", "开始录音", "设置"]) &&
          zhNavigation.controls.some(
            (control) => control.placeholder === "搜索会议内容…",
          ) &&
          !zhNavigation.bodyText.includes("Meeting Notes"),
        evidence: "01-02 snapshots",
      },
      {
        id: "P2B-RUN-EN-NAVIGATION-LIVE-SWITCH",
        pass:
          enNavigation.htmlLang === "en" &&
          includesAll(enNavigation.bodyText, ["Home", "Meeting Notes", "Start Recording", "Settings"]) &&
          enNavigation.controls.some(
            (control) => control.placeholder === "Search meeting content...",
          ),
        evidence: "03-navigation-en.json/png",
      },
      {
        id: "P2B-RUN-RECORDING-START-AND-A11Y",
        pass:
          recording.mock?.recording === true &&
          recording.bodyText.includes("正在录音") &&
          recording.controls.some(
            (control) => control.ariaLabel === "暂停录音" && !control.disabled,
          ),
        evidence: "04-recording-zh-CN.json/png",
      },
      {
        id: "P2B-RUN-LIVE-TRANSCRIPT",
        pass:
          liveTranscript.bodyText.includes(transcriptFixture) &&
          liveTranscript.bodyText.includes("正在监听语音"),
        evidence: "05-live-transcript-zh-CN.json/png",
      },
      {
        id: "P2B-RUN-LOCALIZED-RECORDING-PARTICIPANT-NOTICE",
        pass:
          includesAll(liveTranscript.bodyText, [
            "🔴 已开始录音",
            "请告知所有参会者本次会议正在录音。",
            "不再显示",
            "我已通知参会者",
          ]) &&
          !liveTranscript.bodyText.includes("Recording Started") &&
          !liveTranscript.bodyText.includes("I've Notified Participants"),
        evidence: "05-live-transcript-zh-CN.json/png",
      },
      {
        id: "P2B-RUN-PAUSE-RESUME",
        pass:
          includesAll(pausedZh.bodyText, ["已暂停", transcriptFixture]) &&
          pausedZh.mock?.paused === true &&
          pausedZh.controls.some((control) => control.ariaLabel === "继续录音") &&
          resumedEn.mock?.recording === true &&
          resumedEn.mock?.paused === false &&
          resumedEn.controls.some((control) => control.ariaLabel === "Pause recording"),
        evidence: "06 and 08 snapshots",
      },
      {
        id: "P2B-RUN-LOCALE-SWITCH-PRESERVES-RECORDING-STATE",
        pass:
          pausedEn.htmlLang === "en" &&
          pausedEn.mock?.recording === true &&
          pausedEn.mock?.paused === true &&
          includesAll(pausedEn.bodyText, ["Paused", transcriptFixture]) &&
          pausedEn.controls.some((control) => control.ariaLabel === "Resume recording"),
        evidence: "07-paused-en-state-preserved.json/png",
      },
      {
        id: "P2B-RUN-SAFE-LOCALIZED-EVENT-ERROR",
        pass:
          includesAll(safeWarning.bodyText, [
            "转写性能警告",
            "部分音频无法实时处理",
            "忽略",
          ]) &&
          !safeWarning.bodyText.includes(backendSentinel) &&
          !safeWarning.bodyText.includes("Transcription Performance Warning"),
        evidence: "09-safe-warning-zh-CN.json/png",
      },
      {
        id: "P2B-RUN-STOP-PROCESS-SAVE-COMPLETE",
        pass:
          processing.bodyText.includes("正在完成转写") &&
          saving.bodyText.includes("正在保存转写内容") &&
          includesAll(completed.bodyText, ["录音已成功保存", "查看会议"]),
        evidence: "10-12 snapshots",
      },
      {
        id: "P2B-RUN-A11Y-AND-LAYOUT",
        pass:
          allSnapshots.every(
            (state) =>
              state.clippedControls.every(
                (control) =>
                  control.rect.left >= 0 &&
                  control.rect.right <= state.viewport.width + 1,
              ),
          ) &&
          zhCollapsed.controls.some(
            (control) => control.ariaLabel === "展开侧边栏",
          ) &&
          zhNavigation.controls.some(
            (control) => control.ariaLabel === "收起侧边栏",
          ),
        evidence: "01-12 snapshots",
      },
      {
        id: "P2B-RUN-NO-RUNTIME-OR-CSP-ERRORS",
        pass:
          runtimeDiagnostics.exceptions === 0 &&
          runtimeDiagnostics.unexpectedConsoleErrors === 0 &&
          runtimeDiagnostics.logErrors === 0 &&
          runtimeDiagnostics.csp === 0,
        evidence: "13-runtime-diagnostics.json",
        diagnostics: runtimeDiagnostics,
      },
      {
        id: "P2B-RUN-MOCK-BOUNDARY-AND-CLEANUP",
        pass:
          mockInstall?.installed === true &&
          completed.mock?.recording === false &&
          completed.mock?.paused === false,
        evidence: "runtime report",
      },
    ];

    const report = {
      phase: "15.5-stage-2-react-frontend-migration",
      batch: "2B",
      scope: "navigation-home-recording-live-transcription",
      generatedAt: new Date().toISOString(),
      target: { title: target.title, url: target.url },
      fixtures: { transcriptFixture, backendSentinel },
      checks,
      summary: {
        passed: checks.filter((check) => check.pass).length,
        failed: checks.filter((check) => !check.pass).length,
        total: checks.length,
      },
    };
    await fs.writeFile(
      path.join(outputDirectory, "runtime-2B-report.json"),
      `${JSON.stringify(report, null, 2)}\n`,
    );
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed) process.exitCode = 1;
  } finally {
    client.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
