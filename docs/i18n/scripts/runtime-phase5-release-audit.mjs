#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9355);
const root = path.resolve(process.argv[3] || ".");
const outputDirectory = path.join(root, "docs/i18n/audit/phase-5-release/runtime");
const reportPath = path.join(outputDirectory, "runtime-audit.json");
await fs.mkdir(path.join(outputDirectory, "screenshots"), { recursive: true });

const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
const target = targets.find((candidate) => candidate.type === "page");
if (!target?.webSocketDebuggerUrl) throw new Error("No Tauri page target found");

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
let nextId = 1;
const pending = new Map();
const diagnostics = { exceptions: [], logErrors: [], consoleErrors: [] };
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const handler = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) {
      handler.reject(
        new Error(
          `${handler.method} ${handler.expression || ""}: ${JSON.stringify(message.error)}`,
        ),
      );
    }
    else handler.resolve(message.result);
    return;
  }
  if (message.method === "Runtime.exceptionThrown") diagnostics.exceptions.push(message.params);
  if (message.method === "Log.entryAdded" && message.params?.entry?.level === "error") {
    diagnostics.logErrors.push(message.params.entry);
  }
  if (message.method === "Runtime.consoleAPICalled" && message.params?.type === "error") {
    diagnostics.consoleErrors.push(message.params);
  }
});
const call = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, {
      resolve,
      reject,
      method,
      expression: typeof params.expression === "string" ? params.expression.slice(0, 180) : "",
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
await call("Runtime.enable");
await call("Log.enable");
await call("Page.enable");
await call("Network.enable");
await call("Performance.enable");

async function evaluate(expression) {
  const result = await call("Runtime.evaluate", {
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

async function waitFor(expression, label, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}`);
}

async function ready() {
  await waitFor(
    `Boolean(document.readyState==='complete'&&document.querySelector('[data-i18n-ready="true"]'))`,
    "localized application readiness",
    45000,
  );
}

async function setLocale(locale) {
  await evaluate(`(()=>{
    localStorage.setItem('meetily.uiLocale',${JSON.stringify(locale)});
    window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:${JSON.stringify(locale)}}));
  })()`);
  await waitFor(
    `Boolean(document.documentElement.lang===${JSON.stringify(locale)}&&document.querySelector('[data-i18n-ready="true"]'))`,
    `${locale} activation`,
  );
}

async function navigate(route) {
  await call("Page.navigate", { url: `http://tauri.localhost${route}` });
  await ready();
  await waitFor(`location.pathname===${JSON.stringify(route)}`, route);
  await new Promise((resolve) => setTimeout(resolve, 700));
}

async function auditDom() {
  return evaluate(`(()=>{
    const visible=(element)=>{const style=getComputedStyle(element);const rect=element.getBoundingClientRect();return style.visibility!=='hidden'&&style.display!=='none'&&rect.width>0&&rect.height>0&&rect.bottom>0&&rect.right>0&&rect.left<innerWidth&&rect.top<innerHeight};
    const text=(element)=>(element.textContent||'').replace(/\\s+/g,' ').trim();
    const controls=[...document.querySelectorAll('button,a,input,select,textarea,[role="button"],[role="tab"],[role="switch"],[role="checkbox"]')].filter(visible);
    const name=(element)=>element.getAttribute('aria-label')||element.getAttribute('title')||element.getAttribute('placeholder')||text(element)||element.getAttribute('value')||'';
    const unnamed=controls.filter((element)=>!name(element)).map((element)=>({tag:element.tagName.toLowerCase(),role:element.getAttribute('role'),html:element.outerHTML.slice(0,240)}));
    const clipped=controls.filter((element)=>{const rect=element.getBoundingClientRect();return rect.left<-1||rect.right>innerWidth+1}).map((element)=>({name:name(element),left:element.getBoundingClientRect().left,right:element.getBoundingClientRect().right}));
    return {
      route:location.pathname,
      locale:document.documentElement.lang,
      dir:document.documentElement.dir||'ltr',
      title:document.title,
      viewport:{width:innerWidth,height:innerHeight,devicePixelRatio},
      document:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight},
      controls:controls.length,
      unnamed,
      clipped,
      replacementCharacters:(document.body.innerText.match(/�/g)||[]).length,
      untranslatedKeys:(document.body.innerText.match(/\\b(?:common|settings|recording|summary|templates|onboarding|import|updates|analytics):[A-Za-z0-9_.-]+\\b/g)||[]),
      bodyTextSample:document.body.innerText.slice(0,1200)
    };
  })()`);
}

