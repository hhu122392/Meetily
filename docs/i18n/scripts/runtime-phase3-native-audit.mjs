#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9345);
const root = path.resolve(process.argv[3] || ".");
const reportPath = path.join(root, "docs/i18n/audit/phase-3-native/runtime-audit.json");
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
    const { resolve, reject } = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) reject(new Error(JSON.stringify(message.error)));
    else resolve(message.result);
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
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
await call("Runtime.enable");
await call("Log.enable");

async function evaluate(expression) {
  const result = await call("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
  return result.result.value;
}

const initial = await evaluate(`(async()=>({
  native: await window.__TAURI_INTERNALS__.invoke('get_ui_locale'),
  document: document.documentElement.lang,
  preference: localStorage.getItem('meetily.uiLocale'),
  ready: document.querySelector('[data-i18n-ready]')?.getAttribute('data-i18n-ready')
}))()`);

const switchResult = await evaluate(`(async()=>{
  const apply=async(value)=>{
    localStorage.setItem('meetily.uiLocale',value);
    window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:value}));
    await new Promise(resolve=>setTimeout(resolve,500));
    return {
      document: document.documentElement.lang,
      native: await window.__TAURI_INTERNALS__.invoke('get_ui_locale')
    };
  };
  const zhCN=await apply('zh-CN');
  await window.__TAURI_INTERNALS__.invoke('show_test_notification');
  const en=await apply('en');
  await window.__TAURI_INTERNALS__.invoke('show_test_notification');
  return {zhCN,en};
})()`);

const invalidLocale = await evaluate(`(async()=>{
  try {
    await window.__TAURI_INTERNALS__.invoke('set_ui_locale',{preference:'fr',locale:'fr'});
    return {resolved:true};
  } catch(error) {
    return {
      resolved:false,
      code:error?.code,
      params:error?.params,
      debugMessage:error?.debugMessage
    };
  }
})()`);

const rapidSwitch = await evaluate(`(async()=>{
  const sequence=[['zh-CN','zh-CN'],['en','en'],['zh-CN','zh-CN'],['en','en']];
  for(const [preference,locale] of sequence) {
    await window.__TAURI_INTERNALS__.invoke('set_ui_locale',{preference,locale});
  }
  return {
    native: await window.__TAURI_INTERNALS__.invoke('get_ui_locale'),
    recording: await window.__TAURI_INTERNALS__.invoke('is_recording')
  };
})()`);

await new Promise((resolve) => setTimeout(resolve, 500));
const assertions = {
  hydrated: initial.ready === "true",
  initialPersistence: initial.preference === "en" && initial.native?.preference === "en",
  zhSync:
    switchResult.zhCN.document === "zh-CN" &&
    switchResult.zhCN.native?.preference === "zh-CN" &&
    switchResult.zhCN.native?.locale === "zh-CN",
  enSync:
    switchResult.en.document === "en" &&
    switchResult.en.native?.preference === "en" &&
    switchResult.en.native?.locale === "en",
  invalidLocaleStructured:
    invalidLocale.resolved === false &&
    invalidLocale.code === "I18N_INVALID_LOCALE" &&
    invalidLocale.params &&
    typeof invalidLocale.debugMessage === "string",
  rapidSwitchStable:
    rapidSwitch.native?.preference === "en" &&
    rapidSwitch.native?.locale === "en" &&
    rapidSwitch.recording === false,
  noRuntimeExceptions: diagnostics.exceptions.length === 0,
  noLogErrors: diagnostics.logErrors.length === 0,
  noConsoleErrors: diagnostics.consoleErrors.length === 0,
};
const passed = Object.values(assertions).every(Boolean);
const report = {
  phase: "15.6-stage-3-tauri-native-i18n",
  generatedAt: new Date().toISOString(),
  port,
  passed,
  assertions,
  initial,
  switchResult,
  invalidLocale,
  rapidSwitch,
  diagnostics: {
    exceptions: diagnostics.exceptions.length,
    logErrors: diagnostics.logErrors.length,
    consoleErrors: diagnostics.consoleErrors.length,
  },
};
await fs.mkdir(path.dirname(reportPath), { recursive: true });
await fs.writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
socket.close();
process.stdout.write(JSON.stringify({ reportPath, passed, assertions }, null, 2) + "\n");
if (!passed) process.exitCode = 1;
