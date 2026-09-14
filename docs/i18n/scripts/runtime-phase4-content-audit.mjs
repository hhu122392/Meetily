#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9348);
const root = path.resolve(process.argv[3] || ".");
const reportPath = path.join(root, "docs/i18n/audit/phase-4-content/runtime-audit.json");
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

const result = await evaluate(`(async()=>{
  const invoke=(command,args={})=>window.__TAURI_INTERNALS__.invoke(command,args);
  const list=(contentLocale)=>invoke('api_list_templates_v2',{request:{origin:'builtin',contentLocale}});
  const get=(templateId,contentLocale)=>invoke('api_get_template_v2',{request:{templateId,origin:'builtin',contentLocale}});
  const snapshot=async()=>({
    ui: await invoke('get_ui_locale'),
    transcript: await invoke('api_get_transcript_config'),
    model: await invoke('api_get_model_config')
  });
  localStorage.setItem('meetily.uiLocale','en');
  window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'en'}));
  await new Promise(resolve=>setTimeout(resolve,750));
  const before=await snapshot();
  const en=await list('en');
  const zh=await list('zh-CN');
  const fallback=await list('fr-FR');
  const enDetails=await get('standard_meeting','en');
  const zhDetails=await get('standard_meeting','zh-CN');
  localStorage.setItem('meetily.uiLocale','zh-CN');
  window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));
  await new Promise(resolve=>setTimeout(resolve,750));
  const after=await snapshot();
  const stable=(items)=>items.templates.map(item=>item.id).sort();
  return {
    before,after,en,zh,fallback,enDetails,zhDetails,
    ids:{en:stable(en),zh:stable(zh),fallback:stable(fallback)}
  };
})()`);

const expectedIds = [
  "daily_standup",
  "project_sync",
  "psychatric_session",
  "retrospective",
  "sales_marketing_client_call",
  "standard_meeting",
].sort();
const sameJson = (left, right) => JSON.stringify(left) === JSON.stringify(right);
const all = (items, predicate) => items.every(predicate);
const assertions = {
  sixStableBuiltins: sameJson(result.ids.en, expectedIds) && sameJson(result.ids.zh, expectedIds),
  englishResources: all(result.en.templates, (item) => item.locale === "en" && item.origin === "builtin"),
  chineseResources: all(result.zh.templates, (item) => item.locale === "zh-CN" && item.origin === "builtin"),
  unsupportedLocaleFallsBackToEnglish:
    sameJson(result.ids.fallback, expectedIds) &&
    all(result.fallback.templates, (item) => item.locale === "en"),
  localizedNamesDiffer:
    result.en.templates.every((item) => {
      const translated = result.zh.templates.find((candidate) => candidate.id === item.id);
      return translated && translated.name !== item.name && translated.description !== item.description;
    }),
  detailsPreserveIdentityAndStructure:
    result.enDetails.template.id === result.zhDetails.template.id &&
    result.enDetails.template.version === result.zhDetails.template.version &&
    result.enDetails.template.locale === "en" &&
    result.zhDetails.template.locale === "zh-CN" &&
    sameJson(
      result.enDetails.template.sections.map((section) => section.id),
      result.zhDetails.template.sections.map((section) => section.id),
    ),
  uiSwitchDoesNotMutateTranscriptConfig: sameJson(result.before.transcript, result.after.transcript),
  uiSwitchDoesNotMutateModelConfig: sameJson(result.before.model, result.after.model),
  uiSwitchReachedChinese:
    result.before.ui?.preference === "en" &&
    result.before.ui?.locale === "en" &&
    result.after.ui?.preference === "zh-CN" &&
    result.after.ui?.locale === "zh-CN",
  noRuntimeExceptions: diagnostics.exceptions.length === 0,
  noLogErrors: diagnostics.logErrors.length === 0,
  noConsoleErrors: diagnostics.consoleErrors.length === 0,
};
const passed = Object.values(assertions).every(Boolean);
const report = {
  phase: "15.7-stage-4-template-and-ai-content-i18n",
  generatedAt: new Date().toISOString(),
  port,
  passed,
  assertions,
  counts: {
    english: result.en.templates.length,
    chinese: result.zh.templates.length,
    fallback: result.fallback.templates.length,
  },
  ids: result.ids,
  standardMeeting: {
    english: {
      name: result.enDetails.template.name,
      locale: result.enDetails.template.locale,
      sectionIds: result.enDetails.template.sections.map((section) => section.id),
    },
    chinese: {
      name: result.zhDetails.template.name,
      locale: result.zhDetails.template.locale,
      sectionIds: result.zhDetails.template.sections.map((section) => section.id),
    },
  },
  stateIsolation: {
    uiBefore: result.before.ui,
    uiAfter: result.after.ui,
    transcriptConfigUnchanged: assertions.uiSwitchDoesNotMutateTranscriptConfig,
    modelConfigUnchanged: assertions.uiSwitchDoesNotMutateModelConfig,
  },
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