async function capture(name, width, height, deviceScaleFactor) {
  await call("Emulation.setDeviceMetricsOverride", {
    width,
    height,
    deviceScaleFactor,
    mobile: false,
  });
  await new Promise((resolve) => setTimeout(resolve, 500));
  const dom = await auditDom();
  const shot = await call("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(
      path.join(outputDirectory, "screenshots", `${name}.png`),
      Buffer.from(shot.data, "base64"),
    ),
    fs.writeFile(
      path.join(outputDirectory, "screenshots", `${name}.json`),
      JSON.stringify(dom, null, 2) + "\n",
    ),
  ]);
  return { name, ...dom };
}

const initialOnboarding = await evaluate(
  `window.__TAURI_INTERNALS__.invoke('get_onboarding_status')`,
);
await evaluate(`window.__TAURI_INTERNALS__.invoke('save_onboarding_status_cmd',{status:{
  version:'1.0',completed:true,current_step:4,
  model_status:{parakeet:'not_downloaded',summary:'not_downloaded'},
  last_updated:new Date().toISOString()
}})`);
await call("Page.reload");
await ready();

const stateBefore = await evaluate(`(async()=>({
  transcript:await window.__TAURI_INTERNALS__.invoke('api_get_transcript_config'),
  model:await window.__TAURI_INTERNALS__.invoke('api_get_model_config'),
  recording:await window.__TAURI_INTERNALS__.invoke('is_recording')
}))()`);
const metricsBefore = await call("Performance.getMetrics");

const screenshots = [];
await setLocale("en");
await navigate("/settings");
screenshots.push(await capture("settings-en-1100x600-100", 1100, 600, 1));
await setLocale("zh-CN");
screenshots.push(await capture("settings-zh-CN-1100x600-100", 1100, 600, 1));
screenshots.push(await capture("settings-zh-CN-800x600-125", 800, 600, 1.25));
await navigate("/settings/templates");
screenshots.push(await capture("templates-zh-CN-1100x700-150", 1100, 700, 1.5));
await setLocale("en");
screenshots.push(await capture("templates-en-1100x700-200", 1100, 700, 2));

await call("Network.emulateNetworkConditions", {
  offline: true,
  latency: 0,
  downloadThroughput: 0,
  uploadThroughput: 0,
});
await setLocale("zh-CN");
const offline = await evaluate(`(async()=>({
  locale:document.documentElement.lang,
  ready:document.querySelector('[data-i18n-ready]')?.getAttribute('data-i18n-ready'),
  templates:(await window.__TAURI_INTERNALS__.invoke('api_list_templates_v2',{request:{origin:'builtin',contentLocale:'zh-CN'}})).templates.length
}))()`);
await call("Network.emulateNetworkConditions", {
  offline: false,
  latency: 0,
  downloadThroughput: -1,
  uploadThroughput: -1,
});

