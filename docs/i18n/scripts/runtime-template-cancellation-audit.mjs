#!/usr/bin/env node

import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

const port = Number(process.argv[2] || 9368);
const root = path.resolve(process.argv[3] || '.');
const runId = `runtime_template_cancel_${Date.now()}`;
const jobId = `${runId}_job`;
const itemIds = [`${runId}_item_a`, `${runId}_item_b`];
const workRoot = path.join(root, 'target-phase5-release', 'runtime-template-cancellation');
const workDirectory = path.join(workRoot, runId);
const evidenceDirectory = path.join(root, 'target', 'release', 'docs', 'phase-0-custom-summary-templates', 'audit');
const reportPath = path.join(evidenceDirectory, 'template-cancellation-runtime-audit.json');
const screenshotPath = path.join(evidenceDirectory, 'template-cancellation-terminal-zh-CN.png');
await fs.mkdir(workDirectory, { recursive: true });
await fs.mkdir(evidenceDirectory, { recursive: true });

const crcTable = new Uint32Array(256);
for (let index = 0; index < 256; index += 1) {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) value = (value & 1) ? (0xedb88320 ^ (value >>> 1)) : (value >>> 1);
  crcTable[index] = value >>> 0;
}

function crc32(buffer) {
  let value = 0xffffffff;
  for (const byte of buffer) value = crcTable[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
}

function storedZip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [entryName, content] of entries) {
    const name = Buffer.from(entryName, 'utf8');
    const data = Buffer.isBuffer(content) ? content : Buffer.from(content, 'utf8');
    const checksum = crc32(data);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0, 6);
    local.writeUInt16LE(0, 8);
    local.writeUInt32LE(checksum, 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(name.length, 26);
    locals.push(local, name, data);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt32LE(checksum, 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt32LE(offset, 42);
    centrals.push(central, name);
    offset += local.length + name.length + data.length;
  }
  const centralSize = centrals.reduce((total, part) => total + part.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}

const contentTypes = `<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>`;
const paragraph = '<w:p><w:r><w:t>Cancelable audit paragraph</w:t></w:r></w:p>';
const targetXmlBytes = 18 * 1024 * 1024;
const documentXml = `<w:document xmlns:w="urn:meetily:cancel-audit"><w:body>${paragraph.repeat(Math.floor(targetXmlBytes / paragraph.length))}</w:body></w:document>`;
const docxBytes = storedZip([
  ['[Content_Types].xml', contentTypes],
  ['word/document.xml', documentXml],
]);
const inputPaths = [path.join(workDirectory, 'large-a.docx'), path.join(workDirectory, 'large-b.docx')];
await Promise.all(inputPaths.map((filePath) => fs.writeFile(filePath, docxBytes)));

async function waitForTarget(timeoutMs = 45_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
      const target = targets.find((candidate) => candidate.type === 'page');
      if (target?.webSocketDebuggerUrl) return target;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`No Tauri page target appeared on port ${port}`);
}

const target = await waitForTarget();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});

let nextId = 1;
const pending = new Map();
const diagnostics = { exceptions: [], consoleErrors: [], logErrors: [] };
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    const operation = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) operation.reject(new Error(JSON.stringify(message.error)));
    else operation.resolve(message.result);
    return;
  }
  if (message.method === 'Runtime.exceptionThrown') diagnostics.exceptions.push(message.params);
  if (message.method === 'Runtime.consoleAPICalled' && message.params?.type === 'error') diagnostics.consoleErrors.push(message.params);
  if (message.method === 'Log.entryAdded' && message.params?.entry?.level === 'error') diagnostics.logErrors.push(message.params.entry);
});