const rapidSwitch = await evaluate(`(async()=>{
  for(let index=0;index<50;index+=1){
    const locale=index%2===0?'en':'zh-CN';
    localStorage.setItem('meetily.uiLocale',locale);
    window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:locale}));
    await new Promise(resolve=>setTimeout(resolve,40));
  }
  await new Promise(resolve=>setTimeout(resolve,1000));
  return {
    documentLocale:document.documentElement.lang,
    nativeLocale:await window.__TAURI_INTERNALS__.invoke('get_ui_locale'),
    recording:await window.__TAURI_INTERNALS__.invoke('is_recording'),
    providerCount:document.querySelectorAll('[data-i18n-ready]').length
  };
})()`);
const stateAfter = await evaluate(`(async()=>({
  transcript:await window.__TAURI_INTERNALS__.invoke('api_get_transcript_config'),
  model:await window.__TAURI_INTERNALS__.invoke('api_get_model_config'),
  recording:await window.__TAURI_INTERNALS__.invoke('is_recording')
}))()`);
const metricsAfter = await call("Performance.getMetrics");
const metric = (result, name) => result.metrics.find((entry) => entry.name === name)?.value ?? null;
const heapBefore = metric(metricsBefore, "JSHeapUsedSize");
const heapAfter = metric(metricsAfter, "JSHeapUsedSize");

const allDom = screenshots;
const assertions = {
  onboardingStateIsIsolated: initialOnboarding === null || typeof initialOnboarding === "object",
  englishAndChineseScreenshotsCaptured: screenshots.length === 5,
  htmlLanguageCorrect: allDom.every((entry) => entry.locale === (entry.name.includes("zh-CN") ? "zh-CN" : "en")),
  noReplacementCharacters: allDom.every((entry) => entry.replacementCharacters === 0),
  noTranslationKeysVisible: allDom.every((entry) => entry.untranslatedKeys.length === 0),
  noHorizontallyClippedControls: allDom.every((entry) => entry.clipped.length === 0),
  allVisibleControlsNamed: allDom.every((entry) => entry.unnamed.length === 0),
  offlineResourcesAvailable: offline.locale === "zh-CN" && offline.ready === "true" && offline.templates === 6,
  fiftySwitchesStable:
    rapidSwitch.documentLocale === "zh-CN" &&
    rapidSwitch.nativeLocale?.locale === "zh-CN" &&
    rapidSwitch.recording === false &&
    rapidSwitch.providerCount === 1,
  transcriptConfigUnchanged: JSON.stringify(stateBefore.transcript) === JSON.stringify(stateAfter.transcript),
  modelConfigUnchanged: JSON.stringify(stateBefore.model) === JSON.stringify(stateAfter.model),
  recordingStateUnchanged: stateBefore.recording === stateAfter.recording,
  jsHeapGrowthBounded:
    heapBefore !== null && heapAfter !== null && heapAfter - heapBefore < 20 * 1024 * 1024,
  noRuntimeExceptions: diagnostics.exceptions.length === 0,
  noLogErrors: diagnostics.logErrors.length === 0,
  noConsoleErrors: diagnostics.consoleErrors.length === 0,
};
const passed = Object.values(assertions).every(Boolean);
const report = {
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  port,
  passed,
  assertions,
  screenshots: screenshots.map((entry) => ({
    name: entry.name,
    locale: entry.locale,
    viewport: entry.viewport,
    controls: entry.controls,
    unnamedControls: entry.unnamed.length,
    clippedControls: entry.clipped.length,
    replacementCharacters: entry.replacementCharacters,
    untranslatedKeys: entry.untranslatedKeys,
  })),
  offline,
  rapidSwitch,
  stateIsolation: {
    transcriptConfigUnchanged: assertions.transcriptConfigUnchanged,
    modelConfigUnchanged: assertions.modelConfigUnchanged,
    recordingStateUnchanged: assertions.recordingStateUnchanged,
  },
  performance: {
    jsHeapUsedBefore: heapBefore,
    jsHeapUsedAfter: heapAfter,
    jsHeapGrowthBytes: heapAfter !== null && heapBefore !== null ? heapAfter - heapBefore : null,
  },
  diagnostics: {
    exceptions: diagnostics.exceptions.length,
    logErrors: diagnostics.logErrors.length,
    consoleErrors: diagnostics.consoleErrors.length,
  },
};
await fs.writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
socket.close();
process.stdout.write(JSON.stringify({ reportPath, passed, assertions }, null, 2) + "\n");
if (!passed) process.exitCode = 1;