function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP ${method} timed out`));
    }, 20_000);
    pending.set(id, {
      resolve: (value) => { clearTimeout(timeout); resolve(value); },
      reject: (error) => { clearTimeout(timeout); reject(error); },
    });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const response = await call('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
  return response.result.value;
}

async function invokeResult(command, args = {}) {
  return evaluate(`(async()=>{
    try { return {ok:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}; }
    catch(error) { return {ok:false,error:{code:error?.code,messageKey:error?.messageKey,retryable:error?.retryable,debugId:error?.debugId}}; }
  })()`);
}

async function invoke(command, args = {}) {
  const result = await invokeResult(command, args);
  if (!result.ok) throw new Error(`${command} failed: ${JSON.stringify(result.error)}`);
  return result.value;
}

async function waitFor(expression, description, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (await evaluate(expression)) return;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  const state = await evaluate(`({route:location.pathname,lang:document.documentElement.lang,text:document.body?.innerText.slice(0,5000)})`);
  throw new Error(`Timed out waiting for ${description}: ${JSON.stringify(state)}`);
}

async function waitForJob(expectedJobId, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = await invokeResult('api_get_template_import_job', { request: { jobId: expectedJobId } });
    if (result.ok) return result.value;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`Import job ${expectedJobId} was not registered`);
}

async function screenshot(filePath) {
  const result = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await fs.writeFile(filePath, Buffer.from(result.data, 'base64'));
}

function converterProcesses() {
  try {
    const output = execFileSync('tasklist.exe', ['/NH', '/FO', 'CSV'], { encoding: 'utf8' });
    return output.split(/\r?\n/).filter((line) => /"soffice(?:\.bin)?\.exe"/i.test(line)).length;
  } catch {
    return null;
  }
}

async function converterTempDirectories() {
  return (await fs.readdir(os.tmpdir())).filter((name) => name.startsWith('meetily-template-doc-')).sort();
}

await call('Runtime.enable');
await call('Log.enable');
await call('Page.enable');
await call('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
await waitFor("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')", 'Tauri IPC readiness', 45_000);

let initialOnboarding;
let report;
try {
  initialOnboarding = await invoke('get_onboarding_status');
  await invoke('save_onboarding_status_cmd', {
    status: {
      version: '1.0', completed: true, current_step: 4,
      model_status: { parakeet: 'not_downloaded', summary: 'not_downloaded' },
      last_updated: new Date().toISOString(),
    },
  });
  const beforeTemplates = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: true, contentLocale: 'zh-CN' },
  });
  const beforeTemp = await converterTempDirectories();
  const beforeProcesses = converterProcesses();

  await evaluate(`(()=>{
    window.__TMP708_DIRECT__={state:'running'};
    window.__TAURI_INTERNALS__.invoke('api_preview_template_imports',{request:${JSON.stringify({ paths: inputPaths, jobId, itemIds })}})
      .then((value)=>{window.__TMP708_DIRECT__={state:'resolved',value};})
      .catch((error)=>{window.__TMP708_DIRECT__={state:'rejected',error:{code:error?.code,messageKey:error?.messageKey,debugId:error?.debugId}};});
    return true;
  })()`);
  const accepted = await waitForJob(jobId);
  const firstCancel = await invoke('api_cancel_template_import_job', { request: { jobId } });
  const repeatedCancel = await invoke('api_cancel_template_import_job', { request: { jobId } });
  await waitFor("window.__TMP708_DIRECT__?.state!=='running'", 'direct native cancellation result', 30_000);
  const directResult = await evaluate('window.__TMP708_DIRECT__');
  const terminal = await invoke('api_get_template_import_job', { request: { jobId } });

  await call('Page.reload');
  await waitFor("Boolean(window.__TAURI_INTERNALS__?.invoke && document.readyState === 'complete')", 'IPC after onboarding reload');
  await evaluate(`(()=>{localStorage.setItem('meetily.uiLocale','zh-CN');window.dispatchEvent(new StorageEvent('storage',{key:'meetily.uiLocale',newValue:'zh-CN'}));})()`);
  await call('Page.navigate', { url: 'http://tauri.localhost/settings/templates' });
  await waitFor("location.pathname==='/settings/templates' && document.documentElement.lang==='zh-CN' && document.body.innerText.includes('导入模板')", 'Chinese template library');
  await new Promise((resolve) => setTimeout(resolve, 1500));
  await evaluate(`(()=>{
    if(window.__TMP708_DELAY_INSTALLED__)return true;
    window.__TMP708_DELAY_INSTALLED__=true;
    const nativeInvoke=window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
    window.__TAURI_INTERNALS__.invoke=(command,args,...rest)=>{
      const result=nativeInvoke(command,args,...rest);
      if(command==='api_preview_template_imports')return result.then(async(value)=>{
        if(value?.status==='cancelled')await new Promise((resolve)=>setTimeout(resolve,1200));
        return value;
      });
      return result;
    };
    return true;
  })()`);
  await invoke('plugin:event|emit_to', {
    target: { kind: 'Webview', label: 'main' },
    event: 'tauri://drag-drop',
    payload: { paths: inputPaths, position: { x: 600, y: 350 } },
  });
  await waitFor("document.body.innerText.includes('large-a.docx')&&document.body.innerText.includes('取消处理')", 'running import UI');
  const waitingUi = await evaluate(`new Promise((resolve,reject)=>{
    const snapshot=()=>{
      const bodyText=document.body.innerText;
      if(!bodyText.includes('正在等待转换器停止'))return false;
      resolve({
        htmlLang:document.documentElement.lang,
        hasWaiting:true,
        hasCancelRequested:bodyText.includes('已请求取消'),
        saveDisabled:[...document.querySelectorAll('button')].some((node)=>(node.textContent||'').includes('保存可用模板')&&node.disabled),
        bodyText:bodyText.slice(0,5000)
      });
      return true;
    };
    const observer=new MutationObserver(()=>{if(snapshot())observer.disconnect();});
    observer.observe(document.body,{subtree:true,childList:true,characterData:true,attributes:true});
    const button=[...document.querySelectorAll('button')].find((node)=>(node.textContent||'').includes('取消处理'));
    if(!button||button.disabled){observer.disconnect();reject(new Error('Could not click the batch cancellation button'));return;}
    button.click();
    snapshot();
    setTimeout(()=>{observer.disconnect();reject(new Error('Backend-stop waiting state was not observed'));},5000);
  })`);
  await waitFor("document.body.innerText.includes('已取消')&&!document.body.innerText.includes('正在等待转换器停止')", 'backend-confirmed cancelled UI', 30_000);
  await screenshot(screenshotPath);
  const terminalUi = await evaluate(`({
    cancelledCount:[...document.querySelectorAll('button')].filter((node)=>(node.textContent||'').includes('已取消')).length,
    hasRawCode:document.body.innerText.includes('TEMPLATE_CANCELLED'),
    bodyText:document.body.innerText.slice(0,5000)
  })`);

  const afterTemplates = await invoke('api_list_templates_v2', {
    request: { origin: 'custom', includeInvalid: true, includeTrash: true, contentLocale: 'zh-CN' },
  });
  await new Promise((resolve) => setTimeout(resolve, 500));
  const afterTemp = await converterTempDirectories();
  const afterProcesses = converterProcesses();
  const directItems = directResult.value?.items ?? [];
  const assertions = {
    stableJobId: accepted.jobId === jobId && terminal.jobId === jobId,
    acceptedOrRunningObserved: ['accepted', 'running'].includes(accepted.status),
    cancelRequestObserved: firstCancel.status === 'cancel_requested',
    repeatedCancellationIdempotent: repeatedCancel.status === 'cancel_requested' || repeatedCancel.status === 'cancelled',
    backendConfirmedCancelled: directResult.state === 'resolved' && directResult.value?.status === 'cancelled' && terminal.status === 'cancelled',
    allDirectItemsTerminalCancelled: directItems.length === 2 && directItems.every((item) => item.status === 'cancelled'),
    chineseWaitingStateVisible: waitingUi.htmlLang === 'zh-CN' && waitingUi.hasWaiting && waitingUi.hasCancelRequested,
    saveBlockedWhileCancelling: waitingUi.saveDisabled,
    uiWaitsForBackendConfirmation: terminalUi.cancelledCount >= 1,
    noRawErrorBubbling: !terminalUi.hasRawCode,
    zeroTemplateWrites: JSON.stringify(beforeTemplates.templates) === JSON.stringify(afterTemplates.templates)
      && JSON.stringify(beforeTemplates.deletedTemplates) === JSON.stringify(afterTemplates.deletedTemplates),
    zeroConverterTempResidue: JSON.stringify(beforeTemp) === JSON.stringify(afterTemp),
    zeroConverterProcessResidue: beforeProcesses === afterProcesses,
    zeroFrontendDiagnostics: diagnostics.exceptions.length === 0 && diagnostics.consoleErrors.length === 0 && diagnostics.logErrors.length === 0,
  };
  report = {
    auditId: runId,
    timestamp: new Date().toISOString(),
    candidate: path.join(root, 'target-phase5-release', 'release', 'meetily.exe'),
    input: { bytesPerDocx: docxBytes.length, paths: inputPaths.map((value) => path.basename(value)), jobId, itemIds },
    direct: { accepted, firstCancel, repeatedCancel, result: directResult, terminal },
    ui: { waiting: waitingUi, terminal: terminalUi, screenshot: screenshotPath },
    cleanup: { beforeTemp, afterTemp, beforeProcesses, afterProcesses },
    diagnostics: {
      exceptions: diagnostics.exceptions.length,
      consoleErrors: diagnostics.consoleErrors.length,
      logErrors: diagnostics.logErrors.length,
    },
    assertions,
    passed: Object.values(assertions).every(Boolean),
  };
  await fs.writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
  if (!report.passed) throw new Error(`TMP-708 runtime assertions failed: ${JSON.stringify(assertions)}`);
} finally {
  if (initialOnboarding) {
    try { await invoke('save_onboarding_status_cmd', { status: initialOnboarding }); } catch {}
  }
  socket.close();
  const resolvedWork = path.resolve(workDirectory);
  if (resolvedWork.startsWith(`${path.resolve(workRoot)}${path.sep}`)) {
    await fs.rm(resolvedWork, { recursive: true, force: true });
  }
}

console.log(JSON.stringify({ reportPath, screenshotPath, passed: report?.passed, assertions: report?.assertions }, null, 2));
